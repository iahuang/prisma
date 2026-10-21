# Architecture (for changing prisma)

```
.luau ──(script.rs: Luau VM + prelude)──► element tables
      ──(script.rs: Conv)───────────────► IR (ir.rs, serde-serializable)
      ──(render.rs: Pass)───────────────► vello scenes + offscreen GPU passes (gpu.rs)
      ──(engine.rs)─────────────────────► RGBA8 ──► PNG
```

## Module map

| File | Owns |
| --- | --- |
| `src/main.rs` | CLI (clap), watch loop, `--sweep` contact sheets, `--crop`/`--scale` → device rect |
| `src/script.rs` | Luau sandbox; installs globals (`PRIMITIVES` constructors, `params`, `param`, `effect`, `rasterize`, `outline`, `measure`, `require`); `Props` (prop reading + unknown-prop errors with edit-distance hints); `Conv` (tables → IR) |
| `src/prelude.luau` | Lua-side helpers (`random`, `pick`, `linear`, `radial`, `grid`, `hexGrid`, `sample`) and the built-in effects, written as user code |
| `src/ir.rs` | `Document`, `Node { kind, opacity, blend, blur, at }`, `Kind`, `Geom`, `Paint`, ... Everything in doc px and straight RGBA. |
| `src/units.rs` | `parse_length` (px pt mm cm in), `parse_color` |
| `src/render.rs` | IR → pixels: encodes nodes into vello scenes in absolute device coords; decides which nodes need offscreen passes; computes bounds; overflow retry and splitting; atlas-sized images and tiles; `--dump` |
| `src/gpu.rs` | wgpu device, vello renderer, the fixed pipelines (separable blur, down/up sampling, mask, jump-flood SDF), effect pipeline cache, readback, overflow checks (`Overflow`, `BumpCheck`, `Gpu::check`) |
| `vendor/vello/` | vello 0.11 with one addition, `render_to_texture_with_bump`, so overflow can be detected (`vendor/vello/PRISMA.md`) |
| `src/effects.rs` | `EffectDef`: uniform types, packing (colors first, then f32s), `reach`/`grow`, the WGSL prelude (`src`, `pm`, `hash`, `fbm`, ...), naga validation |
| `src/text.rs` | Font resolution (an exact system family or a font file; face words only feed the not-found hint), parley layout, skrifa glyph outlines |
| `src/engine.rs` | State that outlives a run: GPU, fonts, decoded images, rasters, the effect registry (cleared at the start of each run); path resolution (`resolve`: relative to `script_dir`, confined to `base_dir` = `--root`; `script.rs` first rewrites a path as written in a module to one from `script_dir`, via the chunk name); `rasterize` memoization (memory + `.prisma-cache/`) |
| `tests/examples.rs` | Renders every example through the binary and checks pixels |

## Key invariants

- **Nothing is inferred.** A value is never converted to another type behind
  the script's back (params stay strings until `param` types them), a default
  never depends on another prop (`fill` is black whether or not there's a
  `stroke`), a prop means one thing for every element kind (`opacity`), and
  out-of-range values are errors, not clamps. When a guess would be
  convenient, make it an error message that names the explicit form.

- **Device space is absolute.** Every scene is encoded in canvas device
  pixels. An offscreen pass renders only its node's device bounds,
  intersected with the parent's region of interest. Effects receive absolute
  `p`. Together these make `--crop` output match a full render pixel for
  pixel, which `crop_matches_full_render` tests. Anything new that's
  position-dependent (noise, fields) must keep this. Passes that read
  neighbors expand their input rect (effect `reach`, blur 3σ, SDF `range`)
  through `Pass::read_rect`. It clips to the canvas grown by its `margin`,
  which is absolute, so crops still match.
- **No GPU submission draws more than `Gpu::tile` px on a side.**
  `render_region` renders vello scenes in tiles and stitches them, and
  `Gpu::run_pass` draws each pass as scissored tiles, each its own submit.
  The pass pipelines don't blend, so scissored tiles write exactly the same
  pixels. This keeps each submission's work bounded however big a pass gets.
- **Fields are data, not pictures.** An effect's `"sdf"` inputs are rendered
  (curves pre-flattened to `FIELD_TOLERANCE`), jump-flooded, and written as
  `Rgba32Float` `(d, q - p, coverage)` covering the effect's input rect.
  `"height"` inputs are rendered normally, then turned into floats
  (luminance x alpha x scale), blurred by the `Height`'s `blur` in float and
  differenced into `(h, dh/dx, dh/dy, alpha)` (`HEIGHT_WGSL`, `Gpu::height_field`).
  Inputs are bound as textures 3, 4, ... in `EffectDef::inputs` order and never
  composited through vello, since anything vello composites is RGBA8. An
  optional input that was left out is bound as a 1x1 texture of its kind's
  empty value (`Gpu::absent_input`); accessors bound their reads by
  `textureDimensions`, so every read of it is that value. The effect bind
  group layout depends on the number of inputs (`Gpu::effect_layout`). Each
  `"sdf"` input's range rides at the end of the `u` buffer
  (`prisma_range_NAME`; `ABSENT_RANGE` for a left-out one).
- **The jump flood** (`SDF_WGSL` in `gpu.rs`) seeds partly covered contour
  pixels at a subpixel edge point (Gustavson & Strand's coverage-to-distance).
  A refine pass fits each seed's edge as the principal axis of its
  neighbours' points, which gives the direction and a denoised position.
  Seeds act as ±0.5 px pieces of that edge, shrinking to points at corners.
  The visual `Sdf` and inputs share it.
- **Alpha conventions.** vello writes straight-alpha RGBA8. Offscreen pass
  outputs are premultiplied and handed back to vello as
  `AlphaPremultiplied` images. Effects see premultiplied values. Rasters and
  PNGs are straight.
- **Offscreen passes** happen for: `blur > 0`, every `Effect`, every `Sdf`,
  and `Mask { luminance = true }` (`needs_pass` in `render.rs`). Alpha and
  inverted masks stay inside vello as `DestIn`/`DestOut` layers. Opacity and
  blend on containers are vello layers clipped to tight bounds.
- **Bounds** (`Pass::bounds`) must be conservative for every `Kind`. A new
  kind that reports bounds too small gets clipped silently. Mask layers are
  clipped to them too.
- **vello's buffers are fixed-size**, and an overflowing scene renders blank
  with no error from vello. Every vello render goes through
  `Gpu::render_scene`, which also downloads vello's counters without waiting.
  `Pass::document` checks them all once at the end (`Gpu::check`). On an
  overflow, `render_document` runs the whole pass again with `checked`, which
  checks each render as it's made and splits an overflowing region in halves
  (`Pass::render_region`, down to `MIN_SPLIT`), with a warning. Checking
  every render up front would stall the GPU each time. Line-segment overflow
  can't be split (vello flattens the whole scene for any viewport), so it's
  an error. Keep layers tight: a layer costs a blend level in every 16 px
  tile it covers, and past 4 nested levels each tile spills to a buffer with
  room for 4096.
- **vello's image atlas is at most 8192 px** (`ATLAS_MAX`), and vello skips an
  image that doesn't fit without a word. Draw pass textures with
  `Pass::draw_texture` (it tiles big ones). File images over the limit go
  through `atlas_fit`, which uploads only the drawn source rect, resampled
  to device resolution.
- **IR is the cache key.** `Engine::rasterize` hashes `CACHE_VERSION`, the
  dpi, the IR JSON and external stamps (image/font mtimes, effect module
  source). **Bump `CACHE_VERSION` in `engine.rs` whenever rendering output
  changes for the same IR**, or stale rasters persist in users'
  `.prisma-cache/`.
- **vello image ids**: each scene re-ids the images it draws (`Pass::fresh`),
  because vello's atlas can serve stale pixels across renders when ids are
  reused.
- **Errors carry script locations.** Constructors record `_at` ("file:line").
  `Props::err` prefixes it, and render errors are wrapped with `Node::at`.
  Keep new errors in the form `file.luau:LINE: Element: what's wrong (how to
  fix it)`.

## Adding an element

1. Add the name to `PRIMITIVES` in `script.rs`.
2. Add an IR variant in `ir.rs` (`Kind` or `Geom`). Update `Node::walk` if it
   has children, and `Engine::stamps` if it reads files.
3. Convert it in `Conv::node`. Read every prop through `Props` (`len_or`,
   `req_len`, `paint`, `choice`, ...) so that unknown-prop detection and
   error locations work. End with `p.finish()`.
4. Render it in `Pass::encode_kind`, or add it to `needs_pass` and
   `Pass::pass` if it needs its own pixels.
5. Give it bounds in `Pass::bounds`.
6. Document it in `docs/scripting.md`.
7. Add an example and a pixel check (see Tests).

## Adding a built-in effect

Write it in `src/prelude.luau` with `effect { ... }`, exactly as a user
would. If several effects need a WGSL helper, add it to `PRELUDE` in
`effects.rs`. Document both in `docs/effects.md`.

## Tests

```sh
cargo test --release
```

- Unit tests: units parsing, `through` (spline passes through its points),
  and effect compilation (including correct user line numbers in errors).
- `tests/examples.rs` runs the real binary on the GPU with `--no-cache`. It
  checks that every example renders (`all_examples_render`), samples exact
  pixels (`basics_pixels`, `sdf_pixels` at scales 1 and 2), measures `"sdf"`
  input precision against an exact polygon (`sdf_input_is_exact`), checks that
  input slots reject anything but an `Sdf` (`sdf_inputs_are_explicit`), measures
  `"height"` inputs against an exact ramp, with an absent optional input
  (`height_input_is_exact`), checks their errors (`height_inputs_are_explicit`),
  checks `Refract` offsets against Snell's law and `Gloss` against Blinn
  (`refraction_follows_snell`, `tests/fixtures/refract_probe.luau`), checks a
  sheared `transform` (`transform_pixels`) and `measure` against layout
  (`measure_matches_layout`, `tests/fixtures/measure.luau`), and checks crop
  invariance (`crop_matches_full_render`, tolerance 2/255). Even-odd fills
  and outlines are checked in `logo_pixels`. vello buffer overflow (bounded mask layers, a
  split render matching a crop, the line-segment error) is checked against
  `tests/fixtures/overflow.luau`, and images and passes over the atlas limit
  against `tests/fixtures/big.luau`.
- **For a new feature, add an example to `examples/` and add it to the list
  in `all_examples_render`.** Then add a pixel test that checks the feature's
  actual math at known coordinates, and a crop check if the feature is
  position-dependent.
- Behaviour that isn't an example (sweeps, `--root`) gets a script in
  `tests/fixtures/`, rendered with `render_script` or run with `prisma`.
- Tests run in parallel, and each render writes its own file under
  `CARGO_TARGET_TMPDIR`.

## Perf notes

- `--profile` prints GPU init time, offscreen pass count and render time.
- Offscreen passes dominate the cost. Many small blurred or effected nodes
  cost more than one large one, since each is a separate texture and pass.
- Before benchmarking, check thermal state (`pmset -g therm`), and make sure
  the machine is idle and on power.
