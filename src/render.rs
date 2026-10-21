//! IR -> pixels.
//!
//! Everything is encoded into vello scenes in absolute device coordinates.
//! Nodes that need their own pixels (effects, blur) are rendered offscreen into
//! a texture covering just their device-space bounds, clipped to the region of
//! interest passed down from above (so a small blurred shape costs a small
//! texture, and an effect only renders what its parent will read). The result
//! is drawn back into the parent scene as a premultiplied image.
//!
//! vello's image atlas can serve stale pixels when a CPU image's blob id is
//! reused across separate renders, so every scene gets its own ids for the
//! images it draws (same bytes, fresh id, deduped within the scene).

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use vello::kurbo::{self, Affine, BezPath, Rect, Shape};
use vello::peniko::{self, Blob, BlendMode, Brush, Compose, Fill, Gradient, ImageAlphaType, ImageBrush, ImageData, ImageFormat, Mix};
use vello::{Scene, wgpu};

use crate::engine::Engine;
use crate::gpu::{BumpCheck, IRect, MaskMode, Overflow};
use crate::ir::{Blend, Cap, Document, Fit, Geom, Join, Kind, Node, Paint, Stroke};
use crate::text::Fonts;
use crate::units::to_color;

/// Flattening tolerance, device px, for geometry a distance field is computed
/// from. vello flattens curves to ~0.25px chords, invisible in a picture but
/// ~10 degree steps in the normals of a 70px circle; lines it draws exactly.
const FIELD_TOLERANCE: f64 = 0.01;

pub struct Pass<'a> {
    eng: &'a mut Engine,
    /// Inside an Sdf's children: curves (and glyphs) go to vello pre-flattened at FIELD_TOLERANCE.
    fine: bool,
    scale: f64,
    resolution: [f32; 2],
    /// The canvas grown by its `margin`: no pass reads beyond it.
    readable: IRect,
    overrides: Vec<ImageData>,
    pub offscreen_passes: usize,
    dumped: usize,
    /// Per scene being built (innermost last): source blob id -> image with this scene's id.
    scene_images: Vec<HashMap<u64, ImageData>>,
    /// Check each vello render as it's made, splitting one that overflows. Off
    /// for the first try: waiting on every render would stall the GPU.
    checked: bool,
    /// Without `checked`, the renders to check once the GPU is done.
    unchecked: Vec<BumpCheck>,
    /// The first overflow that split a render (and its hint), and how many splits it took.
    overflow: Option<(String, &'static str)>,
    splits: usize,
}

/// Regions this small aren't split further: if one overflows, smaller won't fit either.
const MIN_SPLIT: u32 = 128;

/// The largest image vello draws, on a side: its image atlas (vello_encoding's
/// MAX_ATLAS_SIZE) holds every image a scene draws, and it silently skips one
/// that doesn't fit.
const ATLAS_MAX: u32 = 8192;

/// Renders a document at `scale` device px per doc px. `region` is in device px
/// (None = the whole canvas); noise and grain are positioned absolutely, so a
/// region matches the same pixels of a full render.
pub fn render_document(eng: &mut Engine, doc: &Document, scale: f64, region: Option<IRect>) -> Result<(wgpu::Texture, IRect)> {
    let t0 = Instant::now();
    let canvas = IRect {
        x: 0,
        y: 0,
        w: (doc.size[0] * scale).round().max(1.0) as u32,
        h: (doc.size[1] * scale).round().max(1.0) as u32,
    };
    let region = region.unwrap_or(canvas);
    let max = eng.gpu.max_texture();
    if region.w > max || region.h > max {
        bail!("output {}x{} exceeds the GPU's {max}px texture limit (use --crop to render it in pieces)", region.w, region.h);
    }
    // By default reads reach one canvas-length past the edge: content farther out
    // rarely shows. Huge margins mean "unclipped"; the min keeps the arithmetic in range.
    let margin = doc.margin.unwrap_or(doc.size[0].max(doc.size[1]));
    let readable = canvas.expand((margin * scale).ceil().min(1e9) as u32);
    let attempt = |eng: &mut Engine, checked: bool| {
        let mut pass = Pass::new(eng, scale, canvas, readable, checked);
        let result = pass.document(doc, canvas, region);
        pass.finish();
        (result, pass.offscreen_passes, pass.overflow, pass.splits)
    };
    let mut out = attempt(eng, false);
    if out.0.as_ref().is_err_and(|e| e.is::<Overflow>()) {
        // Some render outgrew vello's buffers. Rare, so only now pay for
        // checking each one, to find it and render it in pieces.
        out = attempt(eng, true);
    }
    let (result, passes, overflow, splits) = out;
    if let Some((overflow, hint)) = overflow {
        eprintln!("warning: {overflow}, so it was rendered in pieces ({splits} splits), which is slower. {hint}");
    }
    if eng.profile {
        eprintln!("render {}x{} @{scale}: {passes} offscreen passes, {:.1}ms", region.w, region.h, t0.elapsed().as_secs_f64() * 1e3);
    }
    Ok((result?, region))
}

fn irect(r: IRect) -> Rect {
    Rect::new(r.x as f64, r.y as f64, (r.x + r.w as i32) as f64, (r.y + r.h as i32) as f64)
}

fn round_out(r: Rect) -> Option<IRect> {
    let r = r.expand();
    if !(r.width() > 0.0 && r.height() > 0.0) || !r.is_finite() {
        return None;
    }
    Some(IRect { x: r.x0 as i32, y: r.y0 as i32, w: (r.x1 - r.x0) as u32, h: (r.y1 - r.y0) as u32 })
}

/// Effects and distance fields always need their own pass, and so do luminance
/// masks; alpha and inverted masks are vello DestIn/DestOut layers.
fn needs_pass(n: &Node) -> bool {
    matches!(n.kind, Kind::Effect { .. } | Kind::Sdf { .. } | Kind::Mask { luminance: true, .. })
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = a.intersect(b);
    (r.width() > 0.0 && r.height() > 0.0).then_some(r)
}

/// How much a transform scales lengths (geometric mean of the axes).
fn scale_of(xf: Affine) -> f64 {
    xf.determinant().abs().sqrt()
}

/// The most a transform stretches any length (its largest singular value).
fn max_scale_of(xf: Affine) -> f64 {
    let [a, b, c, d, _, _] = xf.as_coeffs();
    let s = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    ((s + (s * s - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt()
}

fn mix(b: Blend) -> BlendMode {
    let m = match b {
        Blend::Normal => Mix::Normal,
        Blend::Multiply => Mix::Multiply,
        Blend::Screen => Mix::Screen,
        Blend::Overlay => Mix::Overlay,
        Blend::Darken => Mix::Darken,
        Blend::Lighten => Mix::Lighten,
        Blend::ColorDodge => Mix::ColorDodge,
        Blend::ColorBurn => Mix::ColorBurn,
        Blend::HardLight => Mix::HardLight,
        Blend::SoftLight => Mix::SoftLight,
        Blend::Difference => Mix::Difference,
        Blend::Exclusion => Mix::Exclusion,
        Blend::Hue => Mix::Hue,
        Blend::Saturation => Mix::Saturation,
        Blend::Color => Mix::Color,
        Blend::Luminosity => Mix::Luminosity,
        Blend::Plus => return BlendMode::new(Mix::Normal, Compose::Plus),
    };
    BlendMode::new(m, Compose::SrcOver)
}

pub fn geom_path(g: &Geom) -> Result<BezPath> {
    const TOL: f64 = 1e-3;
    Ok(match *g {
        Geom::Rect { x, y, w, h, r } => kurbo::RoundedRect::new(x, y, x + w, y + h, r).to_path(TOL),
        Geom::Circle { cx, cy, r } => kurbo::Circle::new((cx, cy), r).to_path(TOL),
        Geom::Ellipse { cx, cy, rx, ry, rotate } => kurbo::Ellipse::new((cx, cy), (rx, ry), rotate.to_radians()).to_path(TOL),
        Geom::Line { x1, y1, x2, y2 } => kurbo::Line::new((x1, y1), (x2, y2)).to_path(TOL),
        Geom::Path { ref d, transform } => Affine::new(transform) * BezPath::from_svg(d).map_err(|e| anyhow!("bad path data: {e}"))?,
    })
}

/// For an image bigger than vello's atlas: the part of it that the source rect
/// `src` draws (plus a pixel of margin, so filtering at the edges matches),
/// resampled to no more than `px_per_src` device px per source px. Returns it
/// premultiplied, with the transform from its pixels to the image's.
fn atlas_fit(img: &ImageData, src: [f64; 4], px_per_src: f64) -> Result<(ImageData, Affine)> {
    let [sx, sy, sw, sh] = src;
    let x0 = (sx.floor() - 1.0).max(0.0) as u32;
    let y0 = (sy.floor() - 1.0).max(0.0) as u32;
    let x1 = ((sx + sw).ceil() + 1.0).min(img.width as f64) as u32;
    let y1 = ((sy + sh).ceil() + 1.0).min(img.height as f64) as u32;
    let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    let k = px_per_src.min(1.0);
    let (nw, nh) = (((w as f64 * k).ceil() as u32).max(1), ((h as f64 * k).ceil() as u32).max(1));
    if nw > ATLAS_MAX || nh > ATLAS_MAX {
        bail!(
            "this draws {w}x{h} px of a {}x{} image at {nw}x{nh} device px, over the {ATLAS_MAX}px vello can draw one image at: \
             crop it (`crop`) or draw it smaller",
            img.width,
            img.height
        );
    }
    let (data, stride) = (img.data.data(), img.width as usize * 4);
    let mut rows = Vec::with_capacity(w as usize * h as usize * 4);
    for y in y0..y0 + h {
        rows.extend_from_slice(&data[y as usize * stride + x0 as usize * 4..][..w as usize * 4]);
    }
    let mut part = image::RgbaImage::from_raw(w, h, rows).expect("rows are w x h");
    if img.alpha_type == ImageAlphaType::Alpha {
        for p in part.pixels_mut() {
            let a = p.0[3] as u32;
            for c in 0..3 {
                p.0[c] = ((p.0[c] as u32 * a + 127) / 255) as u8;
            }
        }
    }
    if (nw, nh) != (w, h) {
        part = image::imageops::resize(&part, nw, nh, image::imageops::FilterType::Triangle);
    }
    let out = ImageData {
        data: Blob::new(std::sync::Arc::new(part.into_raw())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::AlphaPremultiplied,
        width: nw,
        height: nh,
    };
    let at = Affine::translate((x0 as f64, y0 as f64)) * Affine::scale_non_uniform(w as f64 / nw as f64, h as f64 / nh as f64);
    Ok((out, at))
}

/// `path` as straight segments within `tolerance` (in its own units).
fn flattened(path: &BezPath, tolerance: f64) -> BezPath {
    let mut out = BezPath::new();
    kurbo::flatten(path, tolerance, |el| out.push(el));
    out
}

fn brush(p: &Paint, alpha: f32) -> Brush {
    let stops = |s: &[(f32, crate::units::Rgba)]| s.iter().map(|(t, c)| (*t, to_color(*c))).collect::<Vec<_>>();
    let b: Brush = match p {
        Paint::Solid { color } => to_color(*color).into(),
        Paint::Linear { from, to, stops: s } => {
            Gradient::new_linear((from[0], from[1]), (to[0], to[1])).with_stops(stops(s).as_slice()).into()
        }
        Paint::Radial { center, r, stops: s } => {
            Gradient::new_radial((center[0], center[1]), *r as f32).with_stops(stops(s).as_slice()).into()
        }
    };
    if alpha < 1.0 { b.multiply_alpha(alpha) } else { b }
}

fn kurbo_stroke(s: &Stroke) -> kurbo::Stroke {
    let cap = match s.cap {
        Cap::Butt => kurbo::Cap::Butt,
        Cap::Round => kurbo::Cap::Round,
        Cap::Square => kurbo::Cap::Square,
    };
    let join = match s.join {
        Join::Miter => kurbo::Join::Miter,
        Join::Round => kurbo::Join::Round,
        Join::Bevel => kurbo::Join::Bevel,
    };
    let st = kurbo::Stroke::new(s.width).with_caps(cap).with_join(join);
    if s.dash.is_empty() { st } else { st.with_dashes(0.0, s.dash.iter().copied()) }
}

impl<'a> Pass<'a> {
    fn new(eng: &'a mut Engine, scale: f64, canvas: IRect, readable: IRect, checked: bool) -> Pass<'a> {
        Pass {
            eng,
            fine: false,
            scale,
            resolution: [canvas.w as f32, canvas.h as f32],
            readable,
            overrides: Vec::new(),
            offscreen_passes: 0,
            dumped: 0,
            scene_images: Vec::new(),
            checked,
            unchecked: Vec::new(),
            overflow: None,
            splits: 0,
        }
    }

    fn document(&mut self, doc: &Document, canvas: IRect, region: IRect) -> Result<wgpu::Texture> {
        let xf = Affine::scale(self.scale);
        let roi = irect(region);
        let scene = self.scene(|p, scene| {
            if doc.bg[3] > 0.0 {
                scene.fill(Fill::NonZero, Affine::IDENTITY, to_color(doc.bg), None, &irect(canvas));
            }
            doc.children.iter().try_for_each(|n| p.encode(scene, n, xf, roi))
        })?;
        let tex = self.render_region(&scene, region)?;
        self.eng.gpu.check(&std::mem::take(&mut self.unchecked))?;
        Ok(tex)
    }

    /// Unregisters the offscreen textures handed to vello during this pass.
    fn finish(&mut self) {
        for img in self.overrides.drain(..) {
            self.eng.gpu.vello.override_image(&img, None);
        }
    }

    /// With --dump, writes a pass texture as `pass-NNN-<label>.png` so a wrong
    /// render can be bisected by looking at each intermediate.
    fn dump(&mut self, n: &Node, label: &str, tex: &wgpu::Texture, premultiplied: bool) -> Result<()> {
        let Some(dir) = self.eng.dump_dir.clone() else { return Ok(()) };
        let mut px = self.eng.gpu.read(tex)?;
        if premultiplied {
            for p in px.chunks_exact_mut(4) {
                if p[3] > 0 {
                    for c in 0..3 {
                        p[c] = ((p[c] as u32 * 255 + p[3] as u32 / 2) / p[3] as u32).min(255) as u8;
                    }
                }
            }
        }
        self.dumped += 1;
        let at = n.at.replace([':', '/'], "_");
        let name = format!("pass-{:03}-{at}-{label}.png", self.dumped);
        crate::engine::write_png(&dir.join(name), tex.width(), tex.height(), px)
    }

    /// `b` grown by `by` for a pass that reads neighbors, clipped to what's
    /// readable (absolute, so crops still match full renders). Never smaller than `b`.
    fn read_rect(&self, b: IRect, by: u32) -> IRect {
        b.expand(by).intersect(self.readable).map_or(b, |r| r.union(b))
    }

    /// Builds one scene (one vello render's worth of drawing).
    fn scene(&mut self, f: impl FnOnce(&mut Self, &mut Scene) -> Result<()>) -> Result<Scene> {
        self.scene_images.push(HashMap::new());
        let mut scene = Scene::new();
        let r = f(self, &mut scene);
        self.scene_images.pop();
        r.map(|_| scene)
    }

    /// `img` under an id unique to the scene being built.
    fn fresh(&mut self, img: ImageData) -> ImageData {
        let images = self.scene_images.last_mut().expect("drawing outside a scene");
        images
            .entry(img.data.id())
            .or_insert_with(|| {
                let (bytes, _) = img.data.clone().into_raw_parts();
                ImageData { data: Blob::new(bytes), ..img }
            })
            .clone()
    }

    /// Renders `r` of `scene`, in tiles of at most `Gpu::tile` px on a side,
    /// each its own vello render, stitched.
    fn render_region(&mut self, scene: &Scene, r: IRect) -> Result<wgpu::Texture> {
        let tile = self.eng.gpu.tile;
        if r.w <= tile && r.h <= tile {
            return self.render_piece(scene, r);
        }
        let mut pieces = Vec::new();
        for t in r.tiles(tile) {
            pieces.push((t, self.render_piece(scene, t)?));
        }
        Ok(self.eng.gpu.stitch(r, &pieces))
    }

    /// Renders `r` of `scene` in one vello render. With `checked`, a region that
    /// overflows vello's buffers is rendered in halves and stitched (same pixels, just slower).
    fn render_piece(&mut self, scene: &Scene, r: IRect) -> Result<wgpu::Texture> {
        let mut local = Scene::new();
        local.append(scene, Some(Affine::translate((-r.x as f64, -r.y as f64))));
        let (tex, bump) = self.eng.gpu.render_scene(&local, r.w, r.h)?;
        if !self.checked {
            self.unchecked.push(bump);
            return Ok(tex);
        }
        let err = match self.eng.gpu.check(&[bump]) {
            Ok(()) => return Ok(tex),
            Err(e) => e,
        };
        let Some(overflow) = err.downcast_ref::<Overflow>() else { return Err(err) };
        if !overflow.splittable() {
            bail!("{overflow}. Rendering in pieces can't help: vello flattens every path in the scene, in view or not. {}", overflow.hint());
        }
        if r.w.max(r.h) <= MIN_SPLIT {
            bail!("{overflow}, even rendering {}x{} px at a time. {}", r.w, r.h, overflow.hint());
        }
        if self.overflow.is_none() {
            self.overflow = Some((format!("{overflow} at {}x{} px", r.w, r.h), overflow.hint()));
        }
        self.splits += 1;
        let mut pieces = Vec::new();
        for p in r.split() {
            pieces.push((p, self.render_piece(scene, p)?));
        }
        Ok(self.eng.gpu.stitch(r, &pieces))
    }

    /// Makes a premultiplied texture drawable in a scene.
    fn register(&mut self, tex: wgpu::Texture) -> ImageData {
        let img = ImageData {
            data: Blob::new(std::sync::Arc::new([])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
            width: tex.width(),
            height: tex.height(),
        };
        self.eng.gpu.vello.override_image(
            &img,
            Some(wgpu::TexelCopyTextureInfoBase {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            }),
        );
        self.overrides.push(img.clone());
        img
    }

    fn encode(&mut self, scene: &mut Scene, n: &Node, xf: Affine, roi: Rect) -> Result<()> {
        if n.blur == 0.0 && !needs_pass(n) {
            return self.encode_plain(scene, n, xf, roi, true);
        }
        let Some(b) = self.bounds(n, xf, true)?.and_then(|r| intersect(r, roi)).and_then(round_out) else {
            return Ok(());
        };
        let tex = if n.blur > 0.0 { self.blurred(n, xf, b)? } else { self.pass(n, xf, b)? };
        if n.blend.is_normal() {
            self.draw_texture(scene, tex, b, n.opacity);
        } else {
            scene.push_layer(Fill::NonZero, mix(n.blend), 1.0, Affine::IDENTITY, &irect(b));
            self.draw_texture(scene, tex, b, n.opacity);
            scene.pop_layer();
        }
        Ok(())
    }

    /// Draws a premultiplied texture covering `b`. vello's image atlas skips an
    /// image wider or taller than ATLAS_MAX without a word, so a bigger texture
    /// goes in as tiles.
    fn draw_texture(&mut self, scene: &mut Scene, tex: wgpu::Texture, b: IRect, alpha: f32) {
        let pieces = if tex.width() <= ATLAS_MAX && tex.height() <= ATLAS_MAX {
            vec![(b, tex)]
        } else {
            b.tiles(ATLAS_MAX).into_iter().map(|t| (t, self.eng.gpu.crop(&tex, t.x - b.x, t.y - b.y, t.w, t.h))).collect()
        };
        for (t, tex) in pieces {
            let img = self.register(tex);
            scene.draw_image(&ImageBrush::new(img).with_alpha(alpha), Affine::translate((t.x as f64, t.y as f64)));
        }
    }

    /// The node blurred, as a premultiplied texture covering `b`.
    fn blurred(&mut self, n: &Node, xf: Affine, b: IRect) -> Result<wgpu::Texture> {
        let sigma = n.blur * scale_of(xf);
        let c = self.read_rect(b, (sigma * 3.0).ceil() as u32);
        let scene = self.scene(|p, scene| {
            if !needs_pass(n) {
                return p.encode_plain(scene, n, xf, irect(c), false);
            }
            if let Some(pb) = p.bounds(n, xf, false)?.and_then(|r| intersect(r, irect(c))).and_then(round_out) {
                let tex = p.pass(n, xf, pb)?;
                p.draw_texture(scene, tex, pb, 1.0);
            }
            Ok(())
        })?;
        let input = self.render_region(&scene, c)?;
        self.offscreen_passes += 1;
        let out = self.eng.gpu.blur(&input, c, b, sigma as f32);
        self.dump(n, "blur-in", &input, false)?;
        self.dump(n, "blur-out", &out, true)?;
        Ok(out)
    }

    /// A node whose kind needs its own GPU pass, as a premultiplied texture covering `b`.
    fn pass(&mut self, n: &Node, xf: Affine, b: IRect) -> Result<wgpu::Texture> {
        match &n.kind {
            Kind::Effect { .. } => self.effect(n, xf, b),
            Kind::Mask { shape, invert, luminance, children } => {
                let content = self.scene(|p, s| children.iter().try_for_each(|c| p.encode(s, c, xf, irect(b))))?;
                let gate = self.scene(|p, s| shape.iter().try_for_each(|c| p.encode(s, c, xf, irect(b))))?;
                let content = self.render_region(&content, b)?;
                let gate = self.render_region(&gate, b)?;
                let mode = match (*luminance, *invert) {
                    (true, _) => MaskMode::Luminance,
                    (false, true) => MaskMode::Invert,
                    (false, false) => MaskMode::Alpha,
                };
                self.offscreen_passes += 1;
                let out = self.eng.gpu.mask(&content, &gate, mode);
                self.dump(n, "mask-content", &content, false)?;
                self.dump(n, "mask-shape", &gate, false)?;
                self.dump(n, "mask-out", &out, true)?;
                Ok(out)
            }
            Kind::Sdf { range, fill, children } => {
                // Contours up to `range` outside the output still count, so render that much more.
                let range_px = range * scale_of(xf);
                let c = self.read_rect(b, range_px.ceil() as u32 + 1);
                let scene = self.field_scene(children, xf, c)?;
                let input = self.render_region(&scene, c)?;
                let color = [fill[0] * fill[3], fill[1] * fill[3], fill[2] * fill[3], fill[3]];
                self.offscreen_passes += 1;
                let out = self.eng.gpu.sdf(&input, c, b, range_px as f32, color);
                self.dump(n, "sdf-in", &input, false)?;
                self.dump(n, "sdf-out", &out, true)?;
                Ok(out)
            }
            _ => unreachable!("only effects, distance fields and masks get their own pass"),
        }
    }

    /// The scene a distance field is computed from: `children`, with curves finely flattened.
    fn field_scene(&mut self, children: &[Node], xf: Affine, r: IRect) -> Result<Scene> {
        let outer = std::mem::replace(&mut self.fine, true);
        let scene = self.scene(|p, s| children.iter().try_for_each(|child| p.encode(s, child, xf, irect(r))));
        self.fine = outer;
        scene
    }

    /// The effect applied to the node's children, as a premultiplied texture covering `b`.
    fn effect(&mut self, n: &Node, xf: Affine, b: IRect) -> Result<wgpu::Texture> {
        let Kind::Effect { effect, uniforms, inputs, children, .. } = &n.kind else { unreachable!() };
        let def = self.eng.effects.get(effect)?.clone();
        let c = self.read_rect(b, def.reach_px(uniforms, self.scale).ceil() as u32);
        let scene = self.scene(|p, s| children.iter().try_for_each(|child| p.encode(s, child, xf, irect(c))))?;
        let input = self.render_region(&scene, c)?;
        // Field inputs cover the same rect as the children, so `body(p)` reads wherever `src(p)` does.
        let (mut fields, mut ranges) = (Vec::new(), Vec::new());
        for (name, node) in inputs {
            let Kind::Sdf { range, children, .. } = &node.kind else { unreachable!("inputs are checked to be Sdf elements") };
            let range_px = range * scale_of(xf);
            // Contours up to `range` outside the rect still count, as for a drawn Sdf.
            let fc = self.read_rect(c, range_px.ceil() as u32 + 1);
            let scene = self.field_scene(children, xf, fc)?;
            let shape = self.render_region(&scene, fc)?;
            self.offscreen_passes += 1;
            let field = self.eng.gpu.sdf_field(&shape, fc, c, range_px as f32);
            self.dump(node, &format!("{effect}-{name}-in"), &shape, false)?;
            if self.eng.dump_dir.is_some() {
                let preview = self.eng.gpu.field_preview(&field, range_px as f32);
                self.dump(node, &format!("{effect}-{name}-field"), &preview, false)?;
            }
            fields.push(field);
            ranges.push(range_px as f32);
        }
        let packed = def.pack(uniforms, &ranges, self.scale);
        self.offscreen_passes += 1;
        let out = self.eng.gpu.effect_pass(&def, &packed, &input, &fields, c, b, self.resolution, self.scale as f32);
        self.dump(n, &format!("{effect}-in"), &input, false)?;
        self.dump(n, &format!("{effect}-out"), &out, true)?;
        Ok(out)
    }

    /// Draws a node that needs no pixels of its own. With `composite`, its
    /// opacity and blend are applied; without, the caller applies them later.
    fn encode_plain(&mut self, scene: &mut Scene, n: &Node, xf: Affine, roi: Rect, composite: bool) -> Result<()> {
        // Opacity fades the node's composited result. Multiplying the paint's
        // alpha gives the same pixels when the node is a single paint drawn once
        // (one fill or one stroke, or an image), so those skip the layer; text
        // (glyphs can overlap) and fill + stroke need one.
        let single_paint = match &n.kind {
            Kind::Shape { fill, stroke, .. } => fill.is_none() || stroke.is_none(),
            Kind::Image { .. } => true,
            _ => false,
        };
        let layer_alpha = if composite && !single_paint { n.opacity } else { 1.0 };
        let paint_alpha = if composite && single_paint { n.opacity } else { 1.0 };
        let blend = if composite { n.blend } else { Blend::Normal };
        let layered = !blend.is_normal() || layer_alpha < 1.0;
        if layered {
            // A tight clip keeps thousands of small blended shapes cheap.
            let Some(clip) = self.bounds(n, xf, false)?.and_then(|r| intersect(r, roi)) else { return Ok(()) };
            scene.push_layer(Fill::NonZero, mix(blend), layer_alpha, Affine::IDENTITY, &clip.expand());
        }
        let r = self.encode_kind(scene, n, xf, roi, paint_alpha);
        if layered {
            scene.pop_layer();
        }
        r.map_err(|e| if n.at.is_empty() || e.to_string().contains(".luau:") { e } else { anyhow!("{}: {e}", n.at) })
    }

    fn encode_kind(&mut self, scene: &mut Scene, n: &Node, xf: Affine, roi: Rect, alpha: f32) -> Result<()> {
        match &n.kind {
            Kind::Group { transform, clip, children } => {
                let xf = xf * Affine::new(*transform);
                let mut roi = roi;
                if let Some([x, y, w, h]) = *clip {
                    let rect = Rect::new(x, y, x + w, y + h);
                    let Some(r) = intersect(roi, (xf * rect.to_path(0.1)).bounding_box()) else { return Ok(()) };
                    roi = r;
                    scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, xf, &rect);
                }
                for c in children {
                    self.encode(scene, c, xf, roi)?;
                }
                if clip.is_some() {
                    scene.pop_layer();
                }
            }
            Kind::Mask { shape, invert, luminance, children } => {
                // Both layers cover only what the mask can draw. A DestIn/DestOut
                // layer is a blend level in every tile it covers, and nested past
                // vello's 4 in-register levels each tile spills to a small fixed
                // buffer, so a canvas-sized one overflows it on a large render.
                let Some(clip) = self.bounds(n, xf, false)?.and_then(|r| intersect(r, roi)) else { return Ok(()) };
                let clip = clip.expand();
                scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, Affine::IDENTITY, &clip);
                for c in children {
                    self.encode(scene, c, xf, roi)?;
                }
                debug_assert!(!luminance, "luminance masks take the pass path");
                let compose = if *invert { Compose::DestOut } else { Compose::DestIn };
                scene.push_layer(Fill::NonZero, BlendMode::new(Mix::Normal, compose), 1.0, Affine::IDENTITY, &clip);
                for c in shape {
                    self.encode(scene, c, xf, roi)?;
                }
                scene.pop_layer();
                scene.pop_layer();
            }
            Kind::Shape { geom, fill, stroke, even_odd } => {
                let mut path = geom_path(geom)?;
                if self.fine {
                    path = flattened(&path, FIELD_TOLERANCE / scale_of(xf));
                }
                if let Some(p) = fill {
                    let rule = if *even_odd { Fill::EvenOdd } else { Fill::NonZero };
                    scene.fill(rule, xf, &brush(p, alpha), None, &path);
                }
                if let Some(s) = stroke {
                    scene.stroke(&kurbo_stroke(s), xf, &brush(&s.paint, alpha), None, &path);
                }
            }
            Kind::Text(t) if self.fine => {
                let path = self.eng.fonts.outline(t)?;
                scene.fill(Fill::NonZero, xf, &brush(&t.fill, alpha), None, &flattened(&path, FIELD_TOLERANCE / scale_of(xf)));
            }
            Kind::Text(t) => {
                let layout = self.eng.fonts.layout(t)?;
                // Offset the glyphs rather than the transform, so gradients stay in the
                // same coordinates as every other paint.
                let (ox, oy) = Fonts::origin(t, &layout);
                let (ox, oy) = (ox as f32, oy as f32);
                let b = brush(&t.fill, alpha);
                for line in layout.lines() {
                    for item in line.items() {
                        let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                        let r = run.run();
                        let coords: Vec<i16> = r.normalized_coords().iter().map(|c| c.to_bits()).collect();
                        let skew = r.synthesis().skew().map(|deg| Affine::skew((deg as f64).to_radians().tan(), 0.0));
                        scene
                            .draw_glyphs(r.font())
                            .font_size(r.font_size())
                            .transform(xf)
                            .glyph_transform(skew)
                            .normalized_coords(&coords)
                            .brush(&b)
                            .hint(false)
                            .draw(Fill::NonZero, run.positioned_glyphs().map(|g| vello::Glyph { id: g.id, x: g.x + ox, y: g.y + oy }));
                    }
                }
            }
            Kind::Image { src, dst, crop, fit } => {
                let (img, px_per_unit) = self.eng.image(src)?;
                let [dx, dy, dw, dh] = *dst;
                let [cx, cy, cw, ch] = crop.map(|v| v * px_per_unit);
                let (mut sx, mut sy, mut sw, mut sh) = (cx, cy, cw, ch);
                let (mut tx, mut ty, mut tw, mut th) = (dx, dy, dw, dh);
                match fit {
                    Fit::Cover => {
                        let k = (dw / cw).max(dh / ch);
                        (sw, sh) = (dw / k, dh / k);
                        (sx, sy) = (cx + (cw - sw) / 2.0, cy + (ch - sh) / 2.0);
                    }
                    Fit::Contain => {
                        let k = (dw / cw).min(dh / ch);
                        (tw, th) = (cw * k, ch * k);
                        (tx, ty) = (dx + (dw - tw) / 2.0, dy + (dh - th) / 2.0);
                    }
                    Fit::Fill => {}
                }
                let mut place = xf
                    * Affine::translate((tx, ty))
                    * Affine::scale_non_uniform(tw / sw, th / sh)
                    * Affine::translate((-sx, -sy));
                let mut cropped = sx > 0.0 || sy > 0.0 || sx + sw < img.width as f64 || sy + sh < img.height as f64;
                let img = if img.width > ATLAS_MAX || img.height > ATLAS_MAX {
                    let (part, at) = atlas_fit(&img, [sx, sy, sw, sh], max_scale_of(place))?;
                    place *= at;
                    cropped = true;
                    part
                } else {
                    img
                };
                let img = self.fresh(img);
                if cropped {
                    scene.push_clip_layer(Fill::NonZero, xf, &Rect::new(tx, ty, tx + tw, ty + th));
                }
                let quality = if scale_of(place) < 0.75 { peniko::ImageQuality::High } else { peniko::ImageQuality::Medium };
                scene.draw_image(&ImageBrush::new(img).with_alpha(alpha).with_quality(quality), place);
                if cropped {
                    scene.pop_layer();
                }
            }
            Kind::Effect { .. } | Kind::Sdf { .. } => unreachable!("effects and distance fields are always offscreen"),
        }
        Ok(())
    }

    /// Device-space bounds of what a node draws. `own_blur` includes the node's own blur spread.
    fn bounds(&mut self, n: &Node, xf: Affine, own_blur: bool) -> Result<Option<Rect>> {
        let union = |this: &mut Self, nodes: &[Node], xf: Affine| -> Result<Option<Rect>> {
            let mut acc: Option<Rect> = None;
            for c in nodes {
                if let Some(r) = this.bounds(c, xf, true)? {
                    acc = Some(acc.map_or(r, |a| a.union(r)));
                }
            }
            Ok(acc)
        };
        let r = match &n.kind {
            Kind::Group { transform, clip, children } => {
                let xf = xf * Affine::new(*transform);
                let r = union(self, children, xf)?;
                match (r, clip) {
                    (Some(r), Some([x, y, w, h])) => intersect(r, (xf * Rect::new(*x, *y, x + w, y + h).to_path(0.1)).bounding_box()),
                    (r, _) => r,
                }
            }
            Kind::Mask { shape, invert, luminance, children } => {
                let r = union(self, children, xf)?;
                if *invert || *luminance {
                    r
                } else {
                    match (r, union(self, shape, xf)?) {
                        (Some(a), Some(b)) => intersect(a, b),
                        _ => None,
                    }
                }
            }
            Kind::Shape { geom, stroke, .. } => {
                let path = geom_path(geom)?;
                let r = (xf * path).bounding_box();
                let grow = stroke.as_ref().map_or(0.0, |s| s.width * max_scale_of(xf) * 0.75);
                Some(r.inflate(grow + 1.0, grow + 1.0))
            }
            Kind::Text(t) => {
                let layout = self.eng.fonts.layout(t).map_err(|e| if n.at.is_empty() { e } else { anyhow!("{}: Text: {e:#}", n.at) })?;
                Some((xf * Fonts::bounds(t, &layout).to_path(0.1)).bounding_box())
            }
            Kind::Image { dst: [x, y, w, h], .. } => Some((xf * Rect::new(*x, *y, x + w, y + h).to_path(0.1)).bounding_box()),
            Kind::Effect { area: Some([x, y, w, h]), .. } => Some((xf * Rect::new(*x, *y, x + w, y + h).to_path(0.1)).bounding_box()),
            // The children plus what each input's field is computed from (not the
            // field's own spread: that is data, drawn only where the effect says).
            Kind::Effect { effect, uniforms, inputs, children, area: None } => {
                let grow = self.eng.effects.get(effect)?.grow_px(uniforms, self.scale);
                let mut r = union(self, children, xf)?;
                for (_, node) in inputs {
                    let Kind::Sdf { children, .. } = &node.kind else { unreachable!("inputs are checked to be Sdf elements") };
                    if let Some(i) = union(self, children, xf)? {
                        r = Some(r.map_or(i, |r| r.union(i)));
                    }
                }
                r.map(|r| r.inflate(grow, grow))
            }
            // Beyond `range` outside the children the field is 0, i.e. transparent.
            Kind::Sdf { range, children, .. } => {
                let grow = range * scale_of(xf) + 1.0;
                union(self, children, xf)?.map(|r| r.inflate(grow, grow))
            }
        };
        let spread = if own_blur { n.blur * scale_of(xf) * 3.0 } else { 0.0 };
        Ok(r.map(|r| r.inflate(spread, spread)))
    }
}
