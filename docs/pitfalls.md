# Pitfalls

These come from reading the source and probing the CLI. Each one has
surprised someone.

## Text

- `y` is the first line's **baseline**. Descenders hang below it, and a
  line's top sits about 0.7 × size above it for caps.
- **`align` needs a box, `anchor` places a line.** With `w`, `align` places
  lines inside `[x, x + w]`. Without `w`, use `anchor = "end", x = 760` to
  make the line *end* at 760. Using either one in the other's case is an error.
- **`tracking` is in em**, so `tracking = 2` means 2 × size. Typical values
  are −0.05 to 0.2.
- To place things next to text, use `measure(Text {...})`, not a guessed
  width. `w` is the advance (what the next run should start after), `ink` the
  tight glyph bounds (what an underline or highlight should hug).
- `Text` has no `stroke`. Use `Path { d = outline(Text {...}), stroke = ... }`.
- `font` is a **family** name only. `"Courier New Bold"` is an error that
  tells you to write `font = "Courier New", weight = 700`. Font files go in
  `fontFile`. Font names fail loudly, and there's no silent fallback.

## Paint and shapes

- **Shapes are filled black unless you say `fill = "none"`**, even when they
  have a `stroke`. An outline-only shape needs `fill = "none"`. (`Line` is
  never filled, and giving it a `fill` is an error.)
- **Holes don't need masks.** Put the outer and inner contours in one
  `Path` and set `fillRule = "evenodd"`. The same `d` with `fill = "none"`
  and a `stroke` draws its outlines, so a construction diagram comes from
  the mark itself (`examples/logo.luau`).
- `opacity` fades the element as a whole. A shape with both fill and stroke
  at `opacity = 0.5` shows its stroke at half strength over the background,
  not over its own fill. Out-of-range `opacity` (or negative `blur`) is an
  error, not clamped.
- The default `join` is `round`, not `miter`.
- Gradient coordinates are absolute in the shape's coordinate space. To make
  a gradient follow a shape, compute its endpoints from the same numbers as
  the shape, or wrap the shape in a `Group { x, y }` and draw both at the
  origin.
- The only color keywords are `none transparent black white`. `"red"` is an
  error.
- `rgba()` alpha is 0–1, while its r, g and b are 0–255.

## Groups, masks, opacity, blend

- `blend` composites against what's already drawn **inside the same parent**.
  A `Group` with `opacity < 1` or a non-normal blend is a separate layer, so
  children blending inside it don't see what's under the group.
- `Group.clip` uses the group's local coordinates, and the clip is
  transformed with the group.
- `transform = { a, b, c, d, e, f }` is the **whole** placement of a `Group`
  or `Path`. Combining it with `x`, `y`, `rotate` or `scale` is an error;
  fold them into the matrix. `(e, f)` is the translation, `(a, b)` and
  `(c, d)` are the images of the local x and y axes (SVG order, not row-major).
- `blur` and effect lengths under an anisotropic `transform` or `scale` use the
  geometric mean of the axes, so a blur stays round in device space.
- `scale = { 3, 1 }` on a stroked `Path` or `Group` stretches the stroke too
  (hairlines turn into slabs on one axis). Bake the stretch into the path
  with `transformPath(d, { scale = { 3, 1 } })` to keep strokes even.
- Children of a `Group` rotate and scale around the group's `(x, y)`, not
  their own centers. To rotate a shape about its center, put the shape at
  the origin inside a group positioned at the center.

## Warp

- **Patches need `from`; functions refuse it.** A patch's domain is the unit
  square, so `from` names the rect of your content that fills it. A function
  already maps doc px.
- **Content outside `from` is an error by default**, naming the point.
  Beads or caps that poke past the last row or column are the usual cause:
  grow `from`, or pass `outside = "extend"`. Stroke overhang doesn't count.
- **`strokeWidth` under `strokes = "after"` (the default) is doc px of the
  result**, not of the content's units. Content drawn in a 27 × 5 grid
  still takes `strokeWidth = 0.6` for a hairline.
- Images, gradients, `Group.clip` and effect `area` can't be warped; each is
  an error saying what to use instead.
- A map function runs for many points per curve, during conversion. It
  can't call `rasterize` or define effects; do that before the `Warp`.

## Big renders

- An offscreen pass covers its output plus every pixel it reads: an
  effect's `reach`, 3σ of blur, an `Sdf`'s `range`, all in device px (so
  times `--scale`). A small `--crop` doesn't make that small. A 500 px
  `reach` at `--scale 6` reads 3000 px around even a 60 × 40 crop, about
  6000 × 6000 px of work. Reads stop at the canvas's `margin`, so a big
  `reach` mostly costs what's on (or near) the canvas.

- vello (the rasterizer) has fixed-size GPU buffers. A scene that outgrows
  one renders blank, so prisma checks every render, and when one
  overflowed renders it again in pieces, with a `warning:` naming the
  buffer. The pieces match a whole render pixel for pixel; they're just
  slower. Overflow comes from area times count: many large shapes, layers
  or clips (a full-page shape repeated hundreds of times), or masks,
  `opacity` and `blend` layers nested more than 4 deep over a large area.
- Too many **line segments** (from every path in the scene, after curves
  are flattened) is an error, since pieces can't help: each piece still
  flattens the whole scene. Thousands of detailed paths or long texts set
  as `outline()` paths can hit it.
- vello draws an image (or an offscreen pass's output) at most 8192 px on
  a side. Prisma tiles pass outputs and crops/downsamples big images, but
  all the images one scene draws share an 8192 × 8192 atlas.

## Effects

- **By default an effect only draws over its children's bounds (plus
  `grow`).** For a full-canvas effect, set `area = { 0, 0, W, H }`.
- If an effect reads `src` at an offset (displace, scatter, blur-like), set
  `reach`; otherwise its edges read transparency. If it also moves or
  spreads ink past its children, set `grow` too. `pad` is gone: `pad = x`
  was `reach = x, grow = x`.
- `src()`, color uniforms and the return value are all **premultiplied**.
  Doing color math on premultiplied values darkens semi-transparent edges.
  Use `unpm` / `pm`.
- `p` is in absolute device px. Spatial constants written in pixels change
  with `--scale`, so use `length` uniforms, or work in doc px with
  `to_doc(p)` / `src_doc(q)`.
- An effect over an `Sdf` reads the field from alpha. Anything opaque under
  the `Sdf` in the same effect corrupts it. An `"sdf"` input slot
  (`body = Sdf { ... }`) avoids this and is exact; prefer it.
- An input slot takes exactly its element: `body = Sdf { range = 40, shape }`,
  never a bare `shape`. Prisma doesn't wrap it for you.
- `NAME_march` reads up to `maxd` away. Set `reach` to at least `maxd`, or
  crops won't match full renders. A march that starts within a fraction of a
  pixel of the contour may treat the start as the other side and return ~0.
- A `Height` is **white = high**. Shapes are filled black by default, which is
  height 0, so a bump needs `fill = "#fff"` (or a gradient). Its slope comes
  from an 8-bit render: give it a `blur` of a couple of px, more for gentle
  ramps, or the surface shows steps.
- `Refract` and `Gloss` each build the surface from their own `thickness`,
  `bevel`, `curve`, `body` and `surface`. If a `Gloss` doesn't get the same
  values, its highlights sit on a different surface. Keep them in one table.
- `Refract` reads its children where the light bends to, which can be well
  outside the glass. Past the backdrop (beyond an image's edge, off the
  canvas) it reads transparency, so the glass shows holes there. Make the
  backdrop reach past it, or keep the glass away from the edge.

## Params

- `params.k` is always the **string** typed after `-p k=`. Use
  `param("k", default)` to get it as the default's type. It errors on
  `-p cell=abc` when the default is a number. Arithmetic on a raw
  `params.cell` relies on Luau's own string coercion, so don't.

## Files and paths

- Relative paths resolve from the file they're written in: a `require`d
  helper's `require`s, images and fonts resolve from the helper's directory.
  A path passed *into* a helper as an argument resolves from the helper too,
  so pass it already joined. A path that leaves `--root` (default: the entry
  script's directory) is an error, so pass `--root ..` to reach
  `"../photo.jpg"`.
- Image natural size is 1 image px = 1 doc px. A 4000 px photo dropped in
  without `w`/`h` covers a 600 px poster many times over.
- A `rasterize` result is cached on disk by content and by the mtimes of
  files it reads. If you change something it can't see (only possible when
  developing prisma itself), use `--no-cache` or bump `CACHE_VERSION`.

## Script structure

- `Canvas` must be the returned root. To nest one, `rasterize` it and draw
  the raster with `Image`.
- Strings directly in children are an error (`stray text "..."`). Wrap them
  in `Text`.
- A typo'd prop is an error with a suggestion, never a silent no-op. Read
  the message.
- The script has no `io` or `os`. Compute everything in Lua, or pass values
  in with `-p`.
