//! A shim that points the published `gpui-unofficial` crate at this fork's
//! `gpui`.
//!
//! Crates from the gpui-unofficial ecosystem depend on the published
//! `gpui-unofficial` package, often renaming it to `gpui` in their own source.
//! The workspace's `[patch.crates-io]` table resolves that dependency to this
//! crate instead, so those crates compile against the fork rather than pulling
//! in a second, incompatible copy of gpui.
//!
//! This only redirects the dependency; it cannot make an ecosystem crate match
//! the fork's API. Where the two differ, the crate needs a local patch — see
//! `patches/gpui_animotion`, whose `Element` impl is the one place
//! `gpui_animotion` needed adapting.

/// Everything `gpui` exports, plus its prelude.
///
/// The prelude is re-exported at the root as well, because upstream crates
/// habitually write `use gpui::*;` and expect the traits that come with it.
pub use gpui::*;
pub use gpui::prelude::*;
