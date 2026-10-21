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
use crate::gpu::{IRect, MaskMode};
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
    overrides: Vec<ImageData>,
    pub offscreen_passes: usize,
    dumped: usize,
    /// Per scene being built (innermost last): source blob id -> image with this scene's id.
    scene_images: Vec<HashMap<u64, ImageData>>,
}

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
    let mut pass = Pass {
        eng,
        fine: false,
        scale,
        resolution: [canvas.w as f32, canvas.h as f32],
        overrides: Vec::new(),
        offscreen_passes: 0,
        dumped: 0,
        scene_images: Vec::new(),
    };
    let xf = Affine::scale(scale);
    let roi = irect(region);
    let result = pass
        .scene(|p, scene| {
            if doc.bg[3] > 0.0 {
                scene.fill(Fill::NonZero, Affine::IDENTITY, to_color(doc.bg), None, &irect(canvas));
            }
            doc.children.iter().try_for_each(|n| p.encode(scene, n, xf, roi))
        })
        .and_then(|scene| pass.render_region(&scene, region));
    let passes = pass.offscreen_passes;
    pass.finish();
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

impl Pass<'_> {
    /// Unregisters the offscreen textures handed to vello during this pass.
    fn finish(self) {
        for img in &self.overrides {
            self.eng.gpu.vello.override_image(img, None);
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

    fn render_region(&mut self, scene: &Scene, r: IRect) -> Result<wgpu::Texture> {
        let mut local = Scene::new();
        local.append(scene, Some(Affine::translate((-r.x as f64, -r.y as f64))));
        self.eng.gpu.render_scene(&local, r.w, r.h)
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
        let img = self.register(tex);
        let brush = ImageBrush::new(img).with_alpha(n.opacity);
        let at = Affine::translate((b.x as f64, b.y as f64));
        if n.blend.is_normal() {
            scene.draw_image(&brush, at);
        } else {
            scene.push_layer(Fill::NonZero, mix(n.blend), 1.0, Affine::IDENTITY, &irect(b));
            scene.draw_image(&brush, at);
            scene.pop_layer();
        }
        Ok(())
    }

    /// The node blurred, as a premultiplied texture covering `b`.
    fn blurred(&mut self, n: &Node, xf: Affine, b: IRect) -> Result<wgpu::Texture> {
        let sigma = n.blur * scale_of(xf);
        let c = b.expand((sigma * 3.0).ceil() as u32);
        let scene = self.scene(|p, scene| {
            if !needs_pass(n) {
                return p.encode_plain(scene, n, xf, irect(c), false);
            }
            if let Some(pb) = p.bounds(n, xf, false)?.and_then(|r| intersect(r, irect(c))).and_then(round_out) {
                let tex = p.pass(n, xf, pb)?;
                let img = p.register(tex);
                scene.draw_image(&ImageBrush::new(img), Affine::translate((pb.x as f64, pb.y as f64)));
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
                let c = b.expand(range_px.ceil() as u32 + 1);
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
        let c = b.expand(def.reach_px(uniforms, self.scale).ceil() as u32);
        let scene = self.scene(|p, s| children.iter().try_for_each(|child| p.encode(s, child, xf, irect(c))))?;
        let input = self.render_region(&scene, c)?;
        // Field inputs cover the same rect as the children, so `body(p)` reads wherever `src(p)` does.
        let (mut fields, mut ranges) = (Vec::new(), Vec::new());
        for (name, node) in inputs {
            let Kind::Sdf { range, children, .. } = &node.kind else { unreachable!("inputs are checked to be Sdf elements") };
            let range_px = range * scale_of(xf);
            // Contours up to `range` outside the rect still count, as for a drawn Sdf.
            let fc = c.expand(range_px.ceil() as u32 + 1);
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
                let full = roi.expand();
                scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, Affine::IDENTITY, &full);
                for c in children {
                    self.encode(scene, c, xf, roi)?;
                }
                debug_assert!(!luminance, "luminance masks take the pass path");
                let compose = if *invert { Compose::DestOut } else { Compose::DestIn };
                scene.push_layer(Fill::NonZero, BlendMode::new(Mix::Normal, compose), 1.0, Affine::IDENTITY, &full);
                for c in shape {
                    self.encode(scene, c, xf, roi)?;
                }
                scene.pop_layer();
                scene.pop_layer();
            }
            Kind::Shape { geom, fill, stroke } => {
                let mut path = geom_path(geom)?;
                if self.fine {
                    path = flattened(&path, FIELD_TOLERANCE / scale_of(xf));
                }
                if let Some(p) = fill {
                    scene.fill(Fill::NonZero, xf, &brush(p, alpha), None, &path);
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
                let img = self.fresh(img);
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
                let place = xf
                    * Affine::translate((tx, ty))
                    * Affine::scale_non_uniform(tw / sw, th / sh)
                    * Affine::translate((-sx, -sy));
                let cropped = sx > 0.0 || sy > 0.0 || sx + sw < img.width as f64 || sy + sh < img.height as f64;
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
                let grow = stroke.as_ref().map_or(0.0, |s| s.width * scale_of(xf) * 0.75);
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
