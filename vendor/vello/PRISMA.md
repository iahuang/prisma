# vendored vello 0.11.0

This is vello 0.11.0 from crates.io with one change, marked `PRISMA PATCH` in
`src/lib.rs`: `render_to_texture_async` always reads back vello's bump
allocator counters, not only when built with `debug_layers`.

vello sizes its GPU working buffers at fixed sizes and, when a scene outgrows
one, skips drawing and sets a `failed` flag that `render_to_texture` never
reports. Prisma needs that flag to fail loudly or split the render (see
`Gpu::render_scene` and `Pass::render_region`). The `debug_layers` feature
would expose it too, but it also copies the whole line buffer back on every
render (several ms each) and panics when that buffer overflowed.

Drop this directory and the `[patch.crates-io]` entry in the root
`Cargo.toml` once a vello release reports overflow on its own.
