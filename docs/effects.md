# Effects

An effect is a per-pixel WGSL function over its rendered children. The
built-in effects are written the same way you write your own (see
`src/prelude.luau`).

## Using an effect

```lua
Halftone { cell = 9, angle = 30, ink = "#1d2a6b",
	Circle { cx = 680, cy = 400, r = 70, fill = "#888" },
}
```

- The props are the effect's uniforms plus `opacity`, `blend`, `blur`, `id`
  and `area` (see below). Uniforms you leave out keep their defaults. A wrong uniform name is an
  error with a suggestion.
- Children render first, into an offscreen texture. The effect then reads
  them through `src(p)`.
- Effects nest and stack. For example, `Grain { Rough { titles } }` roughens
  the titles and then adds grain over the result.

## Built-in effects

| Effect | Uniforms (default) | Does |
| --- | --- | --- |
| `Halftone` | `cell` length (`"1.5mm"`), `angle` (45), `ink` (`"#000"`), `paper` (`"#0000"`) | Dots sized by darkness on a rotated grid |
| `Duotone` | `dark` (`"#1b1b3a"`), `light` (`"#f4e9d8"`) | Maps luminance onto two colors |
| `Posterize` | `levels` (4) | Quantizes each channel |
| `Grain` | `amount` (0.12), `size` length (1) | Per-cell lightness noise |
| `Displace` | `amount` length (6), `freq` length (40), `seed` (0) | Smooth noise warp of up to `amount` |
| `Speckle` | `spacing` length (20), `size` length (0.8), `density` (0.3), `light` (`"#e0e8e8"`), `dark` (`"#000a"`), `darkShare` (0.3), `seed` (0) | Dust and pinholes, kept inside shapes |
| `Rough` | `amount` (0.5), `freq` length (3), `seed` (0) | Ragged, printed-looking edges (re-thresholds alpha against noise) |

## Defining an effect

```lua
local Scatter = effect {
	name = "scatter",                          -- optional; see the watch-mode note below
	uniforms = { amount = { "length", 14 } },  -- name = { type, default }
	reach = "amount",                          -- how far outside what it draws the effect reads src
	grow = "amount",                           -- how far outside the children it draws
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let a = hash(p) * 6.2831853;
  let r = sqrt(hash(p + 31.7)) * u.amount;
  return src(clamp(p + vec2f(cos(a), sin(a)) * r, vec2f(0.5), sys.resolution - 0.5));
}
]],
}

return Canvas { size = { 800, 600 }, Scatter { amount = 16, blobs } }
```

`effect { ... }` returns a constructor. Its only fields are `name`,
`uniforms`, `inputs`, `reach`, `grow` and `wgsl`; anything else is an error. The WGSL is compiled
and validated (by naga) when the effect is defined. Compile errors report
line numbers inside your `wgsl` string.

### The contract

- You must define `fn effect(p: vec2f) -> vec4f`.
- `p` is the **absolute device-pixel position** (pixel center) on the
  canvas, not a local or normalized coordinate. It's the same position in a
  full render and in a `--crop`.
- `src(p)` returns the children at `p`, **premultiplied** RGBA, bilinear,
  and transparent outside the input.
- You return **premultiplied** RGBA. The result is clamped to 0–1, with rgb
  clamped to ≤ alpha. To work in straight color, use
  `pm(... unpm(src(p)) ...)`.

### Uniforms

| Type | In the script | In WGSL (`u.name`) |
| --- | --- | --- |
| `"float"` | number | `f32` as is |
| `"length"` | length (number or `"3mm"`) in doc px | `f32` in **device px** (scaled by the render scale) |
| `"angle"` | degrees | `f32` radians |
| `"color"` | color string | **premultiplied** `vec4f` |

Uniform names must be WGSL identifiers, and can't be `opacity`, `blend`,
`blur`, `id` or `area`, since every effect already has those props.

**Write geometry in doc px.** To place things at coordinates computed in
the script (a circle at `cx, cy`, a horizon at `y = 1110`), convert once with
`let q = to_doc(p);`, do the math in doc px, and read with `src_doc(q)`. Doc
px here means **canvas** coordinates: an effect inside a translated or
scaled `Group` doesn't see the group's local space.

**Make anything spatial a `length` uniform.** Lengths are converted to device
px, so an effect looks the same at `--scale 1` and `--scale 4`. A raw
constant such as `hash(floor(p / 3.0))` is in device pixels, and its grain
changes size when you change the scale.

### Inputs: distance fields as data

An effect can declare named **inputs** besides its children. The call site
fills each one with an explicit element of the declared kind. Prisma computes
it offscreen into a float texture and binds it straight to the shader, so the
values never pass through 8-bit compositing.

```lua
local Lit = effect {
	inputs = { body = "sdf" },               -- name = kind
	reach = "shadow",                        -- marches read beyond what is drawn
	uniforms = { shadow = { "length", 140 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let f = body(p);                                        // SdfSample
  let up = body_march(p, vec2f(0.0, -1.0), u.shadow);     // px to the contour, looking up
  ...
}
]],
}

Lit { area = { 0, 0, W, H }, body = Sdf { range = 60, blob, Circle { ... } } }
```

The only kind so far is `"sdf"`, which takes an `Sdf { range, ... }`. The slot
must be exactly that element. A bare shape, a list or another element is an
error (`input `body` expects an Sdf, got Circle (wrap it: body = Sdf { ... })`),
and so is a missing input. `fill`, `opacity` and `blend` on the `Sdf` are
errors there, because it's read as data rather than drawn. `blur` on an input
`Sdf` is not supported yet.

For each `"sdf"` input `NAME`, the prelude defines:

| Function | Returns |
| --- | --- |
| `NAME(p: vec2f) -> SdfSample` | `.d`: signed distance to the children's alpha 0.5 contour, device px, positive inside, clamped to ±`range`. `.n`: outward unit normal (zero where the field is flat: beyond `range`, or exactly on a ridge). `.q`: nearest contour point, absolute device px. `.cover`: the children's own antialiased coverage at `p`, for crisp edges. |
| `NAME_march(p: vec2f, dir: vec2f, maxd: f32) -> f32` | Distance from `p` along the unit vector `dir` to where `d` changes sign, at most `maxd` (device px). Sphere tracing: about 5 to 20 reads for most rays, more when a ray runs close alongside an edge. |

How precise it is, measured against a polygon whose rendered edge is exact
(`tests/fixtures/field_probe.luau`): `d` within 0.03 px, marches within
0.03 px, normals within 1° for over 99% of pixels (worst about 5°, a pixel
or two from an edge). Two limits come from rendering itself:

- **Corners are rounded by antialiasing.** The alpha 0.5 contour cuts a
  sharp corner by about a third of a pixel, so the field follows the rounded
  corner. That's correct for what's drawn, but it isn't the mathematical
  vertex.
- **Curves are flattened.** Prisma flattens the children's curves and glyphs
  to 0.01 px before rasterizing them for a field. vello's own 0.25 px chords
  would step the normals of a 70 px circle by about 10°.

Inputs and `src` cover the same rect, so `reach` governs both: for crops to
match full renders, `reach` must cover any offset you read at, including a
march's `maxd`. The effect draws over its children's bounds plus the bounds
of each input's children, grown by `grow` (or over `area`). An effect can
therefore have no children and draw only from its inputs.

Input names follow the uniform rules. They also can't match a uniform or a
prelude name (`src`, `hash`, `fbm`, ...), or start with `prisma_`.

### Where an effect draws and reads: `grow`, `area` and `reach`

These are two separate settings:

- **`grow`**: how far beyond its children's bounds the effect draws (default
  0). Set it when the effect moves or spreads ink outward, as displacement,
  scatter or halftone cells do.
- **`reach`**: how far beyond what it draws the shader can read `src` and
  its inputs (default 0). Set it when the effect reads at an offset: a
  displacement, a neighbourhood, or a march's `maxd`. Reads past it come
  back transparent, and crops only match full renders for reads within it.
  Reads also stop at the canvas's `margin` past its edge
  ([scripting.md](scripting.md#canvas-root-only)). A huge `reach` near the
  edge costs only what lies within it.

Each is a fixed length or the name of a `length` uniform (`reach = "amount"`).
Neither can be negative. The old `pad` set both; `pad = x` is now
`reach = x, grow = x`, and `pad` itself is an error.

`area = { x, y, w, h }` (local doc px) sets the drawn region explicitly and
replaces the children's bounds. With `area`, an effect can generate a
full-bleed background with no children at all:

```lua
Paper { area = { 0, 0, W, H } }                          -- generated ground
Bands { area = { 400, 0, 400, 400 }, Sdf { range = 60, blobs } }
```

`src` still reads up to `reach` beyond the area.

### WGSL helpers available in every effect

| Helper | |
| --- | --- |
| `src(p: vec2f) -> vec4f` | Children at absolute device px, premultiplied |
| `to_doc(p: vec2f) -> vec2f`, `to_px(q: vec2f) -> vec2f` | Device px ↔ canvas doc px (`p / sys.px_per_unit` and back) |
| `src_doc(q: vec2f) -> vec4f` | `src` at a canvas doc-px position |
| `pm(c)`, `unpm(c)` | Premultiply / un-premultiply a `vec4f` |
| `luma(rgb: vec3f) -> f32` | Rec. 601 luminance |
| `rot(a: f32) -> mat2x2f` | 2D rotation by `a` radians |
| `sdf_dist(v: f32, range: f32) -> f32` | Signed distance (positive inside) from an `Sdf` drawn as a picture (8-bit; prefer an `"sdf"` input) |
| `hash(p: vec2f) -> f32`, `hash3(p: vec2f) -> vec3f` | Uniform hashes in [0, 1) |
| `vnoise(p: vec2f) -> f32` | Value noise |
| `fbm(p: vec2f) -> f32` | 4-octave value noise, roughly 0–1 |
| `sys.resolution` | Canvas size in device px |
| `sys.px_per_unit` | Device px per doc px (the render scale) |

### Names

- With no `name`, the effect is named after a hash of its WGSL.
- Within one run, defining the same name twice with **different** code is
  an error: `effect "scatter" is defined twice with different code`.
- Each run starts with no user effects defined, so editing a named effect
  under `--watch`, or changing its code per `--sweep` cell, works.

## Recipes

Tint by luminance (straight-color math, premultiplied in and out):

```wgsl
fn effect(p: vec2f) -> vec4f {
  let c = unpm(src(p));
  let o = mix(unpm(u.dark), unpm(u.light), luma(c.rgb));
  return pm(vec4f(o.rgb, o.a * c.a));
}
```

Generate from scratch and ignore the input (use with `area`, as in
`examples/type.luau`):

```wgsl
fn effect(p: vec2f) -> vec4f {
  let t = fbm(p / u.scale);
  return vec4f(vec3f(t), 1.0);
}
```
