# prisma docs

Prisma renders graphic design written as Luau scripts. A script returns a
`Canvas { ... }` tree. Prisma turns that tree into an IR and renders it on the
GPU (vello + wgpu) to a PNG.

These docs are written for coding agents. They are dense, and every default
and error message comes from the source, so you can act on a page without
reading the Rust.

| If you want to… | Read |
| --- | --- |
| Write or fix a design script | [scripting.md](scripting.md): elements, props, defaults, units |
| Build a logo or mark: holes, compound shapes, outline-only views | [scripting.md](scripting.md#holes-compound-shapes-and-outlines): one `Path`, `fillRule = "evenodd"`, `fill = "none"` |
| Write a pixel shader effect | [effects.md](effects.md): `effect { ... }`, WGSL helpers, built-in effects |
| Bend content into a shape: type on a path, patterns in a silhouette | [scripting.md](scripting.md#warp): `Warp`, `edgePatch`, `along`, `bezierPatch`, `warpPath` |
| Drive shapes from pixels (halftones, sampling, type as paths, distance fields) | [procedural.md](procedural.md): `rasterize`, `sample`, grids, `outline`, `Sdf`, randomness |
| Animate: motion, particles, simulations, video output | [animation.md](animation.md): `Animation { fps, duration, init, step, frame }`, `rng`, checkpoints |
| Render, iterate, debug a render | [cli.md](cli.md): flags and the debugging loop |
| Avoid the known traps | [pitfalls.md](pitfalls.md) |
| Change prisma itself | [architecture.md](architecture.md): pipeline, module map, adding features, tests |

## 30-second start

```sh
cargo build --release
target/release/prisma examples/basics.luau -o out/basics.png
```

```lua
-- out/hello.luau
return Canvas { size = { 400, 200 }, bg = "#f3efe6",
	Rect { x = 20, y = 20, w = 160, h = 160, r = 16, fill = "#e8402a" },
	Text { x = 200, y = 110, size = 40, font = "Helvetica", weight = 700, "hello" },
}
```

```sh
target/release/prisma out/hello.luau            # writes out/hello.png
target/release/prisma out/hello.luau --watch    # re-render on save
```

Then **look at the PNG** with your image-reading tool. Prisma doesn't check
that a design looks right. Only the render can tell you.

## The examples are the best reference

| Example | Shows |
| --- | --- |
| `examples/basics.luau` | Every primitive once: gradients, strokes, dash, blend, mask, blur, effect, text |
| `examples/poster.luau` | Grid layout, seeded randomness, stacked built-in effects (`Grain`, `Rough`, `Displace`), rotated text |
| `examples/halftone.luau` | The same image halftoned as a shader and as vector dots via `rasterize` + `sample` |
| `examples/a-grid.luau` | One glyph rasterized once, redrawn through vector "brushes"; `param` |
| `examples/dracula.luau` | A custom effect (`Scatter`) over blurred shapes; recreating a real poster |
| `examples/outline.luau` | `outline()`: glyphs as a `Path` to stroke and mask with |
| `examples/sdf.luau` | `Sdf` distance fields + a custom effect that reads them; `Path { points, smooth }` |
| `examples/field.luau` | An `"sdf"` effect input: shading from exact normals, contour lines from distance, a shadow from `body_march` |
| `examples/type.luau` | Baseline-aligned type at mixed sizes, a gradient through text, a full-bleed effect via `area` |
| `examples/transform.luau` | `measure()`-driven layout (chained breadcrumb, pills fit to labels) placed with a sheared `transform` |
| `examples/logo.luau` | A mark as one `Path` with `fillRule = "evenodd"` (holes without masks), and its construction drawn from the same path as outlines |
| `examples/warp.luau` | `Warp` through each kind of map: a Coons ribbon, type `along` a circle (fit with `measure`), a `bezierPatch` flag with `strokes = "warp"`, a function |
| `examples/boids.luau` | An `Animation` with a simulation: a flock in `step`, drawn as birds whose wings flap at a phase keyed by `rng` |
| `examples/refract.luau` | Glass from parts: `Refract` and `Gloss` composed into a lens, water (a `Height` input drawn with noise) and bubbles in frosted glass |
