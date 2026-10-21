//! wgpu device, vello renderer, and the offscreen passes (effects, blur).
//!
//! Texture conventions: vello writes straight-alpha Rgba8Unorm. Pass outputs
//! are premultiplied Rgba8Unorm, which is what gets handed back to vello as an
//! `AlphaPremultiplied` image.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use vello::wgpu::{self, util::DeviceExt};
use vello::{AaConfig, AaSupport, RenderParams, RendererOptions, Scene, peniko};

use crate::effects::{EffectDef, Sys};

/// An integer device-pixel rect in absolute canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl IRect {
    pub fn expand(self, by: u32) -> IRect {
        IRect { x: self.x - by as i32, y: self.y - by as i32, w: self.w + 2 * by, h: self.h + 2 * by }
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub vello: vello::Renderer,
    effect_pipelines: HashMap<String, (String, wgpu::RenderPipeline)>,
    blur_layout: wgpu::BindGroupLayout,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    mask_layout: wgpu::BindGroupLayout,
    mask: wgpu::RenderPipeline,
    sdf_init: wgpu::RenderPipeline,
    sdf_refine: wgpu::RenderPipeline,
    sdf_step: wgpu::RenderPipeline,
    sdf_final: wgpu::RenderPipeline,
    sdf_field: wgpu::RenderPipeline,
    sdf_preview: wgpu::RenderPipeline,
    /// Bind group and pipeline layouts for effects, by number of field inputs.
    effect_layouts: HashMap<usize, (wgpu::BindGroupLayout, wgpu::PipelineLayout)>,
}

/// How a mask's shape gates its content.
#[derive(Clone, Copy)]
pub enum MaskMode {
    Alpha = 0,
    Invert = 1,
    Luminance = 2,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurParams {
    dir: [f32; 2],
    sigma: f32,
    radius: f32,
    src_origin: [f32; 2],
    dst_origin: [f32; 2],
    src_size: [f32; 2],
    premul_in: f32,
    _pad: f32,
}

const BLUR_WGSL: &str = r#"
struct B { dir: vec2f, sigma: f32, radius: f32, src_origin: vec2f, dst_origin: vec2f, src_size: vec2f, premul_in: f32, _pad: f32 }
@group(0) @binding(0) var<uniform> b: B;
@group(0) @binding(1) var t: texture_2d<f32>;

fn load(i: vec2i) -> vec4f {
  if (any(i < vec2i(0)) || any(i >= vec2i(b.src_size))) { return vec4f(0.0); }
  let c = textureLoad(t, i, 0);
  if (b.premul_in > 0.5) { return vec4f(c.rgb * c.a, c.a); }
  return c;
}

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn fs(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let p = vec2i(floor(fc.xy + b.dst_origin - b.src_origin));
  let d = vec2i(b.dir);
  let k = -0.5 / (b.sigma * b.sigma);
  var acc = load(p);
  var wsum = 1.0;
  for (var i = 1; i <= i32(b.radius); i++) {
    let w = exp(f32(i * i) * k);
    acc += w * (load(p + d * i) + load(p - d * i));
    wsum += 2.0 * w;
  }
  return acc / wsum;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ResampleParams {
    f: f32,
    _pad: f32,
    src_size: [f32; 2],
    /// Up: output origin minus low-res grid origin, in full-res px.
    offset: [f32; 2],
    _pad2: [f32; 2],
}

// Box-downsample by f (premultiplying straight input) and bilinear upsample
// back, so wide blurs run at low resolution.
const RESAMPLE_WGSL: &str = r#"
struct R { f: f32, _pad: f32, src_size: vec2f, offset: vec2f, _pad2: vec2f }
@group(0) @binding(0) var<uniform> r: R;
@group(0) @binding(1) var t: texture_2d<f32>;

fn load(i: vec2i) -> vec4f {
  if (any(i < vec2i(0)) || any(i >= vec2i(r.src_size))) { return vec4f(0.0); }
  return textureLoad(t, i, 0);
}

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn down(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let f = i32(r.f);
  let base = vec2i(floor(fc.xy)) * f;
  var acc = vec4f(0.0);
  for (var y = 0; y < f; y++) {
    for (var x = 0; x < f; x++) {
      let c = load(base + vec2i(x, y));
      acc += vec4f(c.rgb * c.a, c.a);
    }
  }
  return acc / f32(f * f);
}

@fragment fn up(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let q = (fc.xy + r.offset) / r.f - 0.5;
  let i = vec2i(floor(q));
  let w = q - floor(q);
  let a = mix(load(i), load(i + vec2i(1, 0)), w.x);
  let b = mix(load(i + vec2i(0, 1)), load(i + vec2i(1, 1)), w.x);
  return mix(a, b, w.y);
}
"#;

// Content and mask cover the same region; output is premultiplied.
const MASK_WGSL: &str = r#"
@group(0) @binding(0) var<uniform> mode: vec4f;
@group(0) @binding(1) var content: texture_2d<f32>;
@group(0) @binding(2) var shape: texture_2d<f32>;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn fs(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let i = vec2i(fc.xy);
  let c = textureLoad(content, i, 0);
  let k = textureLoad(shape, i, 0);
  var m = k.a;
  if (mode.x > 1.5) { m = dot(k.rgb, vec3f(0.2126, 0.7152, 0.0722)) * k.a; }
  else if (mode.x > 0.5) { m = 1.0 - k.a; }
  let a = c.a * m;
  return vec4f(c.rgb * a, a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SdfParams {
    step: f32,
    range: f32,
    src_size: [f32; 2],
    /// Output origin minus input origin, device px.
    offset: [f32; 2],
    _pad: [f32; 2],
    /// Premultiplied.
    color: [f32; 4],
}

// Jump flooding (Rong & Tan) to the nearest contour point. A seed is a pixel
// the alpha 0.5 contour passes through or along (see `init`). It stores a subpixel
// estimate of where the contour crosses near it: the coverage gradient gives
// the edge direction, and the pixel's area coverage gives its distance from
// the pixel center (Gustavson & Strand's anti-aliased EDT). Distances are then
// point-to-segment in float: each seed stands for a ±0.5px piece of edge
// through its edge point, perpendicular to the gradient, so on a straight edge
// the nearest point is the exact perpendicular foot (and the normal exact),
// while a corner still acts as a point. Texels hold (edge.x, edge.y, seed,
// alpha) in texel coordinates where pixel i has its center at i + 0.5; seed is
// 0 for none, 1 for a point, else 10 + the inward normal's angle + 100 * h,
// with h in 1..8 the piece's half-length in sixteenths of a pixel.
const SDF_WGSL: &str = r#"
struct S { step: f32, range: f32, src_size: vec2f, offset: vec2f, _pad: vec2f, color: vec4f }
@group(0) @binding(0) var<uniform> s: S;
@group(0) @binding(1) var t: texture_2d<f32>;

fn load(i: vec2i) -> vec4f {
  if (any(i < vec2i(0)) || any(i >= vec2i(s.src_size))) { return vec4f(0.0); }
  return textureLoad(t, i, 0);
}
fn alpha(i: vec2i) -> f32 { return load(i).a; }
fn inside(i: vec2i) -> bool { return alpha(i) >= 0.5; }
// The point nearest to c on the piece of edge a seed texel stands for.
fn foot(seed: vec4f, c: vec2f) -> vec2f {
  if (seed.z < 5.0) { return seed.xy; }
  let h = floor(seed.z / 100.0);
  let a = seed.z - 100.0 * h - 10.0;
  let tangent = vec2f(-sin(a), cos(a));
  let half = h / 16.0;
  return seed.xy + tangent * clamp(dot(c - seed.xy, tangent), -half, half);
}
fn seed_code(n: vec2f, half: f32) -> f32 { return 10.0 + atan2(n.y, n.x) + 100.0 * clamp(round(half * 16.0), 1.0, 8.0); }
fn seed_normal(seed: vec4f) -> vec2f {
  let a = seed.z - 100.0 * floor(seed.z / 100.0) - 10.0;
  return vec2f(cos(a), sin(a));
}
fn partial(a: f32) -> bool { return a > 0.5 / 255.0 && a < 254.5 / 255.0; }

// Distance from a pixel center to the edge, along the unit gradient g (pointing
// inward), for a pixel whose area coverage by the half-plane is a. Positive when
// the center is outside.
fn edge_df(g: vec2f, a: f32) -> f32 {
  var gx = abs(g.x);
  var gy = abs(g.y);
  if (gx < 1e-4 || gy < 1e-4) { return 0.5 - a; }
  if (gx < gy) { let tmp = gx; gx = gy; gy = tmp; }
  let a1 = 0.5 * gy / gx;
  if (a < a1) { return 0.5 * (gx + gy) - sqrt(2.0 * gx * gy * a); }
  if (a < 1.0 - a1) { return (0.5 - a) * gx; }
  return -0.5 * (gx + gy) + sqrt(2.0 * gx * gy * (1.0 - a));
}

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn init(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let i = vec2i(floor(fc.xy));
  let a = alpha(i);
  let me = a >= 0.5;
  // The contour passes through partly covered pixels next to the other side;
  // a fully covered (or empty) pixel only locates it when its neighbour across
  // 0.5 is fully empty (or covered) too, i.e. a hard edge on a pixel boundary.
  var across8 = false;
  var hard = false;
  for (var y = -1; y <= 1; y++) {
    for (var x = -1; x <= 1; x++) {
      let o = vec2i(x, y);
      if (all(o == vec2i(0)) || inside(i + o) == me) { continue; }
      across8 = true;
      if (x == 0 || y == 0) { hard = hard || !partial(alpha(i + o)); }
    }
  }
  let seed = select(hard, across8, partial(a));
  if (!seed) { return vec4f(0.0, 0.0, 0.0, a); }
  // Isotropic Sobel gradient of coverage; points inward.
  let k = 1.41421356;
  let g = vec2f(
    alpha(i + vec2i(1, -1)) + k * alpha(i + vec2i(1, 0)) + alpha(i + vec2i(1, 1))
      - alpha(i + vec2i(-1, -1)) - k * alpha(i + vec2i(-1, 0)) - alpha(i + vec2i(-1, 1)),
    alpha(i + vec2i(-1, 1)) + k * alpha(i + vec2i(0, 1)) + alpha(i + vec2i(1, 1))
      - alpha(i + vec2i(-1, -1)) - k * alpha(i + vec2i(0, -1)) - alpha(i + vec2i(1, -1)));
  let c = vec2f(i) + 0.5;
  let gl = length(g);
  if (gl < 1e-5) { return vec4f(c, 1.0, a); }
  let n = g / gl;
  return vec4f(c + n * clamp(edge_df(n, a), -1.0, 1.0), seed_code(n, 0.5), a);
}

// The coverage gradient is only good to a few degrees on antialiased edges,
// but the edge points are good to a few hundredths of a pixel. So each seed's
// edge is refit as the principal axis of its neighbours' edge points
// (Gaussian-weighted, 5x5, and by how well each pixel's coverage locates the
// edge), and its edge point moved onto that line. Where the neighbours aren't
// collinear (a corner), the seed becomes a point.
@fragment fn refine(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let i = vec2i(floor(fc.xy));
  let own = load(i);
  if (own.z < 5.0) { return own; }
  var w_sum = 0.0;
  var m = vec2f(0.0);
  var pts: array<vec3f, 25>;
  var n_pts = 0;
  for (var y = -2; y <= 2; y++) {
    for (var x = -2; x <= 2; x++) {
      let t = load(i + vec2i(x, y));
      if (t.z > 0.5) {
        // Nearly empty or full pixels locate the edge poorly (their offset goes
        // like the square root of the uncovered area), so they count for little.
        let w = exp(-f32(x * x + y * y) / 4.5) * clamp(min(t.w, 1.0 - t.w) * 10.0, 0.05, 1.0);
        pts[n_pts] = vec3f(t.xy, w);
        n_pts++;
        w_sum += w;
        m += t.xy * w;
      }
    }
  }
  let sobel = seed_normal(own);
  if (n_pts < 3) { return own; }
  m /= w_sum;
  var cov = vec3f(0.0); // xx, xy, yy
  for (var k = 0; k < n_pts; k++) {
    let d = pts[k].xy - m;
    cov += pts[k].z * vec3f(d.x * d.x, d.x * d.y, d.y * d.y);
  }
  cov /= w_sum;
  if (cov.x + cov.z < 0.25) { return own; }
  // Neighbours far from collinear (a corner) make the seed a point (its own
  // estimate), so its piece of edge can't overshoot the vertex; in between it
  // blends continuously, so a one-level change in coverage can't jump it.
  let tr = cov.x + cov.z;
  let disc = sqrt((cov.x - cov.z) * (cov.x - cov.z) + 4.0 * cov.y * cov.y);
  let corner = smoothstep(0.005, 0.03, (tr - disc) / (tr + disc));
  if (corner >= 1.0) { return vec4f(own.xy, 1.0, own.w); }
  let th = 0.5 * atan2(2.0 * cov.y, cov.x - cov.z);
  var n = vec2f(-sin(th), cos(th));
  if (dot(n, sobel) < 0.0) { n = -n; }
  // The seed's edge point: its center's foot on the fitted line.
  let c = vec2f(i) + 0.5;
  let tangent = vec2f(cos(th), sin(th));
  let on_line = m + tangent * dot(c - m, tangent);
  return vec4f(mix(on_line, own.xy, corner), seed_code(n, 0.5 * (1.0 - corner)), own.w);
}

@fragment fn jump(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let i = vec2i(floor(fc.xy));
  let c = vec2f(i) + 0.5;
  let own = load(i);
  var best = own.xyz;
  var bd = 1e30;
  if (own.z > 0.5) { let d = foot(own, c) - c; bd = dot(d, d); }
  let k = i32(s.step);
  for (var y = -1; y <= 1; y++) {
    for (var x = -1; x <= 1; x++) {
      let n = load(i + vec2i(x, y) * k);
      if (n.z > 0.5) {
        let d = foot(n, c) - c;
        let dd = dot(d, d);
        if (dd < bd) { bd = dd; best = n.xyz; }
      }
    }
  }
  return vec4f(best, own.w);
}

// Signed distance (positive inside) at output pixel fc, and the offset to the
// nearest edge point; |d| >= range reports range and no offset.
fn signed(fc: vec4f) -> vec4f {
  let i = vec2i(floor(fc.xy + s.offset));
  let n = load(i);
  let me = n.w >= 0.5;
  if (n.z < 0.5) { return vec4f(select(-s.range, s.range, me), 0.0, 0.0, n.w); }
  let c = vec2f(i) + 0.5;
  let off = foot(n, c) - c;
  let d = select(-1.0, 1.0, me) * length(off);
  if (abs(d) >= s.range) { return vec4f(clamp(d, -s.range, s.range), 0.0, 0.0, n.w); }
  return vec4f(d, off, n.w);
}

// The Sdf element as a picture: color at alpha 0.5 + d / (2 range).
@fragment fn finish(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  return s.color * clamp(0.5 + signed(fc).x / (2.0 * s.range), 0.0, 1.0);
}

// An effect input: (d, q - p, coverage) as floats, device px.
@fragment fn field(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  return signed(fc);
}

// A viewable --dump of a field texture (offset 0): gray = 0.5 + d / (2 range).
@fragment fn preview(@builtin(position) fc: vec4f) -> @location(0) vec4f {
  let v = load(vec2i(floor(fc.xy)));
  return vec4f(vec3f(clamp(0.5 + v.x / (2.0 * s.range), 0.0, 1.0)), 1.0);
}
"#;

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Gpu {
    pub fn new() -> Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .context("no GPU adapter found")?;
        let limits = wgpu::Limits { max_texture_dimension_2d: 16384, ..Default::default() }.using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("prisma"),
            required_limits: limits,
            ..Default::default()
        }))
        .context("could not open GPU device")?;
        let vello = vello::Renderer::new(
            &device,
            RendererOptions { antialiasing_support: AaSupport::area_only(), ..Default::default() },
        )
        .map_err(|e| anyhow!("vello init: {e}"))?;

        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur"),
            entries: &[uniform_entry(0), texture_entry(1)],
        });
        let blur_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur"),
            source: wgpu::ShaderSource::Wgsl(BLUR_WGSL.into()),
        });
        let blur_h = fullscreen_pipeline(&device, &blur_pl, &blur_module, "vs", "fs", wgpu::TextureFormat::Rgba16Float);
        let blur_v = fullscreen_pipeline(&device, &blur_pl, &blur_module, "vs", "fs", wgpu::TextureFormat::Rgba8Unorm);
        let resample = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resample"),
            source: wgpu::ShaderSource::Wgsl(RESAMPLE_WGSL.into()),
        });
        let down = fullscreen_pipeline(&device, &blur_pl, &resample, "vs", "down", wgpu::TextureFormat::Rgba16Float);
        let up = fullscreen_pipeline(&device, &blur_pl, &resample, "vs", "up", wgpu::TextureFormat::Rgba8Unorm);

        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mask"),
            entries: &[uniform_entry(0), texture_entry(1), texture_entry(2)],
        });
        let mask_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mask"),
            bind_group_layouts: &[Some(&mask_layout)],
            immediate_size: 0,
        });
        let mask_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mask"),
            source: wgpu::ShaderSource::Wgsl(MASK_WGSL.into()),
        });
        let mask = fullscreen_pipeline(&device, &mask_pl, &mask_module, "vs", "fs", wgpu::TextureFormat::Rgba8Unorm);
        let sdf_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sdf"),
            source: wgpu::ShaderSource::Wgsl(SDF_WGSL.into()),
        });
        let sdf_init = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "init", wgpu::TextureFormat::Rgba32Float);
        let sdf_refine = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "refine", wgpu::TextureFormat::Rgba32Float);
        let sdf_step = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "jump", wgpu::TextureFormat::Rgba32Float);
        let sdf_final = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "finish", wgpu::TextureFormat::Rgba8Unorm);
        let sdf_field = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "field", wgpu::TextureFormat::Rgba32Float);
        let sdf_preview = fullscreen_pipeline(&device, &blur_pl, &sdf_module, "vs", "preview", wgpu::TextureFormat::Rgba8Unorm);

        Ok(Gpu {
            device,
            queue,
            vello,
            effect_pipelines: HashMap::new(),
            blur_layout,
            blur_h,
            blur_v,
            down,
            up,
            mask_layout,
            mask,
            sdf_init,
            sdf_refine,
            sdf_step,
            sdf_final,
            sdf_field,
            sdf_preview,
            effect_layouts: HashMap::new(),
        })
    }

    pub fn max_texture(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn texture(&self, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    /// Renders a vello scene to a new straight-alpha texture.
    pub fn render_scene(&mut self, scene: &Scene, w: u32, h: u32) -> Result<wgpu::Texture> {
        let tex = self.texture(
            w,
            h,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let view = tex.create_view(&Default::default());
        self.vello
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &view,
                &RenderParams {
                    base_color: peniko::Color::TRANSPARENT,
                    width: w.max(1),
                    height: h.max(1),
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| anyhow!("vello render: {e}"))?;
        Ok(tex)
    }

    fn pass_target(&self, w: u32, h: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        self.texture(
            w,
            h,
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        )
    }

    fn run_pass(&self, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup, target: &wgpu::Texture) {
        let view = target.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// The effect bind group layout for `fields` field inputs: sys, src, u, then one texture per field.
    fn effect_layout(&mut self, fields: usize) -> &(wgpu::BindGroupLayout, wgpu::PipelineLayout) {
        let device = &self.device;
        self.effect_layouts.entry(fields).or_insert_with(|| {
            let mut entries = vec![uniform_entry(0), texture_entry(1), uniform_entry(2)];
            entries.extend((0..fields as u32).map(|k| texture_entry(3 + k)));
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("effect"), entries: &entries });
            let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("effect"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            (layout, pipeline)
        })
    }

    /// Runs an effect over `input` (straight, covering `src`) into a premultiplied
    /// texture covering `dst`. `fields` are its field inputs (from `sdf_field`, also
    /// covering `src`), in the effect's input order.
    #[allow(clippy::too_many_arguments)]
    pub fn effect_pass(
        &mut self,
        def: &EffectDef,
        uniforms: &[u8],
        input: &wgpu::Texture,
        fields: &[wgpu::Texture],
        src: IRect,
        dst: IRect,
        resolution: [f32; 2],
        scale: f32,
    ) -> wgpu::Texture {
        let pipeline_layout = self.effect_layout(fields.len()).1.clone();
        let stale = self.effect_pipelines.get(&def.name).is_none_or(|(m, _)| *m != def.module);
        if stale {
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&def.name),
                source: wgpu::ShaderSource::Wgsl(def.module.as_str().into()),
            });
            let pipeline = fullscreen_pipeline(&self.device, &pipeline_layout, &module, "gw_vs", "gw_fs", wgpu::TextureFormat::Rgba8Unorm);
            self.effect_pipelines.insert(def.name.clone(), (def.module.clone(), pipeline));
        }
        let sys = Sys {
            resolution,
            px_per_unit: scale,
            _pad0: 0.0,
            src_origin: [src.x as f32, src.y as f32],
            src_size: [src.w as f32, src.h as f32],
            dst_origin: [dst.x as f32, dst.y as f32],
            _pad1: [0.0; 2],
        };
        let sys_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&sys),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let u_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: uniforms,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = input.create_view(&Default::default());
        let field_views: Vec<wgpu::TextureView> = fields.iter().map(|f| f.create_view(&Default::default())).collect();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: sys_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: u_buf.as_entire_binding() },
        ];
        for (k, v) in field_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: 3 + k as u32, resource: wgpu::BindingResource::TextureView(v) });
        }
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.effect_layouts[&fields.len()].0,
            entries: &entries,
        });
        let out = self.pass_target(dst.w, dst.h, wgpu::TextureFormat::Rgba8Unorm);
        self.run_pass(&self.effect_pipelines[&def.name].1, &bind, &out);
        out
    }

    fn bind(&self, layout: &wgpu::BindGroupLayout, uniform: &[u8], tex: &wgpu::Texture) -> wgpu::BindGroup {
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = tex.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        })
    }

    /// One separable gaussian pass from `from` (covering `from_rect`) into `to` (covering `to_rect`).
    #[allow(clippy::too_many_arguments)]
    fn blur_pass(&self, dir: [f32; 2], sigma: f32, from: &wgpu::Texture, from_rect: IRect, to: &wgpu::Texture, to_rect: IRect, premul_in: bool) {
        let params = BlurParams {
            dir,
            sigma,
            radius: (sigma * 3.0).ceil(),
            src_origin: [from_rect.x as f32, from_rect.y as f32],
            dst_origin: [to_rect.x as f32, to_rect.y as f32],
            src_size: [from_rect.w as f32, from_rect.h as f32],
            premul_in: if premul_in { 1.0 } else { 0.0 },
            _pad: 0.0,
        };
        let pipeline = if to.format() == wgpu::TextureFormat::Rgba16Float { &self.blur_h } else { &self.blur_v };
        self.run_pass(pipeline, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params), from), to);
    }

    /// Gaussian blur of `input` (straight, covering `src`) into a premultiplied texture covering `dst`.
    /// Wide blurs run on a box-downsampled copy; the box's own variance is taken out of sigma.
    pub fn blur(&self, input: &wgpu::Texture, src: IRect, dst: IRect, sigma: f32) -> wgpu::Texture {
        let f = (sigma / 3.0).floor().clamp(1.0, 16.0) as u32;
        let out = self.pass_target(dst.w, dst.h, wgpu::TextureFormat::Rgba8Unorm);
        if f == 1 {
            let radius = (sigma * 3.0).ceil() as u32;
            // The horizontal pass covers dst grown vertically, so the vertical pass has its taps.
            let mid = IRect { x: dst.x, y: dst.y - radius as i32, w: dst.w, h: dst.h + 2 * radius };
            let mid_tex = self.pass_target(mid.w, mid.h, wgpu::TextureFormat::Rgba16Float);
            self.blur_pass([1.0, 0.0], sigma, input, src, &mid_tex, mid, true);
            self.blur_pass([0.0, 1.0], sigma, &mid_tex, mid, &out, dst, false);
            return out;
        }
        let low = IRect { x: 0, y: 0, w: src.w.div_ceil(f), h: src.h.div_ceil(f) };
        let resample = |offset: [f32; 2], size: IRect| ResampleParams {
            f: f as f32,
            _pad: 0.0,
            src_size: [size.w as f32, size.h as f32],
            offset,
            _pad2: [0.0; 2],
        };
        let small = self.pass_target(low.w, low.h, wgpu::TextureFormat::Rgba16Float);
        self.run_pass(&self.down, &self.bind(&self.blur_layout, bytemuck::bytes_of(&resample([0.0; 2], src)), input), &small);
        let s = ((sigma * sigma - (f * f - 1) as f32 / 12.0).max(0.25)).sqrt() / f as f32;
        let a = self.pass_target(low.w, low.h, wgpu::TextureFormat::Rgba16Float);
        let b = self.pass_target(low.w, low.h, wgpu::TextureFormat::Rgba16Float);
        self.blur_pass([1.0, 0.0], s, &small, low, &a, low, false);
        self.blur_pass([0.0, 1.0], s, &a, low, &b, low, false);
        let offset = [(dst.x - src.x) as f32, (dst.y - src.y) as f32];
        self.run_pass(&self.up, &self.bind(&self.blur_layout, bytemuck::bytes_of(&resample(offset, low)), &b), &out);
        out
    }

    /// `content` gated by `shape` (both straight, same size) into a premultiplied texture.
    pub fn mask(&self, content: &wgpu::Texture, shape: &wgpu::Texture, mode: MaskMode) -> wgpu::Texture {
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[mode as u32 as f32, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (cv, sv) = (content.create_view(&Default::default()), shape.create_view(&Default::default()));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.mask_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cv) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&sv) },
            ],
        });
        let out = self.pass_target(content.width(), content.height(), wgpu::TextureFormat::Rgba8Unorm);
        self.run_pass(&self.mask, &bind, &out);
        out
    }

    fn sdf_params(src: IRect, dst: IRect, step: u32, range: f32, color: [f32; 4]) -> SdfParams {
        SdfParams {
            step: step as f32,
            range,
            src_size: [src.w as f32, src.h as f32],
            offset: [(dst.x - src.x) as f32, (dst.y - src.y) as f32],
            _pad: [0.0; 2],
            color,
        }
    }

    /// Jump-floods `input` (straight, covering `src`) to its nearest contour points.
    /// Only distances up to `range` (device px) are exact.
    fn jump_flood(&self, input: &wgpu::Texture, src: IRect, range: f32) -> wgpu::Texture {
        let params = |step: u32| Self::sdf_params(src, src, step, range, [0.0; 4]);
        let mut a = self.pass_target(src.w, src.h, wgpu::TextureFormat::Rgba32Float);
        let mut b = self.pass_target(src.w, src.h, wgpu::TextureFormat::Rgba32Float);
        self.run_pass(&self.sdf_init, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params(0)), input), &b);
        self.run_pass(&self.sdf_refine, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params(0)), &b), &a);
        // Steps from the first power of two covering the range down to 1, plus a
        // final 1 (JFA+1) to fix most of the plain algorithm's errors.
        let reach = (range.ceil() as u32).min(src.w.max(src.h)).max(1);
        let mut steps: Vec<u32> = std::iter::successors(Some(reach.next_power_of_two()), |k| (*k > 1).then_some(k / 2)).collect();
        steps.push(1);
        for k in steps {
            self.run_pass(&self.sdf_step, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params(k)), &a), &b);
            std::mem::swap(&mut a, &mut b);
        }
        a
    }

    /// Signed distance field of `input` (straight, covering `src`) into a
    /// premultiplied texture covering `dst`: `color` at alpha 0.5 + d / (2 range).
    /// Only distances up to `range` (device px) are exact; `src` should extend
    /// `range` beyond `dst` so contours just outside it are seen.
    pub fn sdf(&self, input: &wgpu::Texture, src: IRect, dst: IRect, range: f32, color: [f32; 4]) -> wgpu::Texture {
        let flood = self.jump_flood(input, src, range);
        let out = self.pass_target(dst.w, dst.h, wgpu::TextureFormat::Rgba8Unorm);
        let params = Self::sdf_params(src, dst, 0, range, color);
        self.run_pass(&self.sdf_final, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params), &flood), &out);
        out
    }

    /// The distance field of `input` (straight, covering `src`) as an Rgba32Float
    /// effect input covering `dst`: (d, q - p, coverage), device px, d positive
    /// inside and clamped to ±`range`. `src` should extend `range` beyond `dst`.
    pub fn sdf_field(&self, input: &wgpu::Texture, src: IRect, dst: IRect, range: f32) -> wgpu::Texture {
        let flood = self.jump_flood(input, src, range);
        let out = self.pass_target(dst.w, dst.h, wgpu::TextureFormat::Rgba32Float);
        let params = Self::sdf_params(src, dst, 0, range, [0.0; 4]);
        self.run_pass(&self.sdf_field, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params), &flood), &out);
        out
    }

    /// A viewable Rgba8 picture of a field from `sdf_field`, for --dump.
    pub fn field_preview(&self, field: &wgpu::Texture, range: f32) -> wgpu::Texture {
        let r = IRect { x: 0, y: 0, w: field.width(), h: field.height() };
        let out = self.pass_target(r.w, r.h, wgpu::TextureFormat::Rgba8Unorm);
        let params = Self::sdf_params(r, r, 0, range, [0.0; 4]);
        self.run_pass(&self.sdf_preview, &self.bind(&self.blur_layout, bytemuck::bytes_of(&params), field), &out);
        out
    }

    /// Reads an Rgba8Unorm texture back as tightly packed RGBA bytes.
    pub fn read(&self, tex: &wgpu::Texture) -> Result<Vec<u8>> {
        let (w, h) = (tex.width(), tex.height());
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
            },
            tex.size(),
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("gpu poll: {e}"))?;
        let data = slice.get_mapped_range().map_err(|e| anyhow!("gpu readback: {e}"))?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&data[y * row as usize..][..w as usize * 4]);
        }
        Ok(out)
    }
}
