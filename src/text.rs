//! Fonts and paragraph layout (parley), plus glyph outlines (skrifa).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use parley::style::{FontFamily, FontFeature, FontStyle, FontVariation, FontWeight, LineHeight, StyleProperty};
use parley::{Alignment, AlignmentOptions, FontContext, Layout, LayoutContext, PositionedLayoutItem};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use vello::kurbo::{Affine, BezPath, Rect, Shape};
use vello::peniko::Blob;

use crate::ir::{Align, Anchor, Font, Text};

pub struct Fonts {
    fcx: FontContext,
    lcx: LayoutContext<()>,
    /// Font files registered by path, mapped to their family name.
    files: HashMap<PathBuf, String>,
}

/// Trailing words in names like "Courier New Bold" that name a face, not a
/// family. Only used to suggest `font = "Courier New", weight = 700` when such
/// a name isn't found.
const FACE_WORDS: &[(&str, Option<f32>, bool)] = &[
    ("thin", Some(100.0), false),
    ("extralight", Some(200.0), false),
    ("light", Some(300.0), false),
    ("regular", None, false),
    ("medium", Some(500.0), false),
    ("semibold", Some(600.0), false),
    ("bold", Some(700.0), false),
    ("extrabold", Some(800.0), false),
    ("black", Some(900.0), false),
    ("heavy", Some(900.0), false),
    ("italic", None, true),
    ("oblique", None, true),
];

impl Fonts {
    pub fn new() -> Fonts {
        Fonts { fcx: FontContext::new(), lcx: LayoutContext::new(), files: HashMap::new() }
    }

    /// The family name to lay out with: a system family as given, or the family in a font file.
    fn resolve(&mut self, font: &Font) -> Result<String> {
        let name = match font {
            Font::File(path) => {
                let path = PathBuf::from(path);
                if let Some(f) = self.files.get(&path) {
                    return Ok(f.clone());
                }
                let data = std::fs::read(&path).with_context(|| format!("font file {}", path.display()))?;
                let fams = self.fcx.collection.register_fonts(Blob::new(Arc::new(data)), None);
                let Some((id, _)) = fams.first() else { bail!("no fonts in {}", path.display()) };
                let family = self.fcx.collection.family_name(*id).unwrap_or_default().to_string();
                self.files.insert(path, family.clone());
                return Ok(family);
            }
            Font::Family(name) => name,
        };
        if self.fcx.collection.family_by_name(name).is_some() {
            return Ok(name.clone());
        }
        // "DIN Condensed Bold" is family "DIN Condensed" at weight 700: say so.
        let mut words: Vec<&str> = name.split_whitespace().collect();
        let (mut weight, mut italic) = (None, false);
        while let Some(last) = words.last().map(|w| w.to_lowercase().replace('-', "")) {
            let Some((_, w, i)) = FACE_WORDS.iter().find(|(f, ..)| *f == last) else { break };
            weight = weight.or(*w);
            italic |= *i;
            words.pop();
            let family = words.join(" ");
            if !family.is_empty() && self.fcx.collection.family_by_name(&family).is_some() {
                let mut fix = format!("font = {family:?}");
                if let Some(w) = weight {
                    fix += &format!(", weight = {w}");
                }
                if italic {
                    fix += ", italic = true";
                }
                bail!("font {name:?} not found: the family is {family:?} (use {fix})");
            }
        }
        let want = name.to_lowercase();
        let first = want.split_whitespace().next().unwrap_or("");
        let mut close: Vec<String> =
            self.fcx.collection.family_names().filter(|f| f.to_lowercase().contains(first)).map(String::from).collect();
        close.sort();
        close.truncate(8);
        bail!("font {name:?} not found{}", if close.is_empty() { String::new() } else { format!(" (similar: {})", close.join(", ")) })
    }

    /// Lays out a text node. Coordinates are relative to the text box's top-left.
    pub fn layout(&mut self, t: &Text) -> Result<Layout<()>> {
        let family = self.resolve(&t.font)?;
        let features: Vec<FontFeature> =
            t.features.iter().map(|(tag, v)| FontFeature::new(tag_of(tag), *v)).collect();
        let variations: Vec<FontVariation> =
            t.variations.iter().map(|(tag, v)| FontVariation::new(tag_of(tag), *v)).collect();
        let mut b = self.lcx.ranged_builder(&mut self.fcx, &t.text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(family.as_str().into())));
        b.push_default(StyleProperty::FontSize(t.size as f32));
        if let Some(w) = t.weight {
            b.push_default(StyleProperty::FontWeight(FontWeight::new(w)));
        }
        if t.italic {
            b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
        }
        b.push_default(StyleProperty::LetterSpacing(t.tracking * t.size as f32));
        b.push_default(StyleProperty::LineHeight(match t.leading {
            Some(l) => LineHeight::FontSizeRelative(l),
            None => LineHeight::MetricsRelative(1.0),
        }));
        if !features.is_empty() {
            b.push_default(StyleProperty::FontFeatures(features.as_slice().into()));
        }
        if !variations.is_empty() {
            b.push_default(StyleProperty::FontVariations(variations.as_slice().into()));
        }
        let mut layout = b.build(&t.text);
        layout.break_all_lines(t.w.map(|w| w as f32));
        let align = match (t.w, t.align, t.anchor) {
            (Some(_), Align::Left, _) | (None, _, Anchor::Start) => Alignment::Left,
            (Some(_), Align::Center, _) | (None, _, Anchor::Middle) => Alignment::Center,
            (Some(_), Align::Right, _) | (None, _, Anchor::End) => Alignment::Right,
            (Some(_), Align::Justify, _) => Alignment::Justify,
        };
        // Without a wrap width, lines align within the widest line, the way the anchor places the block.
        layout.align(align, AlignmentOptions::default());
        Ok(layout)
    }

    /// Where the layout box's top-left goes: with `w`, `x` is the box's left
    /// edge; without, the block's start, middle or end per `anchor`. `y` is the
    /// first line's baseline.
    pub fn origin(t: &Text, layout: &Layout<()>) -> (f64, f64) {
        let w = layout.width() as f64;
        let x = match (t.w, t.anchor) {
            (None, Anchor::Middle) => t.x - w / 2.0,
            (None, Anchor::End) => t.x - w,
            _ => t.x,
        };
        let baseline = layout.lines().next().map_or(0.0, |l| l.metrics().baseline as f64);
        (x, t.y - baseline)
    }

    pub fn bounds(t: &Text, layout: &Layout<()>) -> Rect {
        let (x, y) = Fonts::origin(t, layout);
        let w = t.w.unwrap_or(layout.width() as f64);
        // Glyphs can overhang their advance box (italics, accents, swashes).
        Rect::new(x, y, x + w, y + layout.height() as f64).inflate(t.size * 0.5, t.size * 0.5)
    }

    /// Where a text node's lines and glyphs land, in doc px (`measure` in scripts).
    pub fn measure(&mut self, t: &Text) -> Result<Metrics> {
        let layout = self.layout(t)?;
        let (ox, oy) = Fonts::origin(t, &layout);
        let mut lines = Vec::new();
        for line in layout.lines() {
            let m = line.metrics();
            // The space a line wraps at hangs and isn't counted; the text's own trailing
            // spaces are, just as `anchor` places them.
            let w = (m.advance - m.hanging_advance) as f64;
            lines.push(LineMetrics { x: ox + m.offset as f64, w, baseline: oy + m.baseline as f64 });
        }
        let (x0, x1) = lines.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), l| (a.min(l.x), b.max(l.x + l.w)));
        let (x0, x1) = if lines.is_empty() { (ox, ox) } else { (x0, x1) };
        let first = layout.lines().next().map(|l| *l.metrics());
        let ink = self.outline(t)?;
        Ok(Metrics {
            advance: Rect::new(x0, oy, x1, oy + layout.height() as f64),
            baseline: t.y,
            ascent: first.map_or(0.0, |m| (m.baseline - m.content_block_min_coord) as f64),
            descent: first.map_or(0.0, |m| (m.content_block_max_coord - m.baseline) as f64),
            ink: (!ink.elements().is_empty()).then(|| ink.bounding_box()),
            lines,
        })
    }

    /// Glyph outlines of a laid-out text node as one path, in doc px.
    pub fn outline(&mut self, t: &Text) -> Result<BezPath> {
        let layout = self.layout(t)?;
        let (ox, oy) = Fonts::origin(t, &layout);
        let mut out = BezPath::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else { continue };
                let r = run.run();
                let font = r.font();
                let face = skrifa::FontRef::from_index(font.data.as_ref(), font.index)?;
                let outlines = face.outline_glyphs();
                let coords: Vec<skrifa::instance::NormalizedCoord> =
                    r.normalized_coords().iter().map(|c| skrifa::instance::NormalizedCoord::from_bits(c.to_bits())).collect();
                let size = Size::new(r.font_size());
                for g in run.positioned_glyphs() {
                    let Some(glyph) = outlines.get(skrifa::GlyphId::new(g.id)) else { continue };
                    let mut pen = PathPen { path: BezPath::new() };
                    glyph.draw(DrawSettings::unhinted(size, LocationRef::new(&coords)), &mut pen)?;
                    // Font outlines are y-up.
                    let xf = Affine::translate((ox + g.x as f64, oy + g.y as f64)) * Affine::FLIP_Y;
                    out.extend(xf * pen.path);
                }
            }
        }
        Ok(out)
    }
}

pub struct Metrics {
    /// The lines' advance box: from the leftmost line start to the rightmost line
    /// end, and from the top of the first line box to the bottom of the last.
    pub advance: Rect,
    /// The first line's baseline (the text's `y`).
    pub baseline: f64,
    /// The first line's font ascent and descent, both positive.
    pub ascent: f64,
    pub descent: f64,
    pub lines: Vec<LineMetrics>,
    /// Tight bounds of the glyph outlines; None when nothing is inked (e.g. only spaces).
    pub ink: Option<Rect>,
}

pub struct LineMetrics {
    pub x: f64,
    pub w: f64,
    pub baseline: f64,
}

fn tag_of(s: &str) -> parlance::Tag {
    let mut b = [b' '; 4];
    for (i, c) in s.bytes().take(4).enumerate() {
        b[i] = c;
    }
    parlance::Tag::new(&b)
}

struct PathPen {
    path: BezPath,
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}
