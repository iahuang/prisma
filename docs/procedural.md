# Procedural techniques

The point of prisma is that a design is a program. This page covers the tools
for driving geometry from data: rasterizing a scene and reading its pixels
back, point grids, glyph outlines, distance fields, and seeded randomness.

## `rasterize(scene, opts) -> raster`

Renders a scene **while the script is running** and returns its pixels.
You can draw the result as an image or sample it to place vector shapes.

```lua
local field = rasterize(
	Group { blur = 6, Text { font = "Arial Black", size = 250, x = 110, y = -40, anchor = "middle", "A" } },
	{ size = { 220, 220 }, dpi = 96, name = "field" }
)
```

| Option | Default | |
| --- | --- | --- |
| `size` | **required** | `{ w, h }` doc size of the region, from (0, 0) in the scene's own coordinates |
| `dpi` | `96` | Pixel density; 96 = 1 px per doc px. Lower is faster when you only sample coarsely. |
| `bg` | transparent | Color |
| `margin` | the larger of `size`'s sides | As `Canvas { margin }`: how far past `size` blur, effects and fields read |
| `name` | none | With `--dump DIR`, the raster is written to `DIR/<name>.png` |

`scene` is any element or list of elements (not a `Canvas`).

A raster has these fields and methods:

| | |
| --- | --- |
| `r.width`, `r.height` | Pixel size |
| `r.dpi` | |
| `r.size` | `{ w, h }` in doc px |
| `r:at(x, y)` | `{ r, g, b, a, lum }` at doc-px coordinates, bilinear, 0–1, **straight** (not premultiplied) color. Outside the raster reads transparent. |
| `r:sample(points)` / `sample(r, points)` | Copy of each point table with `r g b a lum` added. Other keys (`i`, `j`, ...) are kept. |
| `Image { src = r, ... }` | Draw it. `crop` is in doc px. |

`lum` is `0.299 r + 0.587 g + 0.114 b` of the straight color. For coverage
of dark ink, use `(1 - p.lum) * p.a`.

Rasters are cached by content (the scene IR, the dpi, and the mtimes of
files the scene reads). The cache lives in memory and on disk in
`.prisma-cache/` next to the script, so re-running an unchanged `rasterize`
is free. Pass `--no-cache` to bypass the disk cache.

## Grids

```lua
grid({ w, h }, step)              -- square grid, cell centers from (step/2, step/2)
grid({ w, h }, step, { ox, oy })  -- custom first center
hexGrid({ w, h }, step)           -- rows offset by half a step, row spacing step·√3/2
```

Both return a list of `{ x, y, i, j }`, where `i` is the column and `j` the
row (0-based).

### Pattern: image → vector halftone

```lua
local src = rasterize(Source(), { size = { S, S }, dpi = 48 })
local dots = {}
for _, p in sample(src, hexGrid({ S, S }, cell)) do
	local cover = (1 - p.lum) * p.a
	if cover > 0.02 then
		table.insert(dots, Circle { cx = p.x, cy = p.y, r = cell / 2 * 1.1 * math.sqrt(cover), fill = "#1d2a6b" })
	end
end
```

Blur the source before rasterizing (`Group { blur = 6, ... }`) to get a
smooth 0–1 field. Brushes then taper at the edges instead of stepping
(`examples/a-grid.luau`).

Tens of thousands of small shapes are fine. Shapes with a blend mode get a
tight clip, so they stay cheap.

## Type as geometry: `outline`

```lua
local d = outline(Text { font = "Arial Black", size = 220, x = 30, y = 10, "Glass" })
Path { d = d, fill = "none", stroke = "#7fd1ff", strokeWidth = 2, dash = { 6, 4 } }
Mask { shape = Path { d = d }, stripes }
```

`outline` takes a `Text` element and returns SVG path data in doc px,
positioned exactly where that `Text` would render (`y` is the baseline). Use it for outlined,
dashed or gradient-stroked type, or for type as a mask. Concatenated with
other path data and filled with `fillRule = "evenodd"`, it cuts lettering
out of a shape ([scripting.md](scripting.md#holes-compound-shapes-and-outlines)).
To stretch, rotate or fit the outline without scaling its stroke, pass it
through `transformPath` and measure it with `pathBounds`
([scripting.md](scripting.md#transforming-path-data-transformpath-pathbounds)).

## Distance fields: `Sdf`

```lua
Sdf { range = 50, fill = "#fff", Circle { cx = 200, cy = 200, r = 100 } }
```

`Sdf` replaces its children with their **signed distance field**: the
distance to the children's alpha = 0.5 contour, encoded in alpha as
`0.5 + d / (2·range)`. That gives 1 at `range` inside, 0.5 on the edge, and
0 at `range` outside. It's drawn in `fill` (a color; default white).

| Prop | Default | |
| --- | --- | --- |
| `range` | `32` | Length, > 0 |
| `fill` | `"#fff"` | Color |

The field is useful on its own (as a glow or soft offset), but mostly as an
effect's input. **Pass it as an input slot** (see
[effects.md](effects.md#inputs-distance-fields-as-data)). The effect then reads
exact float distances, normals and nearest points, and can march rays through it:

```lua
local Bands = effect {
	inputs = { body = "sdf" },
	uniforms = { spacing = { "length", 9 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let f = body(p);   // f.d (device px, + inside), f.n, f.q, f.cover
  ...
}
]],
}
Bands { body = Sdf { range = 60, blobs } }
```

The older way still works: draw the `Sdf` as a child and decode its alpha
with `sdf_dist`. It's quantized to 8 bits, though (one step is
`2·range/255` px), and has no gradient, so prefer an input.

```lua
local Bands = effect {
	uniforms = { range = { "length", 60 }, spacing = { "length", 9 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  let d = sdf_dist(src(p).a, u.range);   // signed distance in device px
  ...
}
]],
}
Bands { range = 60, spacing = 9, Sdf { range = 60, blobs } }
```

With `sdf_dist`, pass the effect the same `range` as the `Sdf`. An effect over an `Sdf`
draws over the field's extent, which is the children grown by `range`. The field is carried in
alpha, so don't put anything opaque under the `Sdf` inside the same effect.
Contours up to `range` outside a crop still shape the field inside it, so
crops stay exact.

The contour is located to a few hundredths of a pixel from the antialiased
coverage. Prisma flattens the children's curves and glyphs finely before
rasterizing them for a field. Sharp corners come out rounded by about a third
of a pixel, because that's where the alpha 0.5 contour of a drawn corner is.

## Randomness

- `math.random`, `random()` and `pick()` all draw from one stream, seeded by
  `--seed` (default `1`). The same seed gives the same design.
- `random()` returns [0, 1), `random(max)` returns [0, max), and
  `random(min, max)` returns [min, max).
- `pick({ "#ff4b1f", "#2b50ff", "#00a35c" })` returns a random element.
- To explore variations, run `--sweep seed=1..9`
  ([cli.md](cli.md#contact-sheets)).
- Effects can't see the Lua seed. To vary a noise effect with the seed, pass
  it as a uniform: `Displace { seed = random(0, 100), ... }`.

## Params

`-p cell=12 -p glyph=B` sets values the script reads with `param`, typed by
the default it gives: `param("cell", 9)` is the number 12, and
`param("glyph", "A")` is the string `"B"`. `-p cell=abc` is then an error,
not a silent fallback. The raw strings are in `params` (`params.cell == "12"`).
A script can also return `function(params) return Canvas { ... } end`.
