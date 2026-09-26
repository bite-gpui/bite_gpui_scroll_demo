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

A third crate used to live here. `gpui_parley/` was a copy of the text system
this app shapes its document with, carrying the changes that make it usable at
this app's sizes — the hinting instance cached per face and size rather than per
rasterized glyph, and shaping and rasterization reading the shaper's own font
database so a family stack resolves against this machine's fonts. It was the
only copy of the crate with those changes in it: the fork's `crates/gpui_parley`
on every branch, and the `bite-gp-parley` published from it, are without them.

It is now the crate's own repository —
[`bite-gpui/gpui_parley`](https://github.com/bite-gpui/gpui_parley) — and this
workspace depends on it by revision, in the root `Cargo.toml`. Its changes, its
examples and the numbers those examples print live there, because they are the
crate's rather than this app's:

```sh
cargo test --features test-support
cargo run --release --example raster_cost
```
