# patches/

Local copies of the crates this workspace depends on that cannot be taken from
the fork as they are.

Two of them are `[patch.crates-io]` targets. Each keeps the upstream package name
and version, so it can stand in for the published crate, and its manifest
documents exactly how it differs from upstream:

- `gpui_unofficial/` — a shim that re-exports this fork's `gpui` under the
  published `gpui-unofficial` name, so ecosystem crates resolve to the fork
  instead of a second copy of gpui.
- `gpui_animotion/` — `gpui_animotion` 0.7.0 with its `Element` impl adapted to
  the fork's `Element` trait (no `inspector_id`).

These two are deliberately not workspace members: they are third-party code that
happens to live here, so the workspace's lints, formatting and test runs leave
them alone. See `corgi-patches/` for the same pattern.

`gpui_parley/` is the odd one out. It is not published, so there is nothing to
patch: it is copied here to be *changed*. It is the crate that shapes the
document's text, the app installs it with
`application().with_text_system(...)`, and it reads its embedded fonts from this
workspace's `assets/fonts/`. It is a plain path dependency and a member of the
workspace, so its own tests and its example stay runnable:

    cargo test -p gpui_parley                          # shaping, rasterization, its caches
    cargo test -p gpui_parley --features test-support  # ...and that it can be injected
    cargo run -p gpui_parley --example parley_demo --features demo
    cargo run --release -p gpui_parley --example vector_cost
    cargo run --release -p gpui_parley --example raster_cost
    cargo run --release -p gpui_parley --example frame_cost

The copy's own changes are listed in its manifest. The one worth knowing here is
that it loads the rest of `assets/fonts/` at runtime through `add_fonts`, and
shapes and rasterizes in the family it is asked for, out of the shaper's own font
database rather than a table of its own. That is what the app's `COOL_SCROLL_FONT`
names — see the app's README.

`raster_cost` is the one to reach for when the document is slow: it times a
screenful of lines the way the app's transform asks for them — a different font
size on every line, so nothing in either cache can be reused between frames —
and splits the cost into shaping, rasterization, and the parts of a raster. In
debug it reads:

    30 lines, 3480 glyphs, 840 distinct rasters   (one frame, before the cache)
      layout        31.0ms   (1.0ms a line)
      raster          1.2s   (1.4ms a glyph)

which is where its first change came from: rasterization is the whole frame, and
`HintingInstance::new` was 0.43ms of each 1.4ms raster because it was built once
per glyph rather than once per size. Caching it takes the raster half to ~0.4s
in the same debug build. The rest is the outline-to-mask work itself, which is
what a vector renderer would remove.

Its second half answers what to do about that, since the atlas keys a glyph on the
exact size it was cut at: it sweeps rounding the size a glyph is *rasterized* at,
over 200 frames of a flick whose sizes stay inside the range the transform covers,
with a cache that persists across them the way the renderer's does. The lines are
still shaped at the exact sizes they are drawn at — that is what puts them where
they are — and each row gets its own text system, so what a row spends laying text
out is its own and not a previous row's cache.

    ladder          atlas   misses/frm   layout/frm   raster/frm
    continuous      32508          154      666.6µs        7.9ms
    0.5px             644            0      504.4µs       83.5µs
    0.25px           1260            0      506.4µs       88.3µs
    0.125px          2548            0      555.9µs       95.5µs
    0.0625px         5068            0      457.7µs       79.9µs

Two things fall out of it. A lattice stops missing entirely once it is warm, and the
step does not matter: every one of those is the same ~85µs a frame against 7.9ms at
a continuous size — ninety times, for free. So the step is not a speed knob at all
but a smoothing one, and the only thing it costs is atlas entries, in proportion.
What it does not change is the layout, which stays at the exact size and is why the
app's `Mode::Text` still lays a line out again when its size has moved. The fork
does that rounding in `Window::glyph_raster_size`, at a quarter of a pixel.

`vector_cost` measures that alternative: the crate exposes `glyph_triangles`,
which tessellates a glyph's outline once (lyon, nonzero winding, so counters stay
holes) into triangles in em units. In release, a screenful costs ~0.7ms of CPU —
681µs of tessellation once, then 44µs a frame to scale and place 3,480 glyphs
into 218,322 vertices — against ~25ms to rasterize. What it costs instead is
vertex traffic: gpui expands a path vertex to 104 bytes, so that is ~23MB a
frame. The app's `Mode::Vector` is where that trade is actually spent — it draws
the document through `glyph_triangles` and `Window::paint_path`, and its readout
reports the triangles a frame came to. See the app's README for what it means.

`frame_cost` is the one to reach for when the *vector* mode is slow, because the
app's per-frame cost is more than the placement `vector_cost` measures: it walks
the whole route. For a screenful of 24 lines at 1000x700, in release and with its
caches warm, it reports about 1.1ms building the paths (a triangle at a time, with
the bounds tracked in plain floats — going through `Path::push_triangle`'s
`Bounds` union per corner costs another half a millisecond), 0.5ms for the
`Path::scale` that `Window::paint_path` does to every path, and 2.7ms for the
expansion into the renderer's 104-byte per-vertex records — 40MB moved a frame
all told, 2.3GB/s at 60fps. The scene-side half of that is 3MB of 16-byte
`PathVertex` (6MB of 32 before the fork's third commit dropped the per-corner
`ContentMask`), so the number left to attack is the expansion itself: the record
carries a `Background` and the path's bounds *per vertex*, where 16 bytes of
position and curve parameter would do.

Those features are the fork's, and they are load-bearing. `test-support` moves
gpui's frame drawing onto a path that draws every dirty window at the end of
every `App::update`, and a dev-dependency's features are unified into the example
build, so the example would render once per input event if `[dev-dependencies]`
were used to pull in `gpui`'s `test-support`. The app depends on this crate with
no features at all, and declares `test-support` as a feature of its own for the
same reason — see the comment on it in the root `Cargo.toml`.
