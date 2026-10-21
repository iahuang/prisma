//! Effects: per-pixel WGSL functions over their rendered children.
//!
//! A script defines one with `effect { name, uniforms, inputs, reach, grow, wgsl }`. The WGSL
//! must define `fn effect(p: vec2f) -> vec4f`, where `p` is the absolute device
//! pixel (center) and the result is premultiplied RGBA. The user's code goes
//! first in the generated module so naga's line numbers match the script's
//! string; WGSL doesn't care about declaration order.
//!
//! Inputs are extra named slots filled at the call site (`body = Sdf { ... }`).
//! Each one is computed offscreen into a float texture and bound directly, so it
//! never passes through 8-bit compositing; the prelude generates an accessor
//! per input (`body(p)`, `body_march(p, dir, maxd)`).

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use crate::ir::UValue;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UType {
    Float,
    /// Doc px in the script, device px in the shader, so effects look the same at any scale.
    Length,
    /// Degrees in the script, radians in the shader.
    Angle,
    /// A color string in the script, premultiplied vec4f in the shader.
    Color,
}

impl UType {
    pub fn parse(s: &str) -> Result<UType> {
        Ok(match s {
            "float" => UType::Float,
            "length" => UType::Length,
            "angle" => UType::Angle,
            "color" => UType::Color,
            _ => bail!("unknown uniform type {s:?} (use float, length, angle or color)"),
        })
    }
}

/// What an effect input slot holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputKind {
    /// A signed distance field, from an `Sdf { ... }` element.
    Sdf,
}

impl InputKind {
    pub fn parse(s: &str) -> Result<InputKind> {
        Ok(match s {
            "sdf" => InputKind::Sdf,
            _ => bail!("unknown input kind {s:?} (kinds: sdf)"),
        })
    }

    /// The element a slot of this kind takes.
    pub fn element(self) -> &'static str {
        match self {
            InputKind::Sdf => "Sdf",
        }
    }
}

/// Names the prelude defines, which inputs can't take.
const RESERVED: &[&str] = &["src", "src_doc", "to_doc", "to_px", "pm", "unpm", "luma", "rot", "sdf_dist", "hash", "hash3", "vnoise", "fbm", "effect", "sys", "u"];

/// An effect's `reach` or `grow`: a fixed length or a length uniform's value.
#[derive(Clone, Debug)]
pub enum Margin {
    /// Doc px.
    Fixed(f64),
    /// The value of a length uniform.
    Uniform(String),
}

#[derive(Clone, Debug)]
pub struct EffectDef {
    pub name: String,
    /// Sorted by name; colors are laid out first in the uniform struct.
    pub uniforms: Vec<(String, UType, UValue)>,
    /// Sorted by name; bound as textures 3, 4, ... in this order.
    pub inputs: Vec<(String, InputKind)>,
    /// How far beyond what it draws the shader reads `src` and its inputs.
    pub reach: Margin,
    /// How far beyond its children (and inputs' children) it draws, without `area`.
    pub grow: Margin,
    /// The complete shader module.
    pub module: String,
}

#[derive(Default)]
pub struct Effects {
    pub defs: BTreeMap<String, EffectDef>,
}

impl Effects {
    pub fn define(&mut self, def: EffectDef) -> Result<()> {
        if let Some(old) = self.defs.get(&def.name) {
            if old.module != def.module {
                bail!("effect {:?} is defined twice with different code", def.name);
            }
            return Ok(());
        }
        self.defs.insert(def.name.clone(), def);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&EffectDef> {
        self.defs.get(name).ok_or_else(|| anyhow!("unknown effect {name:?}"))
    }
}

impl EffectDef {
    pub fn new(
        name: String,
        mut uniforms: Vec<(String, UType, UValue)>,
        mut inputs: Vec<(String, InputKind)>,
        reach: Margin,
        grow: Margin,
        wgsl: String,
    ) -> Result<EffectDef> {
        uniforms.sort_by(|a, b| a.0.cmp(&b.0));
        inputs.sort_by(|a, b| a.0.cmp(&b.0));
        for (field, m) in [("reach", &reach), ("grow", &grow)] {
            if let Margin::Uniform(u) = m {
                match uniforms.iter().find(|(n, ..)| n == u) {
                    Some((_, UType::Length, UValue::Num(d))) if *d < 0.0 => {
                        bail!("effect {name:?}: {field} is {u:?}, whose default {d} is negative")
                    }
                    Some((_, UType::Length, _)) => {}
                    _ => bail!("effect {name:?}: {field} names {u:?}, which is not a length uniform"),
                }
            }
        }
        for (n, _) in &inputs {
            if uniforms.iter().any(|(u, ..)| u == n) {
                bail!("effect {name:?}: {n:?} is both a uniform and an input");
            }
            if RESERVED.contains(&n.as_str()) || n.starts_with("prisma_") {
                bail!("effect {name:?}: input name {n:?} is taken by the prelude (reserved: {}, and anything starting with prisma_)", RESERVED.join(", "));
            }
        }
        let module = format!("{wgsl}\n{}", prelude(&uniforms, &inputs));
        validate(&name, &module)?;
        Ok(EffectDef { name, uniforms, inputs, reach, grow, module })
    }

    /// Bytes for the `u` uniform buffer, in the layout `prelude` declares.
    /// `ranges` are the field inputs' ranges in device px, in input order.
    pub fn pack(&self, values: &[(String, UValue)], ranges: &[f32], scale: f64) -> Vec<u8> {
        let mut colors = Vec::new();
        let mut floats = Vec::new();
        for (name, ty, dflt) in &self.uniforms {
            let v = values.iter().find(|(n, _)| n == name).map_or(dflt, |(_, v)| v);
            match (ty, v) {
                (UType::Color, UValue::Color(c)) => colors.extend([c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]),
                (UType::Length, UValue::Num(n)) => floats.push((n * scale) as f32),
                (_, UValue::Num(n)) => floats.push(*n as f32),
                (_, UValue::Color(_)) => floats.push(0.0),
            }
        }
        floats.extend(ranges);
        let mut out: Vec<f32> = colors;
        out.extend(floats);
        out.resize(out.len().div_ceil(4).max(1) * 4, 0.0);
        bytemuck::cast_slice(&out).to_vec()
    }

    /// `reach` in device px. Negative values are rejected when the effect is defined and used.
    pub fn reach_px(&self, values: &[(String, UValue)], scale: f64) -> f64 {
        self.margin_px(&self.reach, values, scale)
    }

    /// `grow` in device px.
    pub fn grow_px(&self, values: &[(String, UValue)], scale: f64) -> f64 {
        self.margin_px(&self.grow, values, scale)
    }

    fn margin_px(&self, m: &Margin, values: &[(String, UValue)], scale: f64) -> f64 {
        match m {
            Margin::Fixed(v) => v * scale,
            Margin::Uniform(u) => {
                let dflt = self.uniforms.iter().find(|(n, ..)| n == u).map(|(.., d)| d);
                match values.iter().find(|(n, _)| n == u).map(|(_, v)| v).or(dflt) {
                    Some(UValue::Num(n)) => n * scale,
                    _ => 0.0,
                }
            }
        }
    }
}

/// System uniforms every effect sees as `sys`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Sys {
    pub resolution: [f32; 2],
    pub px_per_unit: f32,
    pub _pad0: f32,
    pub src_origin: [f32; 2],
    pub src_size: [f32; 2],
    pub dst_origin: [f32; 2],
    pub _pad1: [f32; 2],
}

fn prelude(uniforms: &[(String, UType, UValue)], inputs: &[(String, InputKind)]) -> String {
    let mut fields = String::new();
    for (n, ..) in uniforms.iter().filter(|u| u.1 == UType::Color) {
        fields += &format!("  {n}: vec4f,\n");
    }
    for (n, ..) in uniforms.iter().filter(|u| u.1 != UType::Color) {
        fields += &format!("  {n}: f32,\n");
    }
    for (n, _) in inputs {
        fields += &format!("  prisma_range_{n}: f32,\n");
    }
    if fields.is_empty() {
        fields = "  _unused: vec4f,\n".into();
    }
    let mut accessors = String::new();
    if !inputs.is_empty() {
        accessors += SDF_SAMPLE;
    }
    for (k, (n, kind)) in inputs.iter().enumerate() {
        match kind {
            InputKind::Sdf => accessors += &SDF_ACCESSOR.replace("NAME", n).replace("BINDING", &(3 + k).to_string()),
        }
    }
    format!("// ---- prisma prelude ----\nstruct U {{\n{fields}}}\n{PRELUDE}{accessors}")
}

const SDF_SAMPLE: &str = r#"
// One read of an Sdf input. All lengths are device px, like p.
struct SdfSample {
  d: f32,       // signed distance to the contour, positive inside, exact up to the Sdf's range (clamped there)
  n: vec2f,     // outward unit normal; zero where the field is flat (beyond range, or on a ridge)
  q: vec2f,     // nearest contour point, absolute device px (q = p beyond range)
  cover: f32,   // the Sdf's children's own coverage at p (antialiased alpha)
}
"#;

// Per Sdf input NAME at texture BINDING; the texture covers the same rect as src.
const SDF_ACCESSOR: &str = r#"
@group(0) @binding(BINDING) var prisma_in_NAME: texture_2d<f32>;
fn prisma_NAME_texel(i: vec2i) -> vec4f {
  if (any(i < vec2i(0)) || any(i >= vec2i(sys.src_size))) { return vec4f(-u.prisma_range_NAME, 0.0, 0.0, 0.0); }
  return textureLoad(prisma_in_NAME, i, 0);
}
// (d, q - p, cover), bilinear.
fn prisma_NAME_raw(p: vec2f) -> vec4f {
  let q = p - sys.src_origin - 0.5;
  let i = vec2i(floor(q));
  let f = q - floor(q);
  let a = mix(prisma_NAME_texel(i), prisma_NAME_texel(i + vec2i(1, 0)), f.x);
  let b = mix(prisma_NAME_texel(i + vec2i(0, 1)), prisma_NAME_texel(i + vec2i(1, 1)), f.x);
  return mix(a, b, f.y);
}
fn NAME(p: vec2f) -> SdfSample {
  let v = prisma_NAME_raw(p);
  var s: SdfSample;
  s.d = v.x;
  s.q = p + v.yz;
  s.cover = v.w;
  let l = length(v.yz);
  if (l > 1e-3) {
    // Inside, the nearest edge is outward; outside, it is back toward the shape.
    s.n = select(-v.yz, v.yz, v.x >= 0.0) / l;
  } else {
    // On the contour itself (or where the field is flat): the distance gradient.
    let g = vec2f(prisma_NAME_raw(p + vec2f(1.0, 0.0)).x - prisma_NAME_raw(p - vec2f(1.0, 0.0)).x,
                  prisma_NAME_raw(p + vec2f(0.0, 1.0)).x - prisma_NAME_raw(p - vec2f(0.0, 1.0)).x);
    let gl = length(g);
    s.n = select(vec2f(0.0), -g / gl, gl > 1e-4);
  }
  return s;
}
// Distance from p along the unit vector dir to the contour (where d changes
// sign), at most maxd. Sphere tracing: steps by |d|, at least 1px, and
// interpolates the crossing. For crops to match full renders, the effect's reach
// must cover maxd, as for any read of src.
fn NAME_march(p: vec2f, dir: vec2f, maxd: f32) -> f32 {
  let side = select(-1.0, 1.0, prisma_NAME_raw(p).x >= 0.0);
  var t = 0.0;
  var tp = 0.0;
  var dp = 0.0;
  for (var i = 0; i < 1024; i++) {
    let d = prisma_NAME_raw(p + dir * t).x * side; // > 0 while still on the starting side
    if (d <= 0.0) {
      if (t == 0.0) { return 0.0; }
      return min(tp + (t - tp) * dp / (dp - d), maxd);
    }
    if (d < 0.01) { return min(t + d, maxd); }
    if (t >= maxd) { return maxd; }
    tp = t;
    dp = d;
    t += max(d, 1.0);
  }
  return min(t, maxd);
}
"#;

const PRELUDE: &str = r#"
struct Sys {
  resolution: vec2f,   // canvas size, device px
  px_per_unit: f32,    // device px per doc px
  _pad0: f32,
  src_origin: vec2f,
  src_size: vec2f,
  dst_origin: vec2f,
  _pad1: vec2f,
}
@group(0) @binding(0) var<uniform> sys: Sys;
@group(0) @binding(1) var prisma_src: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: U;

// Children texels are stored straight; everything the effect sees is premultiplied.
fn prisma_texel(i: vec2i) -> vec4f {
  if (any(i < vec2i(0)) || any(i >= vec2i(sys.src_size))) { return vec4f(0.0); }
  let c = textureLoad(prisma_src, i, 0);
  return vec4f(c.rgb * c.a, c.a);
}

// The rendered children at absolute device position p, bilinear, transparent outside.
fn src(p: vec2f) -> vec4f {
  let q = p - sys.src_origin - 0.5;
  let i = vec2i(floor(q));
  let f = q - floor(q);
  let a = mix(prisma_texel(i), prisma_texel(i + vec2i(1, 0)), f.x);
  let b = mix(prisma_texel(i + vec2i(0, 1)), prisma_texel(i + vec2i(1, 1)), f.x);
  return mix(a, b, f.y);
}

// Canvas doc px <-> device px. p is device px; geometry written in the script is doc px.
fn to_doc(p: vec2f) -> vec2f { return p / sys.px_per_unit; }
fn to_px(q: vec2f) -> vec2f { return q * sys.px_per_unit; }
// The rendered children at canvas doc position q.
fn src_doc(q: vec2f) -> vec4f { return src(q * sys.px_per_unit); }

fn pm(c: vec4f) -> vec4f { return vec4f(c.rgb * c.a, c.a); }
fn unpm(c: vec4f) -> vec4f { if (c.a > 1e-4) { return vec4f(c.rgb / c.a, c.a); } return vec4f(0.0); }
fn luma(c: vec3f) -> f32 { return dot(c, vec3f(0.299, 0.587, 0.114)); }
fn rot(a: f32) -> mat2x2f { let c = cos(a); let s = sin(a); return mat2x2f(c, s, -s, c); }
// Signed distance (positive inside) from an Sdf's alpha `v`; `range` is the Sdf's range as a length uniform.
fn sdf_dist(v: f32, range: f32) -> f32 { return (v - 0.5) * 2.0 * range; }

// Uniform hashes in [0, 1) (Hoskins, sin-free).
fn hash(p: vec2f) -> f32 {
  var q = fract(vec3f(p.xyx) * 0.1031);
  q += dot(q, q.yzx + 33.33);
  q = fract(q);
  return fract((q.x + q.y) * q.z * 95.4307);
}
fn hash3(p: vec2f) -> vec3f {
  var q = fract(vec3f(p.xyx) * vec3f(0.1031, 0.1030, 0.0973));
  q += dot(q, q.yxz + 33.33);
  q = fract(q);
  return fract((q.xxy + q.yzz) * q.zyx * 95.4307);
}
fn vnoise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let w = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2f(1.0, 0.0)), w.x),
             mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0, 1.0)), w.x), w.y);
}
fn fbm(p: vec2f) -> f32 {
  return 0.5 * vnoise(p) + 0.25 * vnoise(p * 2.03) + 0.125 * vnoise(p * 4.12) + 0.0625 * vnoise(p * 8.37);
}

@vertex fn prisma_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn prisma_fs(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let c = clamp(effect(fc.xy + sys.dst_origin), vec4f(0.0), vec4f(1.0));
  return vec4f(min(c.rgb, vec3f(c.a)), c.a);
}
"#;

fn validate(name: &str, module: &str) -> Result<()> {
    let label = format!("effect {name:?} wgsl");
    let parsed = naga::front::wgsl::parse_str(module).map_err(|e| anyhow!("{}", e.emit_to_string_with_path(module, &label)))?;
    if !parsed.functions.iter().any(|(_, f)| f.name.as_deref() == Some("effect")) {
        bail!("{label}: missing `fn effect(p: vec2f) -> vec4f`");
    }
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&parsed)
        .map_err(|e| anyhow!("{}", e.emit_to_string_with_path(module, &label)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_and_reports_user_lines() {
        let ok = EffectDef::new(
            "t".into(),
            vec![("ink".into(), UType::Color, UValue::Color([0.0, 0.0, 0.0, 1.0])), ("k".into(), UType::Float, UValue::Num(1.0))],
            vec![],
            Margin::Fixed(0.0),
            Margin::Fixed(0.0),
            "fn effect(p: vec2f) -> vec4f { return src(p) * u.k + u.ink * hash(p); }".into(),
        );
        assert!(ok.is_ok(), "{:?}", ok.err());
        let err = EffectDef::new("t".into(), vec![], vec![], Margin::Fixed(0.0), Margin::Fixed(0.0), "\nfn effect(p: vec2f) -> vec4f { return nope; }".into())
            .unwrap_err()
            .to_string();
        assert!(err.contains(":2:"), "{err}");
    }
}
