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
| Write a pixel shader effect | [effects.md](effects.md): `effect { ... }`, WGSL helpers, built-in effects |
| Drive shapes from pixels (halftones, sampling, type as paths, distance fields) | [procedural.md](procedural.md): `rasterize`, `sample`, grids, `outline`, `Sdf`, randomness |
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
