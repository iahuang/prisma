//! State that outlives one script run: the GPU, fonts, effect definitions,
//! decoded images and materialized rasters. Watch mode and sweeps reuse one
//! Engine, so everything here is a warm cache across renders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::effects::Effects;
use crate::gpu::Gpu;
use crate::ir::{Document, Font, ImageSrc, Kind, Text};
use crate::render::render_document;
use crate::text::Fonts;

/// Bump when rendering changes in a way that invalidates cached rasters.
const CACHE_VERSION: u32 = 4;

/// Pixels materialized from a scene. Scripts read them (`r:at(x, y)`) and draw them (`Image { src = r }`).
pub struct Raster {
    pub key: String,
    /// Doc-px size of the region this covers.
    pub size: [f64; 2],
    pub dpi: f64,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8.
    pub pixels: Vec<u8>,
    pub image: ImageData,
}

pub struct Sample {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Raster {
    fn new(key: String, size: [f64; 2], dpi: f64, width: u32, height: u32, pixels: Vec<u8>) -> Raster {
        let image = ImageData {
            data: Blob::new(Arc::new(pixels.clone())),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        };
        Raster { key, size, dpi, width, height, pixels, image }
    }

    /// Bilinear sample at doc-px coordinates. Outside the raster reads transparent.
    pub fn at(&self, x: f64, y: f64) -> Sample {
        let k = self.dpi / 96.0;
        let (fx, fy) = (x * k - 0.5, y * k - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let mut out = [0.0f64; 4];
        for (dx, dy, w) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
            let (px, py) = (x0 as i64 + dx, y0 as i64 + dy);
            if px < 0 || py < 0 || px >= self.width as i64 || py >= self.height as i64 {
                continue;
            }
            let i = (py as usize * self.width as usize + px as usize) * 4;
            for c in 0..4 {
                out[c] += self.pixels[i + c] as f64 / 255.0 * w;
            }
        }
        Sample { r: out[0], g: out[1], b: out[2], a: out[3] }
    }
}

pub struct Engine {
    pub gpu: Gpu,
    pub fonts: Fonts,
    pub effects: Effects,
    /// Scripts may only read files under this directory (`--root`).
    pub base_dir: PathBuf,
    /// Relative paths in scripts resolve against this: the entry script's directory.
    pub script_dir: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub dump_dir: Option<PathBuf>,
    pub profile: bool,
    images: HashMap<PathBuf, (SystemTime, ImageData)>,
    pub rasters: HashMap<String, Rc<Raster>>,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Engine {
    pub fn new(base_dir: PathBuf) -> Result<Engine> {
        Ok(Engine {
            gpu: Gpu::new()?,
            fonts: Fonts::new(),
            effects: Effects::default(),
            script_dir: base_dir.clone(),
            base_dir,
            cache_dir: None,
            dump_dir: None,
            profile: false,
            images: HashMap::new(),
            rasters: HashMap::new(),
        })
    }

    /// Resolves a script-relative path, refusing anything outside `base_dir`.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let p = self.script_dir.join(rel);
        let canon = p.canonicalize().with_context(|| format!("{rel:?} not found (looked at {})", p.display()))?;
        let root = self.base_dir.canonicalize()?;
        if !canon.starts_with(&root) {
            bail!("{rel:?} is outside the project directory {} (pass --root to widen it)", root.display());
        }
        Ok(canon)
    }

    /// Pixel size of an image file (header only).
    pub fn image_size(&self, path: &Path) -> Result<(u32, u32)> {
        image::image_dimensions(path).with_context(|| format!("reading {}", path.display()))
    }

    /// A drawable image and its pixels per source unit (1 for files, dpi/96 for rasters).
    pub fn image(&mut self, src: &ImageSrc) -> Result<(ImageData, f64)> {
        match src {
            ImageSrc::Raster(key) => {
                let r = self.rasters.get(key).ok_or_else(|| anyhow!("raster {key} is gone"))?;
                Ok((r.image.clone(), r.dpi / 96.0))
            }
            ImageSrc::File(path) => {
                let path = PathBuf::from(path);
                let stamp = mtime(&path).unwrap_or(SystemTime::UNIX_EPOCH);
                if let Some((t, img)) = self.images.get(&path)
                    && *t == stamp
                {
                    return Ok((img.clone(), 1.0));
                }
                let decoded = image::open(&path).with_context(|| format!("decoding {}", path.display()))?.into_rgba8();
                let (w, h) = decoded.dimensions();
                if w > 8192 || h > 8192 {
                    bail!("{} is {w}x{h}; images are limited to 8192px on a side for now", path.display());
                }
                let img = ImageData {
                    data: Blob::new(Arc::new(decoded.into_raw())),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::Alpha,
                    width: w,
                    height: h,
                };
                self.images.insert(path, (stamp, img.clone()));
                Ok((img, 1.0))
            }
        }
    }

    /// Inputs a document depends on beyond its IR: file mtimes and effect code.
    fn stamps(&self, doc: &Document) -> Vec<String> {
        let mut out = Vec::new();
        for n in &doc.children {
            n.walk(&mut |n| match &n.kind {
                Kind::Image { src: ImageSrc::File(p), .. } => out.push(format!("{p}@{:?}", mtime(Path::new(p)))),
                Kind::Effect { effect, .. } => {
                    out.push(self.effects.defs.get(effect).map(|d| d.module.clone()).unwrap_or_default())
                }
                Kind::Text(Text { font: Font::File(p), .. }) => out.push(format!("{p}@{:?}", mtime(Path::new(p)))),
                _ => {}
            });
        }
        out.sort();
        out.dedup();
        out
    }

    /// Renders a document to a raster at `dpi`, memoized in memory and on disk by content hash.
    pub fn rasterize(&mut self, doc: &Document, dpi: f64) -> Result<Rc<Raster>> {
        let mut h = blake3::Hasher::new();
        h.update(&CACHE_VERSION.to_le_bytes());
        h.update(&dpi.to_le_bytes());
        h.update(serde_json::to_string(doc)?.as_bytes());
        for s in self.stamps(doc) {
            h.update(s.as_bytes());
        }
        let key = h.finalize().to_hex()[..16].to_string();
        if let Some(r) = self.rasters.get(&key) {
            return Ok(r.clone());
        }
        let file = self.cache_dir.as_ref().map(|d| d.join(format!("{key}.rgba")));
        if let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok())
            && bytes.len() >= 8
        {
            let w = u32::from_le_bytes(bytes[0..4].try_into()?);
            let hgt = u32::from_le_bytes(bytes[4..8].try_into()?);
            if bytes.len() == 8 + (w * hgt * 4) as usize {
                let r = Rc::new(Raster::new(key.clone(), doc.size, dpi, w, hgt, bytes[8..].to_vec()));
                self.rasters.insert(key, r.clone());
                return Ok(r);
            }
        }
        let (tex, _) = render_document(self, doc, dpi / 96.0, None)?;
        let pixels = self.gpu.read(&tex)?;
        let r = Rc::new(Raster::new(key.clone(), doc.size, dpi, tex.width(), tex.height(), pixels));
        if let Some(f) = file {
            std::fs::create_dir_all(f.parent().unwrap())?;
            let mut bytes = Vec::with_capacity(8 + r.pixels.len());
            bytes.extend(r.width.to_le_bytes());
            bytes.extend(r.height.to_le_bytes());
            bytes.extend(&r.pixels);
            std::fs::write(f, bytes)?;
        }
        self.rasters.insert(key, r.clone());
        Ok(r)
    }

    /// Registers pixels made outside a render (e.g. contact sheet cells) as a raster.
    pub fn adopt_pixels(&mut self, key: String, size: [f64; 2], dpi: f64, w: u32, h: u32, pixels: Vec<u8>) -> Rc<Raster> {
        let r = Rc::new(Raster::new(key.clone(), size, dpi, w, h, pixels));
        self.rasters.insert(key, r.clone());
        r
    }
}

pub fn write_png(path: &Path, w: u32, h: u32, pixels: Vec<u8>) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?);
    PngEncoder::new_with_quality(file, CompressionType::Fast, FilterType::Adaptive)
        .write_image(&pixels, w, h, image::ExtendedColorType::Rgba8)
        .with_context(|| format!("writing {}", path.display()))
}
