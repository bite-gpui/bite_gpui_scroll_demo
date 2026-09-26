# scroll-demo against the published crates — 2026-09-26

The app moved off the fork checkout and onto the crates.io artefact. This is
what that cost, what it broke, and the numbers the three cost examples produce
afterwards.

```sh
# from this repository's root
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo check --all-targets
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --features test-support
```

The text system's own tests and the three cost examples were run here with `-p
gpui_parley`, because at the time that crate was a member of this workspace. It is
not any more — see below — and those commands are run in
[`bite-gpui/gpui_parley`](https://github.com/bite-gpui/gpui_parley) now, which
records their output.

## Since this was written

A dated record, so the body below is the state on 2026-09-26 and is left as it was
measured. Three things have changed since:

- **The text system's copy became a crate.** `patches/gpui_parley` — the vendored
  copy this run measured — was extracted to
  [`bite-gpui/gpui_parley`](https://github.com/bite-gpui/gpui_parley), given a
  publisher of its own, and published to crates.io as `bite-gp-parley 1.21.1` on
  2026-09-26. Its sources are byte-identical to the copy measured here; only the
  package name, the version and the manifest changed on the way.
- **The app takes it by version.** `patches/gpui_parley` is deleted and the
  dependency is `bite-gp-parley = "1.21"` — the same name the layer stack published,
  which the crate's repository continues. `use gpui_parley::…` in `src/` is
  unchanged: cargo exposes a dependency under its library name.
- **Which makes two passages below historical rather than wrong-at-the-time:** the
  paragraph at the end of *The delta* that says the text system is “still the app's
  own copy”, and the last two bullets under *Not established here*. The published
  crate *is* the app's text system now, and none of these edits are uncommitted.
  For the same reason the `parley-demo/` named in *The delta* is a sibling project
  in the usage suite this record was written in, not a directory of this
  repository.

So the numbers below stand as measured — the text system's sources did not change —
while the arrangement around them did. Two of them are worth carrying forward: the
app is green on the published crates, and the `calloop` substitution holds.

| | |
| --- | --- |
| toolchain | `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1` — the app's own `rust-toolchain.toml` |
| crates | `bite-gpui`, `bite-gp-engine`, `bite-gp-runtime`, `bite-gp-linux`, `bite-gp-wgpu`, `bite-gp-types`, `bite-gp-authoring`, `bite-gp-platform` — all `1.21.0`, from crates.io |
| host | Intel i7-8750H, 12 threads, linux x86_64 — **not a controlled benchmarking host**: a laptop CPU, no pinning, no fixed clocks, other load present |

## The delta

Six files, and only one of them is the app's own source:

| file | change |
| --- | --- |
| `Cargo.toml` | `gpui = { path = "bite-gpui/crates/gpui" }` → `bite-gpui = "1.21"`; the `test-support` feature re-pointed at `bite-gpui/test-support`; `calloop` and `async-task` dropped from `[patch.crates-io]` |
| `patches/gpui_unofficial/Cargo.toml` | `gpui` → `{ package = "bite-gpui" }` |
| `patches/gpui_parley/Cargo.toml` | four `path = "../../bite-gpui/crates/*"` → version deps under their published names |
| `patches/gpui_parley/examples/frame_cost.rs` | one field, see below |
| `src/main.rs` | one field, see below |
| `Cargo.lock` | regenerated |

Two of the removed `[patch.crates-io]` entries stop being the consumer's
business: they were mirrored from the fork's root manifest, which patches
`calloop` and `async-task` to revisions it locks. `cargo` strips a `[patch]`
table before packaging, so no published manifest carries one, and
`docs/staging.md` §8 records the consequence this run is now evidence about:
*"for a patched dependency, the verification builds the git source while
consumers get the registry one."* `calloop` is that case: the fork's
`crates/gpui_linux` takes it from a git rev, the stager rewrites it to `0.14.3`,
`bite-gp-linux` is published `--no-verify`, and what a consumer actually resolves
is the registry's `0.14.4`. This build compiled that crate and the app's window
tests drew a headless frame through it, so the substitution holds at least that
far. The `gpui-unofficial` and `gpui_animotion` patches stay — those are this
app's own shims, not the fork's.

`gpui_parley` is still the app's own copy, by path. It is deliberately modified
(the hinting-instance cache, the host-font stack) and was never published, so the
published `bite-gp-parley` cannot stand in for it. **This run therefore proves
the facade and the layer crates are installable; it does not, by itself, prove
the published `bite-gp-parley` is a drop-in for the app.** That test is
`parley-demo/`, which does use the published crate.

## Result: green

| command | result |
| --- | --- |
| `cargo check --all-targets` | finished, warning-free |
| `cargo test` | 13 passed; 0 failed (the two window-free modules) |
| `cargo test --features test-support` | 28 passed; 0 failed; 1 ignored (the timing test), including the headless frame-drawing and glyph-outline tests |
| `cargo test -p gpui_parley --features test-support` | 12 + 2 passed, including the injection test |

So: no code change was needed to the app's logic, and the `TextSystem` this app
built against the fork injects into the *published* `bite-gpui` and draws with
it. That last one is the interoperability claim, tested rather than asserted.

## The one API gap: `PathVertex::content_mask`

Two lines of the app would not compile:

```
error[E0063]: missing field `content_mask` in initializer of `PathVertex<_>`
   --> src/main.rs:1403:40
```

`PathVertex` is `bite-gp-engine`'s. The published `1.21.0` is:

```rust
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}
```

which is 32 bytes: two `Point`s of two `f32` each, plus a `ContentMask` that is
one `Bounds` of four. `patches/README.md` says the fork's third commit dropped
this field, taking the struct to 16 bytes — and that is the fork's, not the
published crate's. This app was written against that branch, so it omits the
field, and its two `PathVertex` literals had to grow one:

```rust
content_mask: Default::default(),
```

The value is not a compromise. In the published `bite-gp-engine`, the field is
read in exactly one place — `PathVertex::scale`, which copies it — and
`bite-gp-wgpu` does not read it at all:

```rust
// wgpu_renderer.rs, draw_paths_to_intermediate
let bounds = path.clipped_bounds();
vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
    xy_position: v.xy_position,
    st_position: v.st_position,
    color: path.color,
    bounds,                       // the *path's* bounds, not the vertex's mask
}));
```

Clipping is per *path*, from `clipped_bounds()`, so a per-vertex mask is dead
weight: carried, scaled, and never consulted. The published engine's own
`Path::push_triangle` writes `content_mask: Default::default()` for every corner
it pushes, for the same reason. So the shim restores, from outside, the field the
renderer has already stopped reading.

**What this is evidence of.** Not a defect in the packaging — a gap between the
shipped artefact and the branch the app was written on, quantified: the published
`PathVertex` is the 32-byte shape the journal describes as "before the fork's
third commit dropped the per-corner `ContentMask`". The published release is
upstream of that commit. A fork that wants the 16-byte vertex, and the 3 MB a
frame instead of 6 MB it buys on the scene side, cannot get it from `"1.21"`.

## The three cost examples, after the repoint

Verbatim, in release. These are the apparatus the website's Benchmarks section
is built on, and now they run against the published crates with a two-line
change.

### `raster_cost`

```
one frame: 30 lines, 3480 glyphs, 840 distinct rasters
  layout         2.8ms   (92.1µs a line)
  raster        18.9ms   (22.5µs a glyph)
  total         21.7ms

  1000 rasters of one glyph at one size: 14.931106ms
  1000 hinting instances at one size:    27.236348ms
  1000 font loads + outline collections: 293µs
  1000 hinted outline draws at one size: 1.622384ms
  1000 unhinted outline draws at one size: 235.683µs

  a frame asks for one size per line, so: 30 instances, not 3480

steady over 200 frames of a flick, sizes bounded and oscillating:
ladder          atlas   misses/frm   layout/frm   raster/frm
continuous      32508          154      260.8µs        3.2ms
0.5px             644            0      236.1µs       38.9µs
0.25px           1260            0      234.0µs       38.5µs
0.125px          2548            0      236.8µs       39.0µs
0.0625px         5068            0      243.0µs       41.4µs
```

### `vector_cost`

```
one frame: 30 lines, 3480 glyphs
  tessellation (once)       566.1µs  for 56 outlines of 28 glyphs across 2 bands, 1271 triangles
  transform (a frame)        34.9µs  for 3480 glyphs, 218322 vertices
  vertex bytes              1746576  (99.9 MB a frame at 60fps)
```

### `frame_cost`

```
24 lines, 139 glyphs, 887.7682px wide at 14px, 28 glyph outlines in 28 band

flattening error   triangles   vertices   records a frame
    0.25              62040      186120    18.5 MB
    0.50              60552      181656    18.0 MB
    0.75              60024      180072    17.9 MB
    1.00              60024      180072    17.9 MB
    1.50              59880      179640    17.8 MB

cold, one frame                 13.3ms  62040 triangles

one frame (24 lines, 1000x700 window)
  place (the app's route)      863.5µs  62040 triangles, 186120 vertices
  place (push_triangle)          1.3ms  the same, through the per-corner bounds unions
  scale (paint_path)           614.5µs  a second copy of every vertex
  expand (104B records)          1.8ms  18.459778 MB, what the renderer is handed
  the app's route, whole        3.3ms  placement, scaled copy and expansion

  bytes a frame               44668800  (42.6 MB, 2.50 GB/s at 60fps)
  shape (once)                  81.8µs  for 24 lines
  tessellate (once)            311.9µs  for 28 outlines, 587 triangles
  tessellate (cached)            2.9µs  for 28 outlines, which is what a lookup costs
```

## Against the journal's numbers

`patches/README.md` recorded the same three examples, and the two sets do not
agree. The disagreement has three candidate causes and this run cannot separate
them: the journal names **no host and no toolchain**, and the example outputs
above are from a different compiler (1.98.1 vs the journal's 1.95.0 pin) on a
laptop CPU under other load.

**The structure reproduces exactly, which is the part worth trusting:**

| quantity | journal | this run |
| --- | --- | --- |
| `raster_cost`, one frame | 30 lines, 3480 glyphs, 840 distinct rasters | identical |
| ladder, continuous | atlas 32508, 154 misses/frame | identical |
| ladder, 0.5 px | atlas 644, 0 misses/frame | identical |
| ladder, 0.25 / 0.125 / 0.0625 px | 1260 / 2548 / 5068 | identical |
| `vector_cost`, vertices | 218,322 | 218,322 |
| `frame_cost`, vertices at 0.25 px | 186,120 | 186,120 |
| `frame_cost`, scene side | "6MB of 32-byte `PathVertex`" | 186,120 × 32 B = 5.96 MB |
| `frame_cost`, a frame | "40MB … 2.3GB/s" | 44.7 MB, 2.50 GB/s |

Every count and every cache size is the same number. Only the clock moves:

| measurement | journal | this run | ratio |
| --- | --- | --- | --- |
| `raster_cost` raster/frame, continuous | 7.9 ms | 3.2 ms | 2.5× |
| `raster_cost` raster/frame, 0.5 px | 83.5 µs | 38.9 µs | 2.1× |
| `raster_cost` layout/frame, continuous | 666.6 µs | 260.8 µs | 2.6× |
| `vector_cost` tessellation (once) | 681 µs | 566 µs | 1.2× |
| `vector_cost` transform/frame | 44 µs | 34.9 µs | 1.3× |
| `frame_cost` place (app's route) | ~1.1 ms | 863.5 µs | 1.3× |
| `frame_cost` place (`push_triangle`) | +0.5 ms over the route | +0.44 ms | 1.1× |
| `frame_cost` scale | ~0.5 ms | 614.5 µs | 0.8× |
| `frame_cost` expand | ~2.7 ms | 1.8 ms | 1.5× |

The ratios are not one number, so this is not a single clock multiplier: the
rasterization-heavy measurements moved by 2–2.6× and the integer-heavy ones by
1.1–1.5×. Both sets are internally consistent, and the journal does not say what
it ran on.

**What that means for the website.** The journal's *ratios* survive — the ladder
is still ~82× a frame here against the ~95× recorded, and the same two
conclusions fall out (a lattice stops missing once warm; the step trades atlas
entries, not speed). Its *absolute milliseconds* are one machine's, recorded
without saying which. A bar drawn from them is a bar from an unattributed host,
and this run is a different one, so the site either cites a record that names its
host or stops presenting absolute frame times as a property of the crates.

## Not established here

- **The published `bite-gp-parley` as a drop-in.** At the time, this project kept
  its own copy of the text system by design, so this run could not test that; the
  usage suite's `parley-demo` did, against the published crate. *Since updated:*
  the app now takes the published crate, and the paragraph above under *Since this
  was written* is the current state.
- **The GPU half.** All three examples say so themselves: what the renderer does
  with 42.6 MB of vertices a frame is not something a headless example answers.
- **That the published crate is slower or faster than the branch.** Only two
  runs exist, on different compilers, and the structural numbers are equal.
- **A repoint of the repository itself.** These edits were uncommitted when this
  was written; they are committed now, and the README and `patches/README.md` have
  been rewritten for the arrangement that replaced the vendored fork.
