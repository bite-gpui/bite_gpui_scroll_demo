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

Those features are the fork's, and they are load-bearing. `test-support` moves
gpui's frame drawing onto a path that draws every dirty window at the end of
every `App::update`, and a dev-dependency's features are unified into the example
build, so the example would render once per input event if `[dev-dependencies]`
were used to pull in `gpui`'s `test-support`. The app depends on this crate with
no features at all, and declares `test-support` as a feature of its own for the
same reason — see the comment on it in the root `Cargo.toml`.
