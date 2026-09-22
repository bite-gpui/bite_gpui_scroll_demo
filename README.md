# cool_scroll

A perceptual motion engine for scrolling: five candidate ways of making fast
scrolling feel like less of a fight, drawn over the same text so they can be
compared by eye. Run it, arm an effect in the panel in the top-right, and scroll.

Scroll with the wheel or trackpad, or drag the document for a flick.

- **1. Parallax Horizon**, **2. Rotating Barrel** and **3. Black Hole** are
  distortion fields: they trade legibility for a sense of travel.
- **4. Kinetic Fading** washes out whatever is currently large and fast.
- **5. Altitude** sidesteps the problem instead, shrinking the document the way
  ground recedes from a plane. It is mutually exclusive with the others.

The first switch in the panel is not one of the five. **Skeleton text** draws
every line as bars instead of setting its text — the same document either way,
since a bar is as wide as the word it stands in for, so the effects can be
compared without paying to re-shape every line at the size the transform asks
for, which is where a slow frame goes.

## Layout

    src/           the app
    assets/        the corpus, and the font the text system shapes with
    patches/       local copies of crates the fork cannot be used for as-is
    bite-gpui/     the GPUI fork this builds against

**`src/`**

- `main.rs` — the view, the controls panel, the line layout, the entry point.
- `motion.rs` — scroll physics and the perceptual transform: the document's
  position is a `gpui_animotion` property (a wheel tick tweens it, a drag springs
  it, letting go flings it, grabbing stops it) and every effect is a multiplier
  on one scale, while the simulation counts in 60fps frames so its feel survives
  a display that refreshes faster. Documented in depth in the file.
- `document.rs` — parses the corpus into paragraphs and wraps them into the lines
  the engine walks. A line also remembers how wide each of its words is, which is
  what the skeleton mode draws its bars from.

**The text** is `assets/war-and-peace.txt`, read at startup. Paragraphs are
separated by blank lines, short lines opening with `CHAPTER`/`PART`/`BOOK` are
set as headings, and each paragraph is wrapped to an 800px column. Wrapping needs
text measurement, so the document is built on the first frame rather than in the
constructor; `document::MAX_LINES` caps how much of the book is taken (the whole
thing is ~90,000 lines, and the cost is linear in the text). Point
`load_corpus` at another file to scroll through something else.

The measuring and the rendering both go through Parley — Skrifa for the glyph
outlines, tiny-skia for their coverage — because `main` installs a copy of the
fork's `gpui_parley` with `application().with_text_system(...)`. Worth knowing
what that crate is: it shapes with its own embedded IBM Plex Sans and ignores the
family it is asked for, with hard-coded metrics, so the panel and the document
are both set in Plex Sans and nothing falls back. It exists to prove GPUI's text
SPI can be implemented out of tree, not to be a general text stack — which is
why it lives in `patches/` as a copy, rather than being depended on where it is.

The document's position is a `gpui_animotion` property — one of the crate's
interruptible `Prop`s — rather than a simulation of its own, and every way of
moving it is one of that crate's segments: a wheel tick tweens to the accumulated
target, a pointer move re-aims a spring, letting go of a drag hands the pointer's
velocity to a kinetic flick, and taking hold stops it dead. What `motion.rs`
simulates is everything the prototype layered on top of position: the blend that
switches the effects on, the altitude the document climbs, and the direction it
is tilting.

The window asks for a frame only while something is moving, and animations are
unmounted or dropped once they have settled, so the app goes quiet at rest. The
panel's transitions (the switch knob, and controls dimming as they are armed)
come from `gpui_animotion` as well.

**`bite-gpui/`** is a checkout of `Vanuan/bite-gpui`, sitting on the tip of
branch `bite_v1.21.0-pre` (`d418335`) — it is unmodified, so `git status` there
is clean and it can be updated in place. It is a dependency rather than a
workspace member, so it can also be replaced by a `git` dependency (see the
comment in `Cargo.toml`) or built against a local edit, without anything of the
app's riding along.

**`patches/`** holds local copies of the three crates this app cannot take as
they come: a shim that resolves the published `gpui-unofficial` to the fork's
`gpui`, `gpui_animotion` with the one `Element` signature adapted to the fork's
trait, and `gpui_parley` — which the app *uses* rather than patches, and keeps
here so that changing it is possible. See `patches/README.md`.

## Running

    cargo run --release

The toolchain is pinned to the fork's (`rust-toolchain.toml`), and the first
build compiles the GPUI stack, so give it a couple of minutes — several more for
a release build. Release is worth it: this is a motion demo, and in a debug build
most of each frame goes on laying out and shaping the lines on screen.

## Tests

    cargo test                            # the engine's own tests
    cargo test --features test-support    # ...and the view's, on a headless window

Neither half needs a display, but only the second needs a window.

`document.rs` has unit tests for parsing, wrapping and the word widths the
skeleton mode needs, measured with a stick of 40px per character rather than a
text system. `motion.rs` has unit tests for the physics: a wheel tick settling on
its target, a flick landing where its decay says it will, a flick into an end
coming to rest on it, a tick turning a flick around mid-flight, altitude climbing
with speed and falling more slowly, and the mutual exclusion between Altitude and
the distortion fields.

The view's tests live in `main.rs` and drive a real window on the headless
platform, which is `gpui`'s `test-support`: the document is walked and the wheel
moves it in the right direction (and the walk finds the same lines in both
modes), a wheel over the panel leaves the document alone, a switch springs its
knob across and settles (which exercises the patched animotion element end to
end), skeleton mode paints bars where the text was, a drag throws the document
past where the pointer asked it to be, grabbing calls off a flick in flight, the
corpus loads, parses and wraps, and the app really is shaping its text through
Parley.

That feature is declared by this crate rather than left in `[dev-dependencies]`
because a dev-dependency's features are unified into every dev-context build:
`test-support` moves gpui's frame drawing onto a path that draws every dirty
window at the end of every `App::update` instead of leaving frames to the
platform, so a build that is not running tests should not be able to pick it up
by accident. The same reasoning is written out at length in the manifest of
`patches/gpui_parley`, whose example would render once per input event if it
ever saw this feature.

The vendored text system has tests of its own:

    cargo test -p gpui_parley                          # shaping, rasterization, its caches
    cargo test -p gpui_parley --features test-support  # ...and that it can be injected
