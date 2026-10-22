# Animation

A script that returns `Animation { ... }` instead of a `Canvas` is an
animation. Its top level runs once, as setup. Then `frame` is called once per
frame and returns a `Canvas`. Optional `init` and `step` run a simulation
whose state the engine owns.

```lua
local mark = outline(Text { size = 120, font = "Helvetica", weight = 700, "hi" })  -- setup: once

return Animation { fps = 30, duration = "3s",
	frame = function(s, t, f)      -- s is nil without init/step
		return Canvas { size = { 400, 300 }, bg = "#f3efe6",
			Path { d = mark, x = 60 + 80 * t, y = 200 },
		}
	end,
}
```

`examples/boids.luau` is a full simulation: a flock steering around obstacles.

## `Animation` props

| Prop | Default | |
| --- | --- | --- |
| `fps` | required | Whole frames per second, 1–240 |
| `duration` | required | Seconds (`4`), or `"4s"`, `"500ms"`, `"90f"` (frames). It must land on a whole frame: `0.1` at 24 fps is an error. |
| `frame` | required | `function(s, t, f)` returning a `Canvas`. `t` = `f / fps` seconds, and `f` is the 0-based frame index. Every frame's Canvas must be the same size. |
| `init` | none | `function()` returning the state table `s` at frame 0 |
| `step` | none | `function(s, dt, n)`: advances `s` **in place** and returns nothing. `dt = 1 / (fps * substeps)`, and `n` is the 0-based global step index. |
| `substeps` | `1` | Steps per frame (1–1000). Frame `f` sees the state after `f * substeps` steps. |

`init` and `step` go together. Give both or neither.

## The contract

A frame is a function of (seed, params, frame index). Rendering frame 90
alone, as part of a sequence, or from a cached checkpoint gives the same
pixels. For that to hold:

- **All simulation state lives in `s`.** `step` changes only `s`. `frame`
  changes nothing. State kept anywhere else (upvalues, globals, tables made
  during setup and mutated later) is **undefined behaviour**. Prisma doesn't
  detect it. A typical symptom is a frame that changes when you clear the
  cache, or when you render it alone.
- **`s` is plain data**: tables, numbers, strings and booleans. A function, an
  `rng(...)` or a raster inside it is an error that names the key
  (`s.boids[3].go is a function`). Cycles are errors. A table referenced from
  two places in `s` becomes two copies after a checkpoint is restored, and
  metatables are dropped.
- **Don't depend on `pairs` order.** A restored table may iterate its
  non-array keys in another order. Iterate lists with `for i, b in list`
  (or `ipairs`), which is in order.
- **Randomness is keyed, not sequential.** Use `rng(key, ...)` (below).
  `math.random`, `random()` and `pick()` are reseeded before each `init`,
  `step` and `frame` call from (seed, which call, n or f). They're
  deterministic, but each call starts a new stream.

`--timeout` applies to setup and to each `init`, `step` and `frame` call
separately.

## `rng(key, ...)`

A random stream fixed by `--seed` and its keys (numbers, strings or
booleans). The same keys give the same numbers, whatever order they're asked
for in.

| Method | |
| --- | --- |
| `r:float()` | [0, 1) |
| `r:range(a, b)` | [a, b) |
| `r:int(a, b)` | A whole number from a to b, both included |
| `r:normal()` | Mean 0, standard deviation 1 |

Key by identity, not by time: `rng("spawn", i)` for a particle's fixed
traits, and `rng("wander", n, i)` for per-step jitter. Without a
simulation, particles can be computed in closed form: particle `i` is born
at `i / rate`, keyed `rng(i)`, and placed by its age `t - born`.

## Easing

Curves that run from 0 to 1 as `k` goes from 0 to 1. `k` is clamped, so a
time window needs no helper: `easeOut((t - start) / length)`. Map the result
onto a value with `math.lerp(a, b, e)`.

| Function | |
| --- | --- |
| `easeIn(k, p)` | `k ^ p`: starts slow. `p` defaults to 3 (cubic); 2 is quad, 5 quint, 1 linear. |
| `easeOut(k, p)` | `1 - (1 - k) ^ p`: stops slow |
| `easeInOut(k, p)` | `easeIn` over the first half, `easeOut` over the second |
| `easeSpring(k, swings, damping)` | Overshoots, then settles. It crosses 1 `swings + 1` times (default 2), the last at `k = 1`, so it ends exactly on the target. `damping` (default 4) shrinks the overshoot: about 23% at the defaults. |

```lua
local x = math.lerp(80, 600, easeInOut((t - 0.5) / 1.2))   -- slides between 0.5s and 1.7s
local s = 0.4 + 0.6 * easeSpring((t - 2) / 0.8)            -- pops in at 2s, landing at 2.8s
```

Anything else (a chain of segments, a custom curve) is ordinary Lua around
these and `math.clamp`, `math.lerp` and `math.map`.

## Checkpoints

The state is serialized every 30 frames, into memory and into
`.prisma-cache/sim-<key>/`. The key covers the script and every file it read
during setup, the seed, the params, `fps` and `substeps`. Rendering frame N
restores the nearest checkpoint at or before N and steps forward from it, so
`--frame 200` after a previous run costs at most 29 frames of steps. Editing
the script makes a new key. `--no-cache` skips the disk, but not memory.

## Rendering

See [cli.md](cli.md#animation) for flags. In short:

```sh
prisma anim.luau --frame 90                      # one frame as a PNG (the usual loop)
prisma anim.luau --time 1.5s
prisma anim.luau --sweep frame=0..240/30         # a contact sheet over time: how to "watch" it
prisma anim.luau                                 # every frame as an APNG, anim.png
prisma anim.luau -o out/frames/                  # PNG sequence: 0000.png, 0001.png, ...
prisma anim.luau -o out/anim.mp4                 # .mp4 .webm .mov .gif need ffmpeg on PATH
```

To check motion, look at frames, not videos: a frame sweep, an onion skin, or
`--frame` with `--crop`. Video is for the finished piece.

## Onion skin

`--onion 130..150/4` (or `--onion 130,140,150`) renders those frames and
overlays them into one PNG. The **last** frame listed is drawn exactly. Each
earlier one is a ghost, drawn at 70% opacity for the frame before the last
and fading linearly with age. Older ghosts go underneath newer ones.

A ghost is where its frame differs from the still background, and it shows
only where the last frame shows that background. The still background is the
per-pixel median of all the listed frames, so `--onion` needs at least 3
frames. Anything that stays put in more than half of them counts as
background. Text that changes every frame (a frame counter) ghosts like
anything else.

It works with `--crop`, `--scale` and sweeps: `--sweep seed=1..4 --onion
130..150/4` draws one onion per seed. It can't be combined with a `frame=`
sweep, `--frame`, `--time`, `--frames` or `--emit-ir`.

| `-o` | Written by | Alpha | Notes |
| --- | --- | --- | --- |
| `DIR/` | prisma | yes | Lossless. Files are named by frame index. |
| `.png` | prisma (APNG) | yes | Lossless, loops forever, large. Image viewers that don't animate show frame 0. |
| `.mp4` | ffmpeg, H.264 | no | Needs an even width and height. 4:2:0 chroma blurs thin coloured detail. |
| `.webm` | ffmpeg, VP9 | yes | Needs an even width and height. |
| `.mov` | ffmpeg, ProRes 4444 | yes | Near-lossless, very large: for editing. |
| `.gif` | ffmpeg | no | 256 colours. Gradients and grain band. |

Video output converts sRGB to YUV with the bt709 matrix and tags the stream
bt709, so players don't have to guess. A frame with any transparency is an
error for `.mp4` and `.gif`: give the Canvas an opaque `bg`, or write a format
that stores alpha. `--encode "ARGS"` replaces the default ffmpeg output
arguments, split on spaces.
