//! The scene IR: what a script evaluates to. Components are already expanded,
//! lengths are resolved to doc px, colors to straight RGBA. It serializes to
//! JSON, which doubles as a cache key and a diff target (`--emit-ir`).

use serde::Serialize;

use crate::units::Rgba;

#[derive(Serialize, Clone, Debug)]
pub struct Document {
    pub size: [f64; 2],
    pub bg: Rgba,
    /// Doc px beyond the canvas that blur, effects and distance fields can read
    /// (None: the canvas's larger side).
    pub margin: Option<f64>,
    pub children: Vec<Node>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Node {
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(skip_serializing_if = "is_one")]
    pub opacity: f32,
    #[serde(skip_serializing_if = "Blend::is_normal")]
    pub blend: Blend,
    /// Gaussian sigma in doc px, applied before opacity and blend.
    #[serde(skip_serializing_if = "is_zero")]
    pub blur: f64,
    /// Where in the script this node was made ("poster.luau:12"), for errors.
    #[serde(skip)]
    pub at: String,
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "op")]
pub enum Kind {
    Group {
        /// Affine coefficients [a b c d e f] (kurbo order).
        transform: [f64; 6],
        #[serde(skip_serializing_if = "Option::is_none")]
        clip: Option<[f64; 4]>,
        children: Vec<Node>,
    },
    Mask {
        shape: Vec<Node>,
        invert: bool,
        luminance: bool,
        children: Vec<Node>,
    },
    Shape {
        geom: Geom,
        #[serde(skip_serializing_if = "Option::is_none")]
        fill: Option<Paint>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
        /// Fill by the even-odd rule (overlaps and inner contours cut holes) instead of nonzero winding.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        even_odd: bool,
    },
    Text(Text),
    Image {
        src: ImageSrc,
        /// Destination rect in doc px.
        dst: [f64; 4],
        /// Source rect in the image's own units (image px for files, doc px for rasters).
        crop: [f64; 4],
        fit: Fit,
    },
    Effect {
        effect: String,
        uniforms: Vec<(String, UValue)>,
        /// Where the effect draws, in local doc px. None = the children's bounds plus pad.
        #[serde(skip_serializing_if = "Option::is_none")]
        area: Option<[f64; 4]>,
        /// Filled input slots, in the effect's input order: (name, an `Sdf` node).
        #[serde(skip_serializing_if = "Vec::is_empty")]
        inputs: Vec<(String, Node)>,
        children: Vec<Node>,
    },
    /// Signed distance to the children's alpha = 0.5 contour, drawn as `fill`
    /// at alpha 0.5 + d / (2 range): 1 deep inside, 0.5 on the edge, 0 far outside.
    Sdf {
        /// Doc px.
        range: f64,
        fill: Rgba,
        children: Vec<Node>,
    },
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum Geom {
    Rect { x: f64, y: f64, w: f64, h: f64, r: f64 },
    Circle { cx: f64, cy: f64, r: f64 },
    Ellipse { cx: f64, cy: f64, rx: f64, ry: f64, rotate: f64 },
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    /// SVG path data under an affine transform.
    Path { d: String, transform: [f64; 6] },
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Paint {
    Solid { color: Rgba },
    Linear { from: [f64; 2], to: [f64; 2], stops: Vec<(f32, Rgba)> },
    Radial { center: [f64; 2], r: f64, stops: Vec<(f32, Rgba)> },
}

#[derive(Serialize, Clone, Debug)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

#[derive(Serialize, Clone, Debug)]
pub struct Text {
    pub text: String,
    pub x: f64,
    pub y: f64,
    /// Wrap width. With it, `align` places lines in [x, x + w]; without it,
    /// `anchor` places the unboxed block at `x` (and aligns its lines the same way).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    pub font: Font,
    pub size: f64,
    pub fill: Paint,
    pub align: Align,
    pub anchor: Anchor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leading: Option<f32>,
    pub tracking: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,
    pub italic: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<(String, u16)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<(String, f32)>,
}

/// A system font family, or a font file (resolved path).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Font {
    Family(String),
    File(String),
}

/// Where unboxed text sits relative to `x`.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ImageSrc {
    /// Absolute path. The mtime goes into cache keys, not the IR.
    File(String),
    /// Content key of a raster made by `rasterize`.
    Raster(String),
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Fill,
    Cover,
    Contain,
}

/// An effect uniform value. Lengths stay in doc px until render time.
#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum UValue {
    Num(f64),
    Color(Rgba),
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    Plus,
}

impl Blend {
    pub const NAMES: &[(&str, Blend)] = &[
        ("normal", Blend::Normal),
        ("multiply", Blend::Multiply),
        ("screen", Blend::Screen),
        ("overlay", Blend::Overlay),
        ("darken", Blend::Darken),
        ("lighten", Blend::Lighten),
        ("color-dodge", Blend::ColorDodge),
        ("color-burn", Blend::ColorBurn),
        ("hard-light", Blend::HardLight),
        ("soft-light", Blend::SoftLight),
        ("difference", Blend::Difference),
        ("exclusion", Blend::Exclusion),
        ("hue", Blend::Hue),
        ("saturation", Blend::Saturation),
        ("color", Blend::Color),
        ("luminosity", Blend::Luminosity),
        ("plus", Blend::Plus),
    ];

    pub fn is_normal(&self) -> bool {
        *self == Blend::Normal
    }
}

impl Node {
    /// Visits every node, including mask shapes and effect inputs.
    pub fn walk(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        match &self.kind {
            Kind::Group { children, .. } | Kind::Sdf { children, .. } => children.iter().for_each(|c| c.walk(f)),
            Kind::Effect { inputs, children, .. } => {
                inputs.iter().map(|(_, n)| n).chain(children).for_each(|c| c.walk(f))
            }
            Kind::Mask { shape, children, .. } => {
                shape.iter().chain(children).for_each(|c| c.walk(f))
            }
            _ => {}
        }
    }
}
