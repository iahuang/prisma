# CLI and the debugging loop

```
prisma <file.luau> [options]
```

Build once with `cargo build --release` and use `target/release/prisma`. The
debug build is much slower on the CPU side.

## Options

| Flag | Default | |
| --- | --- | --- |
| `-o, --out PATH` | `<file>.png` (`.json` with `--emit-ir`) | Output path; parent dirs are created |
| `--seed N` | `1` | Seed for `math.random` / `random` / `pick` |
| `--scale K` | `1` | Device px per doc px. `--scale 2` gives a 2× render. |
| `--dpi D` | — | Same as `--scale D/96`. Can't be combined with `--scale`. |
| `--crop x,y,w,h` | — | Render only this region, in **doc units** (`"400,120,240,300"` or `"10mm,10mm,50mm,30mm"`). The pixels match the same region of a full render, including noise and grain. |
| `-p, --param k=v` | — | Sets a param (repeatable), read with `param("k", default)`. The value stays a string until `param` reads it as the default's type. |
| `--sweep k=a,b,c` / `k=1..9` | — | Contact sheet over values (repeatable; the axes combine). `seed` sweeps seeds. |
| `--cols N` | √n | Contact sheet columns |
| `--emit-ir` | — | Write the evaluated scene as JSON instead of rendering |
| `--dump DIR` | — | Write every offscreen pass's input/output, plus every named `rasterize`, as PNGs |
| `--no-cache` | — | Don't read or write `.prisma-cache/` |
| `-w, --watch` | — | Re-render when the script or any file it read changes |
| `--profile` | — | Print GPU init, render size, offscreen pass count and timings to stderr |
| `--root DIR` | script's directory | The only directory scripts may read from. Relative paths still resolve from the script, so `--root ..` lets a script reach `../photo.jpg`. |
| `--timeout S` | `120` | Script wall-clock limit, including `rasterize` |

A successful run prints `wrote out/x.png (WxH, Nms; script Nms)`. An error
prints `error: file.luau:LINE: Element: message` and exits with status 1.

## The loop

1. **Render and look.** Read the PNG with your image tool after every
   meaningful change. A script that runs without errors can still be wrong.
2. **Zoom instead of squinting.** `--crop x,y,w,h --scale 3` renders one
   region at high resolution, cheaply. Use it for edges, small type and
   texture.
3. **Bisect wrong layers with `--dump DIR`.** It writes
   `pass-NNN-<file>_<line>-<label>.png` for every offscreen pass:
   - `blur-in` / `blur-out`
   - `<effect>-in` / `<effect>-out`
   - `mask-content` / `mask-shape` / `mask-out`
   - `sdf-in` / `sdf-out`
   - `<effect>-<input>-in` / `<effect>-<input>-field` (an input's shapes, and
     its float field shown as gray = 0.5 + d / (2·range))

   It also writes `<name>.png` for each `rasterize { name = ... }`. Find the
   first pass whose output looks wrong; `<line>` tells you which element made
   it.
4. **Diff structure with `--emit-ir`.** The JSON is the fully evaluated
   scene: components expanded, lengths in px, colors as straight RGBA 0–1.
   Diffing two IR dumps shows what a script change really did, without
   rendering.
5. **Compare to a reference** by writing a throwaway script that draws both
   images side by side, and render that:

   ```lua
   return Canvas { size = { 1200, 800 }, bg = "#888",
   	Image { src = "target.png", w = 590, h = 800, fit = "contain" },
   	Image { src = "out.png", x = 610, w = 590, h = 800, fit = "contain" },
   }
   ```

   Both images must be under the project root.

Scratch renders go in `out/` (gitignored).

## Contact sheets

```sh
prisma examples/poster.luau --sweep seed=1..9 -o out/seeds.png
prisma examples/halftone.luau --sweep cell=5,9,14 --sweep seed=1,2 --cols 3
```

Each variant runs in a fresh VM, sharing one GPU and one raster cache. Cells
are 360 px, labeled `k=v`. `--scale` and `--crop` apply to every cell.

## Limits

- The output (or crop) can't exceed the GPU's max texture size. The error
  says so. Render in pieces with `--crop`.
- Input images are limited to 8192 px per side.
