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
| `--sweep k=a,b,c` / `k=1..9` / `k=0..240/30` | — | Contact sheet over values (repeatable; the axes combine; `/s` steps a range). `seed` sweeps seeds, `frame` sweeps an Animation's frames. |
| `--cols N` | √n | Contact sheet columns |
| `--frame N` | — | Render one frame of an `Animation` (0-based) as a PNG |
| `--time T` | — | The frame at a time: seconds, or `"1.5s"`, `"500ms"`, `"45f"`. It must land on a frame. |
| `--frames A..B` | all | Render these frames (both ends included) to `-o` |
| `--onion FRAMES` | — | Overlay an Animation's frames (`a..b/s` or `a,b,c`, at least 3) into one PNG: the last exactly, earlier ones as ghosts fading with age ([animation.md](animation.md#onion-skin)) |
| `--encode ARGS` | — | ffmpeg output arguments replacing the defaults for `.mp4 .webm .mov .gif` |
| `--emit-ir` | — | Write the evaluated scene as JSON instead of rendering (an `Animation` needs `--frame`) |
| `--dump DIR` | — | Write every offscreen pass's input/output, plus every named `rasterize`, as PNGs |
| `--no-cache` | — | Don't read or write `.prisma-cache/` |
| `-w, --watch` | — | Re-render when the script or any file it read changes |
| `--profile` | — | Print GPU init, render size, offscreen pass count and timings to stderr |
| `--root DIR` | script's directory | The only directory scripts may read from. Relative paths still resolve from the file they're written in, so `--root ..` lets a script reach `../photo.jpg`. |
| `--timeout S` | `120` | Script wall-clock limit, including `rasterize` |
| `--tile N` | `2048` | The most device px on a side that one GPU submission draws (a multiple of 16). Bigger renders and passes are drawn in tiles. Smaller tiles are slower: about +15% at 2048 and +55% at 1024 on a 2400×3400 render. |

A successful run prints `wrote out/x.png (WxH, Nms; script Nms)`. An error
prints `error: file.luau:LINE: Element: message` and exits with status 1.

A render that outgrew vello's GPU buffers prints `warning: the scene
overflowed vello's GPU buffers: <buffer> (...) at WxH px, so it was rendered
in pieces (N splits)`, plus a hint on what to cut back. The output is still
correct. If pieces can't fit either, it's an error instead
([pitfalls.md](pitfalls.md#big-renders)).

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

## Animation

A script returning `Animation { ... }` ([animation.md](animation.md)) renders
every frame unless you pick one. `-o` chooses the output: a path ending in
`/` (or an existing directory) gets a PNG sequence named by frame index,
`.png` gets an APNG (the default, `<file>.png`), and `.mp4 .webm .mov .gif`
are encoded by `ffmpeg`, which must be on PATH. Without it, the error prints
the ffmpeg command to run on a PNG sequence yourself.

```sh
prisma examples/boids.luau --frame 120 --crop 300,200,200,150 --scale 3   # zoom into one frame
prisma examples/boids.luau --sweep frame=0..225/45 --cols 3              # motion at a glance
prisma examples/boids.luau --sweep seed=1..4 --frame 200                 # variations at one moment
prisma examples/boids.luau --onion 130..150/4                            # paths and headings in one image
prisma examples/boids.luau -o out/boids.mp4
```

A frame sweep plays the simulation once per seed and params, and visits
frames in the order given. `--dump DIR` writes each frame's passes to
`DIR/frame-NNNN/` when rendering several frames. `--crop`, `--scale` and `-p`
apply to every frame.

## Limits

- The output (or crop) can't exceed the GPU's max texture size. The error
  says so. Render in pieces with `--crop`.
- One image draws at most 8192 device px on a side, but any input size
  works when `crop`ped or drawn smaller ([scripting.md](scripting.md#image)).
  So a comparison script can zoom into a tall finished page.
