# Scripting reference

A design file is a Luau script. It must return a `Canvas { ... }`, or a
function that takes `params` and returns one.

```lua
return Canvas { size = { 600, 850 }, bg = "#efebe2",
	Rect { x = 0, y = 0, w = 600, h = 120, fill = "#141414" },
	Text { x = 18, y = 22, size = 64, font = "DIN Condensed", weight = 700, fill = "#efebe2", "ORGD 2026" },
}
```

## How elements work

- `Rect { ... }` is an ordinary Lua call with a table argument. Named keys
  are **props**. Array entries are **children** (or, for `Text`, the text).
- Constructors only tag the table. Nothing is checked until the script
  returns, so an error points at the line where the element was built
  (`poster.luau:12: Rect: ...`).
- Elements are plain tables. You can build them in functions and loops, keep
  them in variables, and read their props back (`blobs[1].points`).
- In children:
  - **nested lists flatten**, so `{ a, b, { c, d } }` is fine and helpers can
    return lists;
  - **`nil` and `false` are skipped**, so `cond and Circle { ... }` works;
  - a bare string is an error. Text goes inside `Text { ... }`.
- **Unknown props are errors**, with a suggestion:
  `Rect: unknown prop `fil` (did you mean `fill`?)`.
- Every element accepts `id = "anything"`. It's a label for you and
  never reaches the render.

## Units

**Lengths** are numbers (doc px, i.e. CSS px, 1/96 in) or strings with a unit:
`"12mm"`, `"9pt"`, `"1in"`, `"2.5cm"`, `"40px"`. There are no `em` or `%`
units. Any prop documented as a length accepts both forms. That includes
`size`, points and rects.

**Colors** are strings:

| Form | Example |
| --- | --- |
| hex | `"#f00"`, `"#f008"`, `"#ff0000"`, `"#ff000080"` |
| rgb / rgba | `"rgb(255, 0, 0)"`, `"rgba(255, 0, 0, 0.5)"` (alpha is 0–1) |
| keywords | `"none"`, `"transparent"`, `"black"`, `"white"` (no other CSS names) |

**Angles** (`rotate`) are degrees, clockwise (y points down).

**Coordinates**: the origin is top-left and y grows downward. Children of a
`Group` use the group's local coordinates.

## Common props (every element)

| Prop | Default | Notes |
| --- | --- | --- |
| `opacity` | `1` | 0–1 (anything else is an error). Fades the element's composited result, whatever its kind: a shape's stroke doesn't let its own fill show through, and overlapping glyphs don't darken. To let overlapping shapes show through each other, give each one its own opacity. |
| `blend` | `"normal"` | One of `normal multiply screen overlay darken lighten color-dodge color-burn hard-light soft-light difference exclusion hue saturation color luminosity plus`. A blend applies against what's beneath the element inside its parent. |
| `blur` | `0` | Gaussian **sigma** (a length, ≥ 0). It blurs the element's rendered result before opacity and blend. On a `Group` it blurs the composite. |
| `id` | — | Free-form label, ignored. |

## Paint (`fill`, `stroke`)

A color string, `"none"`, or a gradient:

```lua
fill = linear({ x0, y0 }, { x1, y1 }, { "#ff4b1f", "#2b50ff" })               -- evenly spaced stops
fill = radial({ cx, cy }, r, { { 0, "#fff" }, { 0.3, "#eab" }, { 1, "#5c9" } })  -- {t, color} stops
```

Gradient coordinates use the same local space as the element's geometry,
for shapes and `Text` alike. They're **not** relative to the shape's own
box.

## Elements

### `Canvas` (root only)

| Prop | Default | |
| --- | --- | --- |
| `size` | **required** | `{ w, h }`, lengths |
| `bg` | `"#fff"` | Color; `"none"` for transparent |

Children are the scene. `Canvas` can't be nested. To put one scene inside
another, `rasterize` it ([procedural.md](procedural.md)).

### Shapes: `Rect`, `Circle`, `Ellipse`, `Line`, `Path`

Shared style props:

| Prop | Default | Notes |
| --- | --- | --- |
| `fill` | black | Paint. Stays black when you add a `stroke`: an outline-only shape says `fill = "none"`. `Line` is never filled, and a `fill` on it is an error. |
| `fillRule` | `"nonzero"` | `nonzero evenodd`. With `evenodd`, wherever subpaths overlap an even number of times is a hole: see below. |
| `stroke` | none | Paint |
| `strokeWidth` | `1` | Length |
| `cap` | `"butt"` | `butt round square` |
| `join` | `"round"` | `miter round bevel` |
| `dash` | none | List of lengths, e.g. `{ 12, 6 }` |

| Element | Geometry props |
| --- | --- |
| `Rect` | `x=0 y=0 w h` (w, h required), `r=0` corner radius |
| `Circle` | `cx=0 cy=0 r` (r required) |
| `Ellipse` | `cx=0 cy=0 rx ry` (required), `rotate=0` |
| `Line` | `x1 y1 x2 y2` (all required). Never filled; stroke defaults to black. |
| `Path` | `d` **or** `points` (not both), plus a placement: `x=0 y=0 rotate=0 scale=1`, **or** `transform` (see `Group`) |

`Path` details:

- `d` is SVG path data (`"M40,300 C140,240 240,360 340,300 Z"`). Luau
  backtick strings are handy here: `` `M{x},{y} L{x2},{y2}` ``.
- `points = { {x, y}, ... }` (at least 2) draws straight segments through the
  points.
  - `closed = true` closes the path.
  - `smooth = true` draws a Catmull-Rom spline that passes **through** every
    point.
  - `closed` and `smooth` only apply to `points`. Using them with `d` is an
    error.
- `scale` is a number or `{ sx, sy }`. The transform is applied as
  translate, then rotate, then scale. `transform = { a, b, c, d, e, f }`
  replaces all four, as on `Group`.

#### Holes, compound shapes and outlines

A logo or glyph-like mark is usually **one `Path`**, not shapes stacked
under masks. Concatenate subpaths into one `d`, and cut holes with
`fillRule = "evenodd"`: a point inside an even number of subpaths is
outside the shape. (With the default `nonzero`, an inner contour is a hole
only if it winds the other way.) Then draw the **same `d`** with
`fill = "none"` and a `stroke` for an outline-only view, such as a
construction diagram. `examples/logo.luau` does both:

```lua
local function circle(r) return `M{-r},0 A{r},{r} 0 1,0 {r},0 A{r},{r} 0 1,0 {-r},0 Z` end
local mark = circle(120) .. circle(72) .. "M-150,-24 H150 V24 H-150 Z"  -- ring + bar
Path { d = mark, x = 190, y = 180, fill = "#1d3557", fillRule = "evenodd" }  -- the mark
Path { d = mark, x = 570, y = 180, fill = "none", stroke = "#e63946" }       -- its outlines
```

`outline(Text {...})` gives type as path data, so lettering can join a mark
the same way. The path data is a plain string, so a script can build, reuse
and transform it like any other value.

### `Group`

| Prop | Default | Notes |
| --- | --- | --- |
| `x`, `y` | `0` | Translation |
| `rotate` | `0` | Degrees |
| `scale` | `1` | Number or `{ sx, sy }`. `{ 0.6, 1 }` makes a condensed face out of a regular one. |
| `transform` | none | `{ a, b, c, d, e, f }`: a general affine, **instead of** `x y rotate scale` (giving both is an error) |
| `clip` | none | `{ x, y, w, h }` in the group's **local** coordinates |

The transform order is translate · rotate · scale, so children rotate and
scale about the group's own origin `(x, y)`.

`transform` is in SVG `matrix(a, b, c, d, e, f)` order: a local point
`(x, y)` lands at `(a x + c y + e, b x + d y + f)`. So `(a, b)` is where
the local x axis points, `(c, d)` the local y axis, and `(e, f)` the
origin (lengths). It expresses shear and any skewed "tilted screen" look,
and everything inside stays vector: text, strokes and gradients are
transformed, not resampled. It is affine only, so parallel lines stay
parallel; there's no perspective.

```lua
Group { transform = { 1, 0, 0.5, 1, 600, 300 }, Rect { w = 40, h = 40 } }  -- sheared right
-- map the local axes onto two measured screen directions
Group { transform = { ux * s, uy * s, vx * s, vy * s, ox, oy }, ui }
```

### `Mask`

```lua
Mask { shape = Circle { cx = 340, cy = 400, r = 60, blur = 12 },  -- the gate
	stripes,                                                       -- the content
}
```

| Prop | Default | Notes |
| --- | --- | --- |
| `shape` | **required** | An element or a list. Its alpha gates the children. A blurred shape gives a soft mask. |
| `invert` | `false` | Keep the content *outside* the shape |
| `luminance` | `false` | Gate by the shape's brightness × alpha instead of alpha alone. Costs an offscreen pass. |

To cut a hard-edged hole in a shape, prefer one `Path` with
`fillRule = "evenodd"` ([above](#holes-compound-shapes-and-outlines)) to a
`Mask`: it's cheaper, and it stays a path you can stroke.

### `Text`

```lua
Text { x = 40, y = 537, size = 48, font = "Helvetica", weight = 700, "prisma" }
Text { x = 496, y = 141, w = 500, size = 29, leading = 1.12, "LINE ONE\nLINE TWO" }
```

| Prop | Default | Notes |
| --- | --- | --- |
| `x`, `y` | `0` | `y` is the **baseline of the first line**. Later lines follow it, spaced by `leading`. |
| `w` | none | Wrap width: the text box is `[x, x + w]`. Without it, each `\n` line is laid out on its own. |
| `align` | `"left"` | **Only with `w`**: `left center right justify`, inside the box. |
| `anchor` | `"start"` | **Only without `w`**: `start middle end`. Where the unboxed text sits on `x` (it starts, is centered, or ends there); its lines align with each other the same way. |
| `size` | `16` | Length |
| `font` | `"Helvetica"` | A system font **family** name. See below |
| `fontFile` | none | A font file path, relative to the script, instead of `font` |
| `fill` | black | Paint, in the same coordinates as `x`/`y` (no stroke; use `outline()` for outlined type) |
| `weight` | the family's regular | Number, 100–900 |
| `italic` | `false` | |
| `leading` | font metrics | Line height as a multiple of `size` |
| `tracking` | `0` | Letter spacing in **em** (fraction of `size`); `-0.04` is tight |
| `features` | none | OpenType features, e.g. `{ liga = 0, ss01 = 1 }` |
| `variations` | none | Variable-font axes, e.g. `{ wght = 650, wdth = 80 }` |

Children are the text. Strings and numbers are concatenated; `nil`/`false`
are skipped.

**Fonts**:

- `font` is looked up as a family name exactly. The face is picked with
  `weight` and `italic`, never parsed out of the name. `"DIN Condensed Bold"`
  is an error that names the fix: `font = "DIN Condensed", weight = 700`.
  (Some families, such as `Arial Black`, really are named that way.)
- A font file goes in `fontFile = "fonts/Inter.ttf"`, loaded relative to the
  script. A path in `font` is an error that points you to `fontFile`, and
  giving both is an error.
- A missing font is an error: `font "Helvtica" not found`, sometimes with
  `(similar: ...)`. Families used in the examples include `Helvetica`,
  `Arial Black`, `Courier New`, `Times New Roman` and `DIN Condensed` (all on
  macOS).

### `Image`

```lua
Image { src = "assets/photo.jpg", x = 0, y = 0, w = 600, h = 400, fit = "cover" }
Image { src = someRaster, x = 20, y = 20 }  -- a value returned by rasterize()
```

| Prop | Default | Notes |
| --- | --- | --- |
| `src` | **required** | A file path (relative to the script) or a raster |
| `x`, `y` | `0` | |
| `w`, `h` | natural size | Give one and the other follows the aspect ratio. Natural size treats 1 image px as 1 doc px. For a raster it's the doc size it was made at. |
| `crop` | whole image | `{ x, y, w, h }` source rect, in **image px** for files and **doc px** for rasters |
| `fit` | `"fill"` | `fill` stretches; `cover` fills `w×h` and crops the overflow; `contain` letterboxes |

The formats are PNG, JPEG and WebP, at any size that decodes (512 MiB
of pixels). vello draws an image at most 8192 px on a side, so for a bigger
one prisma uploads only the part `crop`/`fit` shows, downsampled to the size
it's drawn at. Drawing more than 8192 device px of one image in one
direction (say, a 12000 px tall page at full size into a 12000 px canvas)
is an error that says so; crop it into pieces.

### Effects and `Sdf`

These are elements too: `Halftone { cell = 9, ... children }`,
`Sdf { range = 50, ... }`. See [effects.md](effects.md) and
[procedural.md](procedural.md#distance-fields-sdf).

## Globals available to scripts

| Name | What |
| --- | --- |
| `param(name, default)` | A `-p name=value` read as the type of `default` (number, string or boolean): `param("cell", 9)`. Without `-p` it returns `default`. A value that isn't that type is an error (`-p cell=abc`), and so is a missing one when there's no default. |
| `params` | The raw `-p k=v` values, all **strings** as typed (`-p id=007` gives `"007"`). Missing keys are `nil`. Prefer `param`. |
| `random()`, `random(max)`, `random(min, max)` | Seeded uniform floats ([procedural.md](procedural.md#randomness)) |
| `pick(list)` | A seeded random element |
| `linear`, `radial` | Gradients (above) |
| `grid`, `hexGrid`, `sample`, `rasterize` | [procedural.md](procedural.md) |
| `outline(Text {...})` | Glyph outlines as SVG path data for a `Path`: stroke it with `fill = "none"`, mask with it, or join it to a mark ([procedural.md](procedural.md#type-as-geometry-outline)) |
| `measure(Text {...})` | Where the text's lines and ink land (below) |
| `effect { ... }` | Define a shader effect ([effects.md](effects.md)) |
| `Halftone Duotone Posterize Grain Displace Speckle Rough` | Built-in effects |
| `require("lib/x")` | Loads `lib/x.luau` relative to the file calling `require`, once per run, and returns its value |

### `measure`

`measure(Text { ... })` lays the text out exactly as drawing it would (same
`x`, `y`, `w`, `anchor`, `align`, font props) and returns doc px in the
text's own coordinates:

| Field | |
| --- | --- |
| `x`, `y`, `w`, `h` | Advance box: from the leftmost line start to the rightmost line end, and from the top of the first line box to the bottom of the last. With `anchor = "end"`, `x + w` is the text's `x`. |
| `baseline` | The first line's baseline (the text's `y`) |
| `ascent`, `descent` | The first line's font ascent and descent, both positive |
| `lines` | `{ { x, w, baseline }, ... }`, one per laid-out line |
| `ink` | `{ x, y, w, h }`: tight bounds of the glyph outlines. `nil` when nothing is inked (only spaces). |

The text's own trailing spaces count toward `w` (as `anchor` places them);
the space a wrapped line breaks at doesn't. The element passed in is not
drawn, so you can measure first and then edit and return the same table:

```lua
local t = Text { x = 24, y = 40, size = 15, "Product" }
local m = measure(t)
Rect { x = m.x - 8, y = m.y, w = m.w + 16, h = m.h, r = 6, fill = "#222" }  -- pill that fits
Text { x = m.x + m.w + 12, y = 40, size = 15, "Insights" }                 -- chain the next piece
```

**Sandbox**: only Luau's safe standard libraries are available. There's no
`io` or `os`, and no `loadstring`, `getfenv` or `setfenv`. Globals are
read-only. Relative paths (`require`, `Image` `src`, `fontFile`) resolve
from the directory of the file they're written in, so a shared helper in
`lib/` finds its own files whichever script requires it. A path a helper
receives as an argument resolves from the helper, so pass it already
joined. Every path must stay under the project root (`--root`, which
defaults to the entry script's directory). A run is limited to 2 GiB of memory and
`--timeout` seconds (default 120, including `rasterize` calls).
