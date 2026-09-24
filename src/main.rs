//! A perceptual motion engine for scrolling, after the HTML prototype of the
//! same name.
//!
//! Five candidate ways of making fast scrolling feel like less of a fight, all
//! drawn over the same text so they can be compared by eye:
//! `Parallax`, `Barrel` and `Black Hole` are distortion fields that trade
//! legibility for a sense of travel; `Kinetic Fading` washes out the things
//! that are moving too fast to read anyway; and `Altitude` sidesteps the
//! problem by shrinking the document instead of distorting it.
//!
//! Scroll with the wheel or trackpad, or drag the document directly for a
//! flick. The panel in the top-right arms each effect and sets its intensity;
//! the readout in the bottom-left shows what the simulation is doing. The
//! panel's first control is not one of the five: it chooses how the document
//! itself is painted — set as text, drawn as skeleton bars, or drawn as the
//! glyphs' own outlines, tessellated once and scaled by the transform.
//!
//! The document's motion is one of `gpui_animotion`'s interruptible
//! properties: a wheel tick tweens it to where the wheel has asked for, a drag
//! re-aims a spring, letting go hands the pointer's velocity to a kinetic
//! flick, and grabbing stops it dead. Everything the prototype layered on top
//! of position — the effect blends, the altitude, the tilt — is the engine's
//! own, and is tuned in `motion.rs`. The panel's transitions come from
//! `gpui_animotion` too; see `patches/` for how that crate is pointed at this
//! fork.

mod document;
mod motion;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Div, Font, FontWeight, FramePipelineExt as _,
    Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Path, PathVertex, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, Size, StandardImmediatePipeline,
    Stateful, TextRun, TextStyle, TextSystem as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions, WindowTextSystem, application, canvas, div, fill, point, px, relative, rgba,
    size,
};
use gpui_animotion::{
    AnimotionExt as _, Interpolate as _, SpringParams, VelocityTracker, prop, spring, tween,
};
use gpui_parley::{ParleyTextSystem, ParleyTextSystemExt as _};

use document::{BASE_SPACING, CONTENT_WIDTH, Line, Style};
use motion::{Effect, FRAMES_PER_SECOND, IntensitySpec, Motion, Settings, intensity_spec};

/// Multiplier on wheel deltas (the prototype's `deltaY * 1.5`).
const WHEEL_GAIN: f32 = 1.5;
/// A wheel delta reported in "lines" is treated as this many pixels.
const LINE_HEIGHT: f32 = 20.0;
/// A drag moves the document faster than the pointer, so a short swipe crosses
/// ground (the prototype's `deltaY * 2`).
const DRAG_GAIN: f32 = 2.0;
/// Extra document drawn beyond the viewport before a direction stops.
const OVERDRAW: f32 = 100.0;

/// Luminances of the prototype's greys, as HSL lightness. All the colours in
/// the document are achromatic, so lightness is the whole story.
const BACKGROUND_LUMINANCE: f32 = 5.0 / 255.0; // #050505
const BODY_LUMINANCE: f32 = 136.0 / 255.0; // #888888

/// Panel geometry, fixed so the slider maths can be exact.
const PANEL_WIDTH: f32 = 300.0;
const PANEL_MARGIN: f32 = 20.0;
const PANEL_PADDING: f32 = 20.0;
const CONTROL_PADDING: f32 = 12.0;
const SLIDER_WIDTH: f32 = PANEL_WIDTH - 2.0 * PANEL_PADDING - 2.0 * CONTROL_PADDING;
const SLIDER_HEIGHT: f32 = 20.0;
const TRACK_HEIGHT: f32 = 4.0;
const THUMB: f32 = 16.0;
const SWITCH_WIDTH: f32 = 44.0;
const SWITCH_HEIGHT: f32 = 24.0;
const KNOB: f32 = 20.0;

/// How long the prototype's CSS transitions take: an unarmed control dimming,
/// and a switch's track changing colour. The knob is the exception — it gets a
/// spring, which suits a toggle better than a 300ms ease.
const CONTROL_SECS: f32 = 0.3;

/// A skeleton bar's height, as a fraction of its line's font size: roughly a
/// capital's, which is what a bar stands in for.
const SKELETON_BAR: f32 = 0.7;
/// How round a skeleton bar's ends are, in unscaled document pixels.
const SKELETON_RADIUS: f32 = 3.0;

/// How much error a glyph's outline may be flattened with, in device pixels.
///
/// The tolerances are relative to the size the glyph is drawn at, so a quarter
/// of a pixel is finer than anything the transform can magnify into view.
const GLYPH_ERROR: f32 = 0.25;

/// The size an outline should be tessellated for, given the size it is drawn at.
///
/// Rounding *up* to a power of two means the flattening is always fine enough at
/// the size drawn — and it makes the tessellation cache's key coarse, which is
/// the point of rounding at all: a handful of bands rather than one outline per
/// size the transform asks for.
fn band(size: f32) -> f32 {
    2f32.powf(size.max(0.001).log2().ceil())
}

/// A one-shot property animation, mounted only while it has somewhere to go.
///
/// `gpui_animotion`'s declarative element samples its tracks from the first
/// frame it is laid out and asks for another frame on every layout after that.
/// Left mounted, one would mean a window that never goes idle — which this app
/// otherwise does, by only asking for frames while the document is moving. So a
/// transition is dropped once it has settled, and the settled value is drawn
/// straight from the switch's state from then on.
#[derive(Clone, Copy)]
struct Transition {
    /// The value the animation starts from.
    from: f32,
    /// The value it is heading for.
    to: f32,
    /// When it will have settled.
    settles_at: Instant,
}

/// How many switches the panel has: one for each effect.
///
/// The document's own control is not a switch but a selector, which keeps its
/// own row of transitions in [`Switches::mode`].
const SWITCHES: usize = Effect::ALL.len();

/// The panel's switches, and the animations of whichever ones are moving.
///
/// A switch at rest is drawn straight from its state; one that has just changed
/// keeps a row here for as long as it takes to arrive, and is then dropped, so
/// that a settled panel asks for no frames at all.
#[derive(Clone, Copy, Default)]
struct Switches {
    /// Per switch: the knob's spring, the control's opacity as it dims, and the
    /// track's colour.
    knob: [Option<Transition>; SWITCHES],
    dim: [Option<Transition>; SWITCHES],
    track: [Option<Transition>; SWITCHES],
    /// Per mode: the selector's cell, as it lights up or gives way to another.
    mode: [Option<Transition>; Mode::ALL.len()],
}

impl Switches {
    /// Move the switch in `slot` from `was` to `enabled`.
    fn transition(&mut self, slot: usize, was: bool, enabled: bool, now: Instant) {
        let (from, to) = (knob_offset(was), knob_offset(enabled));
        self.knob[slot] = Some(Transition {
            from,
            to,
            // The spring knows when it will have settled: no need to hard-code
            // a duration that does not match its stiffness.
            settles_at: now + spring(from, to, knob_spring()).total_duration(),
        });
        self.dim[slot] = Some(Transition {
            from: control_opacity(was),
            to: control_opacity(enabled),
            settles_at: now + Duration::from_secs_f32(CONTROL_SECS),
        });
        self.track[slot] = Some(Transition {
            from: armed(was),
            to: armed(enabled),
            settles_at: now + Duration::from_secs_f32(CONTROL_SECS),
        });
    }

    /// Move the selector's selection from `from` to `to`.
    ///
    /// Every cell whose state actually changed gets a transition — the one that
    /// loses the selection as well as the one that takes it, so the two cross
    /// over on the same clock rather than one jumping out from under the other.
    fn select(&mut self, from: Mode, to: Mode, now: Instant) {
        for mode in Mode::ALL {
            let (was, is) = (mode == from, mode == to);
            if was == is {
                continue;
            }
            self.mode[mode.index()] = Some(Transition {
                from: armed(was),
                to: armed(is),
                settles_at: now + Duration::from_secs_f32(CONTROL_SECS),
            });
        }
    }

    /// Forget the transitions that have settled, so their elements unmount.
    fn drop_settled(&mut self, now: Instant) {
        let settled =
            |transition: &Option<Transition>| transition.is_some_and(|t| t.settles_at <= now);
        let switches: [&mut [Option<Transition>]; 3] =
            [&mut self.knob, &mut self.dim, &mut self.track];
        for transition in switches.into_iter().flatten().chain(self.mode.iter_mut()) {
            if settled(transition) {
                *transition = None;
            }
        }
    }
}

/// The switch knob's spring.
///
/// ζ = 0.7: enough overshoot to feel like a mechanism with mass, and not enough
/// to look like it is hunting for its position.
fn knob_spring() -> SpringParams {
    SpringParams::from_damping_ratio(420.0, 0.7)
}

/// Where the knob sits within the switch, for each state.
fn knob_offset(enabled: bool) -> f32 {
    if enabled {
        SWITCH_WIDTH - KNOB - 2.0
    } else {
        2.0
    }
}

/// How solid an armed or unarmed control is drawn.
fn control_opacity(enabled: bool) -> f32 {
    if enabled { 1.0 } else { 0.4 }
}

/// 0 or 1: a switch's state as an animatable number.
fn armed(enabled: bool) -> f32 {
    if enabled { 1.0 } else { 0.0 }
}

/// Bounds on how much animation time one frame of real elapsed time is worth. A
/// frame the compositor was slow over should not teleport the document, and a
/// frame that arrives straight away should still count for something. The
/// simulation counts in 60fps frames ([`FRAMES_PER_SECOND`]), so this is also
/// what gives it the same feel on a display that refreshes faster.
const MIN_FRAMES: f32 = 0.25;
const MAX_FRAMES: f32 = 4.0;

/// `#050505`, the page.
fn background() -> Hsla {
    grey(BACKGROUND_LUMINANCE, 1.0)
}

/// An achromatic colour at the given luminance and alpha.
fn grey(luminance: f32, alpha: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: luminance,
        a: alpha,
    }
}

/// `#0A84FF`, the accent, at the given alpha.
fn accent(alpha: f32) -> Hsla {
    let mut color: Hsla = rgba(0x0a84ffff).into();
    color.a = alpha;
    color
}

/// A switch's track: `#333333` when off, the accent when on, and the blend
/// between them while it is changing, so the track can be animated on the same
/// clock as its knob.
fn switch_color(armed: f32) -> Hsla {
    grey(0.2, 1.0).interpolate(&accent(1.0), armed)
}

/// How the document is drawn.
///
/// The three modes are the same lines: what changes is what a line costs to
/// paint, and what the transform has to redo when it changes its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// The corpus, set in the window's text style.
    ///
    /// Each line is shaped and rasterized at the size the transform asks for on
    /// the frame it is drawn, which is the reference the other two are measured
    /// against: the sharpest text, and the only mode that does that work twice
    /// over whenever a line changes size.
    #[default]
    Text,
    /// The same lines, with their glyphs replaced by bars.
    ///
    /// The layout is identical — a bar is as wide as the word it stands in for,
    /// and sits on the same line — so the two can be compared directly. What it
    /// saves is everything about text: a bar is a quad, and no amount of
    /// resizing asks the text system to shape or rasterize anything.
    Skeleton,
    /// The same lines, drawn as the glyphs' own outlines.
    ///
    /// A glyph's outline does not change shape with the size it is drawn at, so
    /// each one is tessellated into triangles once and scaled, per line, to
    /// whatever size the transform is asking for. Nothing is rasterized per
    /// frame: placing a glyph is one multiply per corner of its triangles.
    Vector,
}

impl Mode {
    /// The modes, in the order the selector shows them.
    const ALL: [Mode; 3] = [Mode::Text, Mode::Skeleton, Mode::Vector];

    /// This mode's slot in the selector.
    const fn index(self) -> usize {
        match self {
            Mode::Text => 0,
            Mode::Skeleton => 1,
            Mode::Vector => 2,
        }
    }

    /// The name on the selector's cell, and on the readout's first line.
    const fn label(self) -> &'static str {
        match self {
            Mode::Text => "Text",
            Mode::Skeleton => "Skeleton",
            Mode::Vector => "Vector",
        }
    }

    /// What the mode's cell is found by, for tests.
    ///
    /// `debug_bounds` takes a `&'static str`, so this cannot be built from
    /// [`Mode::label`] at the call site.
    const fn key(self) -> &'static str {
        match self {
            Mode::Text => "mode-text",
            Mode::Skeleton => "mode-skeleton",
            Mode::Vector => "mode-vector",
        }
    }

    /// What the mode is for, under the selector.
    const fn description(self) -> &'static str {
        match self {
            Mode::Text => {
                "Set every line at the size the transform asks for. The reference the other two \
                 are measured against: sharp at any magnification, and shaped and rasterized \
                 afresh whenever a line changes size."
            }
            Mode::Skeleton => SKELETON_DESCRIPTION,
            Mode::Vector => {
                "The same glyphs as their own outlines, tessellated once and scaled to whatever \
                 size each line asks for. Nothing is rasterized per frame — placing a glyph is a \
                 multiply per corner — so the same work covers any magnification."
            }
        }
    }
}

/// The document mode selector's heading, and what the skeleton mode is for.
const DOCUMENT_TITLE: &str = "Document";
const SKELETON_DESCRIPTION: &str = "Draw every line as bars instead of setting its text. Same layout, same motion — but a bar never has \
     to be re-shaped at the size the transform asks for, and that is where a slow frame goes.";

struct CoolScroll {
    settings: Settings,
    /// How the corpus is drawn: set as text, as bars, or as glyph outlines.
    mode: Mode,
    motion: Motion,
    /// The corpus as it came off disk. It is wrapped into `document` on the first
    /// frame, because wrapping measures text.
    source: String,
    /// The wrapped document: what the engine walks, and what gets painted.
    document: Rc<Vec<Line>>,
    /// The vector mode's lines, shaped, by document index.
    ///
    /// The text mode asks the text system to shape every visible line on every
    /// frame, at whatever size the transform is asking for on that frame. The
    /// vector mode wants none of that: a glyph's place in its line and the
    /// width of the line are proportional to the size it is set at, and the
    /// outline itself does not change with size at all — so one shaping at the
    /// document's own size serves every size the line is ever drawn at. It is
    /// filled in as lines come on screen rather than up front, because shaping
    /// the whole book is a second of work for a screenful of reading.
    shaped: RefCell<HashMap<usize, ShapedLine>>,
    /// When the last frame was drawn. Only elapsed time matters here, so a
    /// plain monotonic clock is enough.
    last_frame: Instant,
    /// How long the last frame's interval was, and how much of it went on
    /// building the document's elements. The two are worth telling apart: the
    /// interval is what the frame *cost*, wherever the work happened, while the
    /// walk is the part this file does — visibly cheap in a mode whose frame is
    /// spent elsewhere, which is what makes the readout worth reading.
    frame_time: Duration,
    walk_time: Duration,
    /// Set while the document itself is being dragged.
    scroll_drag: Option<ScrollDrag>,
    /// Set while a slider is being dragged.
    slider_drag: Option<SliderDrag>,
    /// The switches, and which of their springs are still in flight.
    switches: Switches,
}

struct ScrollDrag {
    last_y: Pixels,
    /// How fast the pointer has been carrying the document, measured across the
    /// last handful of moves rather than taken from the previous event's delta:
    /// the same flick of the wrist should throw the document the same way
    /// however fast the events happen to arrive.
    ///
    /// It tracks the drag's *target*, in document pixels, which is where the
    /// pointer is asking the document to be — so the gain between pointer and
    /// document is already part of the measurement.
    velocity: VelocityTracker,
}

#[derive(Clone, Copy)]
struct SliderDrag {
    /// Which slider is being dragged: only one at a time, but pointer moves
    /// also reach the other sliders' hitboxes.
    effect: Effect,
    start_x: Pixels,
    start_value: f32,
}

impl CoolScroll {
    fn new() -> Self {
        Self::with_corpus(load_corpus())
    }

    /// A view over a corpus, before it has been wrapped.
    fn with_corpus(source: String) -> Self {
        Self {
            settings: Settings::default(),
            mode: Mode::default(),
            motion: Motion::new(),
            source,
            document: Rc::new(Vec::new()),
            shaped: RefCell::new(HashMap::new()),
            last_frame: Instant::now(),
            frame_time: Duration::ZERO,
            walk_time: Duration::ZERO,
            scroll_drag: None,
            slider_drag: None,
            switches: Switches::default(),
        }
    }

    /// Wrap the corpus into the lines the engine walks.
    ///
    /// Wrapping measures every word, which needs the window's text system, so
    /// this runs on the first frame rather than in the constructor. The
    /// measurements are cached by the text system, so repeated words are cheap.
    fn build_document(&mut self, window: &mut Window) {
        let text_system = window.text_system().clone();
        let base = window.text_style();
        let mut measure = |word: &str, style: Style| -> f32 {
            let mut font = document_font(&base);
            if style == Style::Heading {
                font.weight = FontWeight::BOLD;
            }
            let run = TextRun {
                len: word.len(),
                font,
                color: base.color,
                ..Default::default()
            };
            f32::from(
                text_system
                    .shape_line(
                        SharedString::from(word),
                        px(style.font_size()),
                        &[run],
                        None,
                    )
                    .width(),
            )
        };

        let lines = document::build(&self.source, document::MAX_LINES, &mut measure);
        self.motion.set_content(document::content_height(&lines));
        self.document = Rc::new(lines);
    }

    /// Elapsed time since the last frame, in 60fps frames.
    fn elapsed_frames(&mut self) -> f32 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame);
        self.last_frame = now;
        self.frame_time = elapsed;
        (elapsed.as_secs_f32() * FRAMES_PER_SECOND).clamp(MIN_FRAMES, MAX_FRAMES)
    }

    /// Let go of the document, handing whatever speed the pointer had to a
    /// kinetic flick.
    fn release_document(&mut self, cx: &mut Context<Self>) {
        let Some(mut drag) = self.scroll_drag.take() else {
            return;
        };
        self.motion.release(drag.velocity.velocity());
        cx.notify();
    }

    /// Arm or disarm an effect, springing the switches that move.
    ///
    /// Altitude and the distortion fields are mutually exclusive, so flipping
    /// one switch can flip another. Every switch whose state actually changed
    /// gets a transition, not just the one that was clicked.
    fn toggle(&mut self, effect: Effect, cx: &mut Context<Self>) {
        let before: [bool; Effect::ALL.len()] =
            std::array::from_fn(|index| self.settings.is_on(Effect::ALL[index]));
        self.settings.set(effect, !before[effect.index()]);

        let now = Instant::now();
        for effect in Effect::ALL {
            let enabled = self.settings.is_on(effect);
            let slot = effect.index();
            if enabled != before[slot] {
                self.switches.transition(slot, before[slot], enabled, now);
            }
        }

        cx.notify();
    }

    /// Set the document's drawing mode, moving the selector's selection.
    ///
    /// The modes are the same document, so this is only a change of how the
    /// lines are painted: the scroll offset, the length of the document and
    /// everything the transform is doing about it all stay as they are.
    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }
        self.switches.select(self.mode, mode, Instant::now());
        self.mode = mode;
        cx.notify();
    }
}

impl Render for CoolScroll {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The document is wrapped on the first frame, because wrapping measures
        // text and only the window has a text system.
        if self.document.is_empty() {
            self.build_document(window);
        }

        // Advance first, so this frame draws the state it is about to show in
        // the readout, and keep asking for frames while anything is still
        // moving. At rest the window goes quiet.
        let frames = self.elapsed_frames();
        self.motion.advance(frames, &self.settings);
        if self.motion.needs_frames() {
            window.request_animation_frame();
        }

        // Transitions that have run their course are dropped before the panel
        // is built, so a settled control is drawn statically and stops asking
        // for frames.
        self.switches.drop_settled(Instant::now());

        let settings = self.settings;
        let switches = self.switches;
        let mode = self.mode;
        let dragging = self.scroll_drag.is_some();
        // What the vector mode placed this frame, for the readout. The walk is
        // what fills it in, so it is read after the walk returns.
        let triangles = Cell::new(0);
        let walk = Instant::now();
        let lines = visible_lines(
            Scene {
                document: &self.document,
                motion: &self.motion,
                settings: &settings,
                mode,
                text_system: window.text_system(),
                font: document_font(&window.text_style()),
                shaped: &self.shaped,
                triangles: &triangles,
                scale_factor: window.scale_factor(),
            },
            window.viewport_size(),
        );
        self.walk_time = walk.elapsed();

        div()
            .relative()
            .size_full()
            .bg(background())
            // The prototype sets `color` on `body`; gpui's default text colour is
            // black, which would be invisible on this page. Everything inherits
            // this and the muted parts override it.
            .text_color(rgba(0xededff))
            .child(document_field(lines, dragging, cx))
            .child(panel(&settings, mode, &switches, cx))
            .child(readout(
                &self.motion,
                mode,
                triangles.get(),
                self.frame_time,
                self.walk_time,
                frame_cap_interval(),
            ))
    }
}

/// The document itself, plus the gestures that move it.
fn document_field(
    lines: Vec<AnyElement>,
    dragging: bool,
    cx: &mut Context<CoolScroll>,
) -> Stateful<Div> {
    div()
        .id("document")
        .absolute()
        .inset_0()
        .cursor(if dragging {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        })
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
            this.motion.wheel(document_delta(event) * WHEEL_GAIN);
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                // Grabbing the document calls off any fling already in flight.
                this.motion.grab();
                let mut velocity = VelocityTracker::default();
                velocity.push(this.motion.target_scroll);
                this.scroll_drag = Some(ScrollDrag {
                    last_y: event.position.y,
                    velocity,
                });
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            let Some(drag) = this.scroll_drag.as_mut() else {
                return;
            };
            // Dragging up moves the document down, as dragging paper would.
            let delta = f32::from(drag.last_y - event.position.y);
            drag.last_y = event.position.y;
            this.motion.drag(delta * DRAG_GAIN);
            drag.velocity.push(this.motion.target_scroll);
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| this.release_document(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| this.release_document(cx)),
        )
        .children(lines)
        .child(
            // The reference band is painted, not laid out: it is a line of light
            // across the document rather than content.
            canvas(
                |_bounds, _window, _cx| {},
                |bounds, _state, window, _cx| {
                    paint_anchor_line(bounds, window);
                },
            )
            .absolute()
            .inset_0(),
        )
}

/// A wheel delta, as pixels of document travel: how far to move the document
/// *down*, sign included.
///
/// GPUI reports deltas in the opposite sense to the DOM: a **negative** y delta
/// scrolls down. Its own scroll containers say so — in `gpui_authoring`'s list
/// tests, `px(100.)` is "the user scrolling up" and `px(-10000.)` is "scroll back
/// down to the bottom" — and a div's scroll offset, which lives in
/// `[-max, 0]`, is advanced by `+= delta.y`.
///
/// The prototype's `targetScrollY += e.deltaY` assumes the DOM's
/// positive-is-down, so the sign is flipped here. Following GPUI's convention is
/// what makes this respect the platform's own scroll settings — natural
/// scrolling, a reversed mouse wheel — instead of fighting them.
fn document_delta(event: &ScrollWheelEvent) -> f32 {
    let reported = match event.delta {
        // Trackpads and smooth wheels report exact pixels.
        ScrollDelta::Pixels(delta) => f32::from(delta.y),
        // Clicky wheels report lines.
        ScrollDelta::Lines(delta) => delta.y * LINE_HEIGHT,
    };
    -reported
}

/// The controls, stacked in the top-right corner.
fn panel(
    settings: &Settings,
    mode: Mode,
    switches: &Switches,
    cx: &mut Context<CoolScroll>,
) -> Stateful<Div> {
    div()
        .id("controls")
        .debug_selector(|| "controls".into())
        .absolute()
        .top(px(PANEL_MARGIN))
        .right(px(PANEL_MARGIN))
        .w(px(PANEL_WIDTH))
        .max_h(relative(0.9))
        .overflow_y_scroll()
        // The panel owns its whole area. `block_mouse_except_scroll` is the
        // usual choice and deliberately lets scroll through to what is behind,
        // which is not what an overlay like this wants: a wheel over the panel
        // scrolls the panel, or nothing. `occlude` is what makes the document
        // behind stop seeing the event at all.
        .occlude()
        .flex()
        .flex_col()
        .gap_3()
        .p(px(PANEL_PADDING))
        .rounded(px(16.0))
        .border_1()
        .border_color(rgba(0xffffff1a))
        .bg(rgba(0x141414d9))
        .shadow_lg()
        .child(
            div()
                // 16px/600, per the prototype's `h1`.
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Perceptual Motion Engine"),
        )
        .child(document_group(mode, switches, cx))
        .children(Effect::ALL.map(|effect| effect_group(settings, switches, cx, effect)))
}

/// The document's own control: how the corpus is drawn.
fn document_group(mode: Mode, switches: &Switches, cx: &mut Context<CoolScroll>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                // 14px/500, like the effects' own titles.
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .child(DOCUMENT_TITLE),
        )
        .child(mode_selector(mode, switches, cx))
        .child(
            div()
                // 11px/1.4, per the prototype's `.description`.
                .text_size(px(11.0))
                .line_height(relative(1.4))
                .text_color(rgba(0x888888ff))
                .child(mode.description()),
        )
        .child(div().h(px(1.0)).w_full().bg(rgba(0xffffff1a)))
}

/// How the document is drawn, as one choice among three.
///
/// A switch would leave the modes as two independent things that happen to be
/// exclusive; the document is drawn exactly one of these ways, so the control
/// says so and there is no state in which two of them are on. The selected cell
/// lights up on the same clock the switches change on.
fn mode_selector(mode: Mode, switches: &Switches, cx: &mut Context<CoolScroll>) -> Div {
    div()
        .flex()
        .gap_1()
        .p(px(2.0))
        .rounded_md()
        .bg(rgba(0x00000033))
        .children(Mode::ALL.map(|candidate| {
            let slot = candidate.index();
            let selected = candidate == mode;

            // The animated face is a plain `Div` inside the interactive cell:
            // `gpui_animotion`'s extension trait is implemented for `Div`, while
            // what carries the click is a `Stateful<Div>`.
            let face = div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .py(px(4.0))
                .rounded_sm()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .child(candidate.label());

            let face = match switches.mode[slot] {
                Some(Transition { from, to, .. }) => face
                    .animotion(
                        ("mode", slot),
                        vec![
                            prop(tween(from, to, CONTROL_SECS), |el, selection| {
                                el.bg(accent(0.22 * selection))
                            }),
                            prop(tween(from, to, CONTROL_SECS), |el, selection| {
                                el.text_color(mode_text(selection))
                            }),
                        ],
                    )
                    .into_any_element(),
                None => face
                    .bg(accent(0.22 * armed(selected)))
                    .text_color(mode_text(armed(selected)))
                    .into_any_element(),
            };

            div()
                .id(("mode", slot))
                .debug_selector(move || candidate.key().to_owned())
                .flex_1()
                .cursor_pointer()
                .on_click(
                    cx.listener(move |this, _event, _window, cx| this.set_mode(candidate, cx)),
                )
                .child(face)
                .into_any_element()
        }))
}

/// How solid a selector cell's label is drawn: grey when another mode holds the
/// selection, white when this one does.
fn mode_text(selection: f32) -> Hsla {
    grey(1.0, 0.55 + 0.45 * selection)
}

/// One effect: its switch, what it is going for, and its intensity.
fn effect_group(
    settings: &Settings,
    switches: &Switches,
    cx: &mut Context<CoolScroll>,
    effect: Effect,
) -> Div {
    let enabled = settings.is_on(effect);
    let spec = intensity_spec(effect);

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        // 14px/500, per the prototype's `.toggle-row`.
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(effect.title()),
                )
                .child(switch(
                    effect.key(),
                    effect.index(),
                    enabled,
                    switches,
                    cx,
                    move |this, cx| this.toggle(effect, cx),
                )),
        )
        .child(
            div()
                // 11px/1.4, per the prototype's `.description`.
                .text_size(px(11.0))
                .line_height(relative(1.4))
                .text_color(rgba(0x888888ff))
                .child(effect.description()),
        )
        .children(spec.map(|spec| {
            intensity_control(
                settings,
                cx,
                effect,
                spec,
                enabled,
                switches.dim[effect.index()],
            )
        }))
        .when(effect != Effect::Altitude, |group| {
            group.child(div().h(px(1.0)).w_full().bg(rgba(0xffffff1a)))
        })
}

/// One switch in `slot`: a track that changes colour, and a knob that springs
/// across it. `key` names it for element ids and test selectors.
fn switch(
    key: &'static str,
    slot: usize,
    enabled: bool,
    switches: &Switches,
    cx: &mut Context<CoolScroll>,
    on_click: impl Fn(&mut CoolScroll, &mut Context<CoolScroll>) + 'static,
) -> AnyElement {
    let knob_el = div()
        .absolute()
        .top(px((SWITCH_HEIGHT - KNOB) / 2.0))
        .size(px(KNOB))
        .rounded_full()
        .bg(rgba(0xffffffff))
        .shadow_lg()
        .debug_selector(move || format!("knob-{key}"));

    // While the spring is in flight it drives the knob's offset; once it has
    // settled the offset comes straight from the switch's state, and the
    // animation is unmounted.
    let knob_el = match switches.knob[slot] {
        Some(Transition { from, to, .. }) => knob_el
            .animotion(
                ("knob", slot),
                vec![prop(spring(from, to, knob_spring()), |el, offset| {
                    el.left(px(offset))
                })],
            )
            .into_any_element(),
        None => knob_el.left(px(knob_offset(enabled))).into_any_element(),
    };

    // The fill is a plain div: `gpui_animotion`'s extension trait is implemented
    // for `Div`, while the element that carries interactivity is a
    // `Stateful<Div>`. It sits behind the knob.
    let fill = div().absolute().inset_0().rounded_full();
    let fill = match switches.track[slot] {
        Some(Transition { from, to, .. }) => fill
            .animotion(
                ("track", slot),
                vec![prop(tween(from, to, CONTROL_SECS), |el, armed| {
                    el.bg(switch_color(armed))
                })],
            )
            .into_any_element(),
        None => fill.bg(switch_color(armed(enabled))).into_any_element(),
    };

    // The track's colour runs on the same clock as the knob, so a switch reads
    // as one object changing state rather than a knob moving inside a colour
    // that has already jumped.
    div()
        .id(("switch", slot))
        .debug_selector(move || format!("switch-{key}"))
        .relative()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .rounded_full()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _event, _window, cx| on_click(this, cx)))
        .child(fill)
        .child(knob_el)
        .into_any_element()
}

/// A label, a value, and a track to drag. Dragging is relative to where the
/// pointer went down rather than jumping to it, which keeps a coarse first
/// click from moving the value far.
fn intensity_control(
    settings: &Settings,
    cx: &mut Context<CoolScroll>,
    effect: Effect,
    spec: IntensitySpec,
    enabled: bool,
    transition: Option<Transition>,
) -> AnyElement {
    let value = settings.intensity(effect);
    let fraction = ((value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0);
    let track = px(SLIDER_WIDTH);

    let control = div()
        .flex()
        .flex_col()
        .gap_2()
        .p(px(CONTROL_PADDING))
        .rounded_md()
        .bg(rgba(0x00000033))
        .child(
            div()
                .flex()
                .justify_between()
                // 11px and uppercase, per the prototype's `.intensity-control
                // label`; GPUI has no letter-spacing to match the rest of it.
                .text_size(px(11.0))
                .text_color(rgba(0xaaaaaaff))
                .child(spec.label.to_uppercase())
                .child(
                    div()
                        .text_color(rgba(0xffffffff))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{:.*}{}", spec.decimals, value, spec.suffix)),
                ),
        )
        .child(
            div()
                .id(("effect-slider", effect.index()))
                .relative()
                .w(track)
                .h(px(SLIDER_HEIGHT))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        if !enabled {
                            return;
                        }
                        this.slider_drag = Some(SliderDrag {
                            effect,
                            start_x: event.position.x,
                            start_value: this.settings.intensity(effect),
                        });
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                        let Some(drag) = this.slider_drag else {
                            return;
                        };
                        if drag.effect != effect {
                            return;
                        }

                        let delta = (event.position.x - drag.start_x) / track;
                        let raw = drag.start_value + delta * (spec.max - spec.min);
                        let snapped = spec.min + ((raw - spec.min) / spec.step).round() * spec.step;
                        this.settings.set_intensity(effect, snapped);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.slider_drag = None;
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, _cx| {
                        this.slider_drag = None;
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px((SLIDER_HEIGHT - TRACK_HEIGHT) / 2.0))
                        .h(px(TRACK_HEIGHT))
                        .w_full()
                        .rounded_full()
                        .bg(rgba(0xffffff33)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px((SLIDER_HEIGHT - TRACK_HEIGHT) / 2.0))
                        .h(px(TRACK_HEIGHT))
                        .w(px(SLIDER_WIDTH * fraction))
                        .rounded_full()
                        .bg(accent(1.0)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px((SLIDER_HEIGHT - THUMB) / 2.0))
                        // Kept inside the track, so a value at either end does
                        // not hang off the edge.
                        .left(px((SLIDER_WIDTH * fraction - THUMB / 2.0)
                            .clamp(0.0, SLIDER_WIDTH - THUMB)))
                        .size(px(THUMB))
                        .rounded_full()
                        .bg(rgba(0xffffffff)),
                ),
        );

    // A control that is not armed is dimmed and inert, as in the prototype.
    match transition {
        Some(Transition { from, to, .. }) => control
            .animotion(
                ("effect-dim", effect.index()),
                vec![prop(tween(from, to, CONTROL_SECS), |el, opacity| {
                    el.opacity(opacity)
                })],
            )
            .into_any_element(),
        None => control.opacity(control_opacity(enabled)).into_any_element(),
    }
}

/// The readout, in the bottom-left corner.
fn readout(
    motion: &Motion,
    mode: Mode,
    triangles: usize,
    frame_time: Duration,
    walk_time: Duration,
    cap: Option<Duration>,
) -> Div {
    let scrolling = motion.is_scrolling();
    let millis = |time: Duration| time.as_secs_f32() * 1000.0;

    div()
        .absolute()
        .bottom(px(PANEL_MARGIN))
        .left(px(PANEL_MARGIN))
        .flex()
        .flex_col()
        // 12px/1.5, per the prototype's `#hud`.
        .text_size(px(12.0))
        .line_height(relative(1.5))
        .font_family(MONO_FONT)
        .text_color(grey(1.0, 0.5))
        .child(match mode {
            // The triangle count is the vector mode's whole per-frame CPU work
            // made visible: it is the same number at any magnification.
            Mode::Vector => format!("Mode: {} ({} triangles)", mode.label(), triangles),
            mode => format!("Mode: {}", mode.label()),
        })
        // The interval is the last frame as a whole — whatever asked for it, and
        // however far it fell short of the display's rate. The walk is the part
        // of it this file does: the elements' own building, which for the vector
        // mode is where its triangles are placed. A frame whose interval is much
        // larger than its walk is one the GPU (or the glyphs' rasterization) is
        // paying for.
        //
        // The cap is in there when there is one, because an interval only means
        // something next to it: a frame sitting on the cap is the cap working,
        // and one above it is the app not keeping up.
        .child(match cap {
            Some(cap) => format!(
                "Frame: {:.1} ms (cap {:.1}, walk {:.1})",
                millis(frame_time),
                millis(cap),
                millis(walk_time),
            ),
            None => format!(
                "Frame: {:.1} ms (walk {:.1})",
                millis(frame_time),
                millis(walk_time),
            ),
        })
        .child(format!("Velocity: {:.1} px/f", motion.velocity))
        .child(
            div()
                .text_color(if scrolling {
                    accent(1.0)
                } else {
                    grey(1.0, 0.5)
                })
                .child(if scrolling {
                    "State: Scrolling"
                } else {
                    "State: Settling/Resting"
                }),
        )
        .child(format!("Effect Intensity: {:.2}", motion.effect_blend))
        .child(format!("Altitude: {:.2}", motion.altitude_blend))
}

/// Everything the walk through the document needs: the lines themselves, what
/// the transform is doing to them, and how they are to be drawn.
#[derive(Clone)]
struct Scene<'a> {
    document: &'a [Line],
    motion: &'a Motion,
    settings: &'a Settings,
    mode: Mode,
    /// The window's text system, which shapes the vector mode's lines and
    /// tessellates its glyphs.
    text_system: &'a WindowTextSystem,
    /// The window's font: the vector mode's runs are shaped in it, so that a
    /// line's glyphs are the ones the text mode would set.
    font: Font,
    /// The vector mode's shaped lines, by document index.
    shaped: &'a RefCell<HashMap<usize, ShapedLine>>,
    /// Triangles the vector mode placed this frame, for the readout.
    triangles: &'a Cell<usize>,
    /// How many device pixels the window draws a logical one with, which is what
    /// decides how finely a glyph's outline has to be flattened.
    scale_factor: f32,
}

/// The lines on screen, laid out from the anchor outwards.
///
/// The prototype did this walk inside its render loop and drew bars into a
/// canvas; here the same walk hands back a div per line, and gpui shapes and
/// paints each one — as text, as the bars that stand in for its words, or as
/// the triangles of its glyphs' own outlines.
fn visible_lines(scene: Scene, viewport: Size<Pixels>) -> Vec<AnyElement> {
    let (document, motion, settings) = (scene.document, scene.motion, scene.settings);
    let height = f32::from(viewport.height);
    let center_x = f32::from(viewport.width) / 2.0;
    let center_y = height / 2.0;

    // The line at the anchor is placed first; the rest are laid out away from it
    // in both directions, each one a step as tall as the transform has made it.
    // Anchoring in the middle rather than walking down from the top is what keeps
    // whatever you are reading still.
    let scroll = motion.offset();
    let anchor_index = (scroll / BASE_SPACING).floor() as isize;
    let anchor_rel_y = anchor_index as f32 * BASE_SPACING - scroll;
    let anchor_y = center_y + anchor_rel_y * motion.scale_at(0.0, settings);

    let mut lines = Vec::new();

    let mut screen_y = anchor_y;
    for index in anchor_index..document.len() as isize {
        let rel_y = index as f32 * BASE_SPACING - scroll;
        let scale = motion.scale_at(rel_y, settings);
        if let Some(line) = line_element(scene.clone(), center_x, index as usize, screen_y, scale) {
            lines.push(line);
        }
        screen_y += BASE_SPACING * scale;
        if screen_y > height + OVERDRAW {
            break;
        }
    }

    let mut screen_y = anchor_y;
    for index in (0..anchor_index).rev() {
        let rel_y = index as f32 * BASE_SPACING - scroll;
        let scale = motion.scale_at(rel_y, settings);
        screen_y -= BASE_SPACING * scale;
        if let Some(line) = line_element(scene.clone(), center_x, index as usize, screen_y, scale) {
            lines.push(line);
        }
        if screen_y < -OVERDRAW {
            break;
        }
    }

    lines
}

/// One line of the document, drawn at the size the transform asks for and faded
/// for how fast it is moving.
///
/// Returns `None` for a line that has faded out entirely, so it is never laid
/// out, shaped or tessellated at all.
fn line_element(
    scene: Scene,
    center_x: f32,
    index: usize,
    screen_y: f32,
    scale: f32,
) -> Option<AnyElement> {
    let line = scene.document.get(index)?;
    let height = line.height() * scale;
    let start_x = (center_x - CONTENT_WIDTH / 2.0).max(40.0) + line.indent;
    let (luminance, opacity) = line_fade(line, &scene, scale);
    if opacity <= 0.01 {
        return None;
    }

    let color = grey(luminance, opacity);
    let element = div()
        .absolute()
        .left(px(start_x))
        .top(px(screen_y - (height / 2.0)));

    Some(match scene.mode {
        Mode::Text => element
            // The text is set at the size the transform asks for, rather than
            // scaled as a bitmap, so it stays sharp at any magnification. The size
            // is exact: the layout, and so every glyph's position and the line's
            // width, is what the transform asked for. It is the *rasterizer* that
            // rounds the size it keys a glyph on, which is what keeps the atlas
            // answering instead of cutting a new mask for every glyph of every line
            // of every frame. See `Window::GLYPH_RASTER_STEP` in the fork.
            .text_size(px(line.style.font_size() * scale))
            .line_height(relative(line.style.height() / line.style.font_size()))
            .when(line.style == Style::Heading, |el| {
                el.font_weight(FontWeight::BOLD)
            })
            .text_color(color)
            .child(line.text.clone())
            .into_any_element(),
        Mode::Skeleton => element
            .h(px(height))
            .children(skeleton(line, height, scale, color))
            .into_any_element(),
        Mode::Vector => {
            // The path is built here, where the text system is in reach, and
            // handed to the canvas to push at paint time: `paint_path` is only
            // callable while the frame is being painted.
            let path = vector_path(
                &scene,
                line,
                index,
                start_x,
                screen_y - (height / 2.0),
                scale,
            );
            canvas(
                |_, _, _| (),
                move |_, _, window, _cx| window.paint_path(path, color),
            )
            .absolute()
            .left(px(start_x))
            .top(px(screen_y - (height / 2.0)))
            .w(px(CONTENT_WIDTH * scale))
            .h(px(height))
            .into_any_element()
        }
    })
}

/// One line of the document as a single path of triangles, in window
/// coordinates.
///
/// A glyph's outline does not change shape with the size it is drawn at, so
/// every glyph here is a cached tessellation in em units and placing one is a
/// multiply per corner of its triangles. Nothing in here rasterizes anything,
/// which is the whole difference from [`Mode::Text`]: the same triangles are
/// placed again at whatever size each frame asks for, instead of a new mask
/// being cut for every glyph at every size.
fn vector_path(
    scene: &Scene,
    line: &Line,
    index: usize,
    left: f32,
    top: f32,
    scale: f32,
) -> Path<Pixels> {
    let mut path = Path::new(point(px(left), px(top)));
    // The app installs the Parley text system itself, so this is the text system
    // it is on; without it there are no outlines to ask for and the line is
    // simply not drawn.
    let Some(parley) = scene.text_system.as_parley() else {
        return path;
    };

    // The line's own copy, shaped the first frame it is on screen. The cache is
    // taken out of the scene first, so that holding it open does not stand in the
    // way of naming the rest of the scene below.
    let cache = scene.shaped;
    let mut shapes = cache.borrow_mut();
    let shaped = shapes.entry(index).or_insert_with(|| shape(scene, line));

    // The size the glyphs are drawn at, which is the size their outlines — in em
    // units — are multiplied by.
    let glyph_size = line.style.font_size() * scale;
    // Flattened for the size the glyph is drawn at, in device pixels, and for a
    // device pixel's worth of error there — so the facets of a magnified glyph
    // are finer than the transform can show, and one outline in each band serves
    // every line that lands in it.
    let band = band(glyph_size * scene.scale_factor);

    // Where the line's baseline sits: the same sum the text system's own painter
    // does — the box the document gives a line of this style, with the shaper's
    // ascent and descent centred in it.
    let height = line.height() * scale;
    let (ascent, descent) = (
        f32::from(shaped.ascent) * scale,
        f32::from(shaped.descent) * scale,
    );
    let baseline = top + (height - ascent - descent) / 2.0 + ascent;

    // The path's own bounds, gathered as the triangles are placed: the renderer
    // clips the path against them and the shader discards what is outside.
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);

    for run in &shaped.runs {
        for glyph in &run.glyphs {
            // A glyph the font has no outline for is one the text mode would have
            // drawn as a missing-glyph box; there is nothing to tessellate, so it
            // is left out rather than failing the line.
            let Ok(triangles) = parley.glyph_triangles(run.font_id, glyph.id, band, GLYPH_ERROR)
            else {
                continue;
            };

            // Where the shaper put the glyph, at the size this line is drawn at.
            let x = left + f32::from(glyph.position.x) * scale;
            let y = baseline + f32::from(glyph.position.y) * scale;

            // The corners are pushed straight onto the path rather than through
            // `Path::push_triangle`, which unions a `Bounds` for every corner it
            // is handed — three struct-wide updates per triangle, measured at
            // half again the cost of building the whole path. What those unions
            // were for is kept here, in plain floats.
            path.vertices.reserve(triangles.len() * 3);
            for triangle in triangles.iter() {
                for corner in triangle {
                    let (corner_x, corner_y) =
                        (x + corner.x.0 * glyph_size, y + corner.y.0 * glyph_size);
                    min_x = min_x.min(corner_x);
                    min_y = min_y.min(corner_y);
                    max_x = max_x.max(corner_x);
                    max_y = max_y.max(corner_y);
                    path.vertices.push(PathVertex {
                        xy_position: point(px(corner_x), px(corner_y)),
                        // All three corners in the solid part of the shader's
                        // implicit curve, which is what a tessellated glyph is:
                        // the outline's curvature is already in the triangle, so
                        // there is nothing left for the fragment stage to
                        // evaluate.
                        st_position: SOLID,
                    });
                }
            }
            scene.triangles.set(scene.triangles.get() + triangles.len());
        }
    }

    if !path.vertices.is_empty() {
        path.bounds = Bounds::new(
            point(px(min_x), px(min_y)),
            size(px(max_x - min_x), px(max_y - min_y)),
        );
    }
    path
}

/// Where a triangle's corners sit in the rasterizer's own parameterisation, to
/// be filled as solid triangles rather than evaluated as curve segments.
const SOLID: Point<f32> = point(0.0, 1.0);

/// The vector mode's copy of a line: shaped once, at the document's own size.
///
/// Shaping is what [`Mode::Text`] does afresh for every visible line of every
/// frame, at whatever size the transform is asking for. The vector mode needs it
/// once per line, because a glyph's place in its line and the width of the line
/// are proportional to the size the line is set at, and the outline itself does
/// not change with size at all — so one shaping answers for every size the line
/// is ever drawn at.
///
/// A line is shaped in the window's font, emboldened if the document calls it a
/// heading, which is what the text mode's own run does.
fn shape(scene: &Scene, line: &Line) -> ShapedLine {
    let mut font = scene.font.clone();
    if line.style == Style::Heading {
        font.weight = FontWeight::BOLD;
    }
    // The run's colour is the text mode's; the vector mode paints the outlines
    // itself, from the fade `line_fade` works out at paint time.
    let run = TextRun {
        len: line.text.len(),
        font,
        color: rgba(0xffffffff).into(),
        ..Default::default()
    };
    scene
        .text_system
        .shape_line(line.text.clone(), px(line.style.font_size()), &[run], None)
}

/// The bars a skeleton line is drawn from.
///
/// One per word, as wide as the word and spaced by the line's own word space, so
/// that a line of bars covers exactly the width its text would. A bar is about
/// as tall as a capital, sat on the middle of the line.
fn skeleton(line: &Line, height: f32, scale: f32, color: Hsla) -> Vec<AnyElement> {
    let bar = SKELETON_BAR * line.style.font_size() * scale;
    let mut x = 0.0;
    line.words
        .iter()
        .map(|word| {
            let left = x;
            x += word + line.gap;
            div()
                .absolute()
                .left(px(left * scale))
                .top(px((height - bar) / 2.0))
                .w(px(word * scale))
                .h(px(bar))
                .rounded(px(SKELETON_RADIUS * scale))
                .bg(color)
                .into_any_element()
        })
        .collect()
}

/// How a line is drawn at this instant: how bright it is, and how solid.
fn line_fade(line: &Line, scene: &Scene, scale: f32) -> (f32, f32) {
    let motion = scene.motion;
    let settings = scene.settings;

    // At altitude there is no distortion field and nothing to fade: the text is
    // simply set smaller, and left legible, wherever it is.
    if motion.altitude_blend > 0.001 {
        return (style_luminance(line.style), 1.0);
    }

    let abs_velocity = motion.abs_velocity();
    let blend = motion.effect_blend;

    // At rest everything is fully opaque; the more the transform has magnified
    // a line, the more solidly it is drawn.
    let base_opacity = ((scale - 0.2) * 1.5).clamp(0.0, 1.0);
    let mut opacity = 1.0 - (blend * (1.0 - base_opacity));

    // Kinetic fading: wash out whatever is currently large and fast. The
    // compensation is for text that is already moving too quickly to read, so
    // fading it removes noise instead of information.
    let mut wash = 0.0;
    if settings.is_on(Effect::Comfort) && scale > 1.0 && abs_velocity > 2.0 {
        let size_factor = ((scale - 1.0) / 2.0).min(1.0);
        let speed_factor = ((abs_velocity - 2.0) / 20.0).min(1.0);
        wash = size_factor * speed_factor * blend;
        opacity *= 1.0 - (wash * 0.8);
    }

    // The gravity well also thins out the far edges, which are the parts
    // travelling fastest across the screen. The prototype additionally stretched
    // each bar vertically here, standing in for motion blur; glyphs cannot be
    // smeared that way, so the fade carries the effect on its own.
    if settings.is_on(Effect::Gravity) && scale > 1.2 && abs_velocity > 10.0 {
        let intensity = settings.intensity(Effect::Gravity);
        opacity *= (1.0 - (abs_velocity * 0.015 * blend * intensity)).max(0.1);
    }

    // Washing out fades towards the page, in colour as well as alpha, so the
    // text disappears rather than turning grey.
    let mut luminance = style_luminance(line.style);
    if wash > 0.0 {
        luminance += (BACKGROUND_LUMINANCE - luminance) * wash * 0.9;
    }

    (luminance, opacity.clamp(0.0, 1.0))
}

/// How bright a line is set, before anything washes it out.
fn style_luminance(style: Style) -> f32 {
    match style {
        Style::Heading => 1.0,
        Style::Body => BODY_LUMINANCE,
    }
}

/// The reference band: a soft accent line through the middle of the viewport,
/// fading out towards both ends. It marks the line the transform anchors on —
/// the one line that is never distorted.
fn paint_anchor_line(bounds: Bounds<Pixels>, window: &mut Window) {
    const SEGMENTS: usize = 64;

    let left = f32::from(bounds.origin.x);
    let center_y = f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.0;
    let segment_width = f32::from(bounds.size.width) / SEGMENTS as f32;

    for segment in 0..SEGMENTS {
        let t = (segment as f32 + 0.5) / SEGMENTS as f32;
        let color = accent(0.3 * (std::f32::consts::PI * t).sin());
        let bounds = Bounds::new(
            point(px(left + (segment as f32 * segment_width)), px(center_y)),
            size(px(segment_width), px(1.0)),
        );
        window.paint_quad(fill(bounds, color));
    }
}

/// The corpus to scroll through, read from `assets/`.
///
/// The demo is built around one book, so the path is fixed. Without the file the
/// app still runs, with an empty document, and says so on stderr.
fn load_corpus() -> String {
    const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/war-and-peace.txt");
    match std::fs::read_to_string(CORPUS) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cool_scroll: could not read {CORPUS}: {error}");
            String::new()
        }
    }
}

/// The most frames a second the document is drawn at while it is moving, or
/// `None` for the display's own rate.
///
/// The motion is time-based rather than frame-based, so this changes nothing
/// about what a scroll looks like — only how many frames it is made of, and
/// therefore what it costs to watch one. A frame that is allowed to be 33ms late
/// instead of 16 has twice the room to arrive in, and the frames that arrive late
/// are the ones that read as stutter.
///
/// It is a *cap*, not a target: a build or a machine that cannot make 30 frames a
/// second still draws every frame it can, and the readout's `Frame:` line says
/// which of the two the app is actually doing. Off by default — a cap buys
/// steadiness at a lower rate, and if the frames themselves are what is expensive
/// it buys nothing at all.
const FRAME_CAP: Option<u32> = None;

/// The interval `FRAME_CAP` allows between frames, for reporting the frame rate
/// against something.
fn frame_cap_interval() -> Option<Duration> {
    FRAME_CAP.map(|cap| Duration::from_secs_f32(1.0 / cap as f32))
}

/// The environment variable that names the family, or the stack, the document is set
/// in.
///
/// A stack is a comma-separated list tried in order — the shape a desktop's font
/// configuration is written in — and it should end in a generic so that something
/// always resolves. The text system loads the host's own fonts, so a family here can
/// be one this machine has:
///
///     COOL_SCROLL_FONT="DejaVu Serif, Liberation Serif, serif" cargo run --release
///
/// Nothing is rebuilt to try one, and nothing has to be told what a face is: its
/// family, weight and slant are read out of the font itself.
const FONT_VAR: &str = "COOL_SCROLL_FONT";

/// The stack the document is set in when `FONT_VAR` says nothing.
///
/// The fonts a GNOME desktop reaches for, in the order it reaches for them, ending in
/// the generic that always resolves. Since the host's fonts are loaded, this is
/// Adwaita Sans on a machine that has the GNOME fonts and whatever comes next on one
/// that does not — rather than a font compiled into the app.
const DEFAULT_FONT: &str =
    "Adwaita Sans, Cantarell, Inter, Ubuntu, DejaVu Sans, Liberation Sans, sans-serif";

/// The stack the readout is set in: the order a GNOME terminal would use.
const MONO_FONT: &str = "Source Code Pro, Fira Mono, DejaVu Sans Mono, Liberation Mono, monospace";

/// The family or stack the document is set in.
fn document_family() -> SharedString {
    static FAMILY: OnceLock<SharedString> = OnceLock::new();
    FAMILY
        .get_or_init(|| match std::env::var(FONT_VAR) {
            Ok(family) if !family.trim().is_empty() => SharedString::from(family),
            _ => SharedString::from(DEFAULT_FONT),
        })
        .clone()
}

/// The font the document is set in: the window's own text style, in the family the
/// app was started with.
///
/// This is the only place the family is decided, because the two things that have to
/// agree about it are the measuring that wraps the document and the shaping that
/// draws it — given different families, the lines would be wrapped to one font's
/// widths and drawn in another's.
fn document_font(base: &TextStyle) -> Font {
    let mut font = base.font();
    font.family = document_family();
    font
}

/// The faces under `assets/fonts/`, for the text system to shape and cut with.
///
/// Everything the text system compiles in is already loaded, so this is the extra:
/// dropping a font in is all it takes to be able to name it. A file that cannot be
/// read is reported and skipped rather than failing the start-up.
fn loose_fonts() -> Vec<Cow<'static, [u8]>> {
    let mut fonts = Vec::new();
    let mut directories = vec![PathBuf::from("assets/fonts")];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("ttf" | "otf")
            ) {
                match std::fs::read(&path) {
                    Ok(data) => fonts.push(Cow::Owned(data)),
                    Err(error) => eprintln!("{}: {error}", path.display()),
                }
            }
        }
    }
    fonts
}

fn main() {
    // The extra faces go in before the window exists: the document is measured
    // through the text system on its first frame.
    let text_system = ParleyTextSystem::new();
    text_system
        .add_fonts(loose_fonts())
        .expect("the fonts under assets/fonts should load");

    application()
        // Frames arrive at the display's rate by default. `ThrottledPipeline`
        // defers the ones that come sooner than the cap allows, leaving them
        // pending rather than dropping them, so the document still moves at the
        // same speed and only the number of frames it is drawn in changes. See
        // `FRAME_CAP`.
        .with_frame_pipeline(|_window_id| match FRAME_CAP {
            Some(cap) => Box::new(StandardImmediatePipeline.max_fps(cap)),
            None => Box::new(StandardImmediatePipeline),
        })
        // Shape and lay out the document through Parley rather than the default
        // engine: `gpui_parley` implements the same `TextSystem` SPI on top of
        // Parley, Skrifa and tiny-skia, with IBM Plex Sans compiled in and the rest
        // of `assets/fonts/` handed to it above. See `patches/gpui_parley`.
        .with_text_system(text_system)
        .run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.0), px(520.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Perceptual Motion Engine".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_window, cx| cx.new(|_| CoolScroll::new()),
            )
            .expect("failed to open the main window");

            cx.activate(true);
        });
}

/// Tests of the view: the walk, the gestures, the panel and the text system.
///
/// They drive a real window on the headless platform, which is `gpui`'s
/// `test-support`, so they sit behind this crate's feature of the same name:
/// `cargo test --features test-support`. The tests in `document.rs` and
/// `motion.rs` need no window, and run either way.
#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers, Point, TestAppContext, TouchPhase, VisualTestContext};
    use gpui_parley::FONT_FAMILY;

    /// A short corpus, so most tests do not wrap the whole book on their first
    /// frame — but still several screens of prose, so the walk has something to
    /// do.
    fn view() -> CoolScroll {
        let mut source = String::from("CHAPTER I\n\n");
        for _ in 0..12 {
            source.push_str(
                "It was in July, 1805, and the speaker was the well-known Anna Pavlovna \
                 Scherer, maid of honor and favorite of the Empress Marya Fedorovna.\n\n",
            );
        }
        CoolScroll::with_corpus(source)
    }

    /// Install the Parley text system, as `main` does.
    ///
    /// A test app starts on gpui's own text engine, so this is what keeps the
    /// tests on the text path the app ships — and what puts the vendored copy of
    /// `gpui_parley` in the way of every one of them.
    fn use_parley(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_text_system(ParleyTextSystem::new()));
    }

    /// The app shapes its text through Parley rather than gpui's default engine,
    /// which is the whole point of the copy in `patches/`.
    #[gpui::test]
    fn the_app_shapes_its_text_with_parley(cx: &mut TestAppContext) {
        // A test app on its own is not on Parley, so the assertion at the end is
        // about what the app installs, not about how tests are set up.
        assert!(cx.update(|cx| cx.text_system().as_parley().is_none()));

        use_parley(cx);
        let (_view, cx) = cx.add_window_view(|_window, _cx| view());

        assert!(
            cx.update(|_window, app| app.text_system().as_parley().is_some()),
            "the app should shape its text through Parley"
        );
    }

    /// The window paints a viewport's worth of words and its chrome, and a
    /// wheel event gets the document moving.
    #[gpui::test]
    fn paints_the_document_and_scrolls(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        let quads = cx.update(|window, _app| window.painted_quads());
        assert!(
            quads
                .iter()
                .any(|quad| quad.background == rgba(0x141414d9).into()),
            "expected the controls panel to be painted"
        );

        // The document is text rather than painted bars, so a screenful is
        // checked on the walk that hands lines to the element tree. The walk is
        // what a frame does, so all three modes walk the same document.
        let text = walk(cx, &view, Mode::Text);
        assert!(
            text.lines > 20,
            "expected a screenful of lines, walked {}",
            text.lines
        );
        assert_eq!(
            text.triangles, 0,
            "the text mode draws glyphs, not the outlines of them"
        );

        let skeleton = walk(cx, &view, Mode::Skeleton);
        assert_eq!(
            skeleton.lines, text.lines,
            "the modes walk the same document"
        );
        assert_eq!(
            skeleton.triangles, 0,
            "the skeleton mode draws bars, not outlines"
        );

        let vector = walk(cx, &view, Mode::Vector);
        assert_eq!(vector.lines, text.lines, "the modes walk the same document");
        assert!(
            vector.triangles > 1000,
            "a screenful of outlines should be thousands of triangles, placed {}",
            vector.triangles
        );

        // GPUI's wheel deltas run the opposite way to the DOM's: negative y is a
        // scroll down. Both directions are pinned here, because getting the sign
        // backwards is invisible to every other assertion in this file.
        let document = point(px(400.0), px(350.0));
        let start = target(cx, &view);

        // A wheel down, which should move the document on.
        wheel(cx, document, -300.0);
        let down = target(cx, &view);
        assert!(
            down > start,
            "a downward wheel should carry the document on: {start} -> {down}"
        );

        // And a wheel up, which should bring it back.
        wheel(cx, document, 300.0);
        let up = target(cx, &view);
        assert!(
            up < down,
            "an upward wheel should bring it back: {down} -> {up}"
        );

        let needs_frames = cx.update(|_window, app| view.read(app).motion.needs_frames());
        assert!(
            needs_frames,
            "a wheel event should keep the animation alive"
        );
    }

    /// The corpus in `assets/` is read, parsed and wrapped: the path is right,
    /// the byte order mark and the CRs are handled, and chapter headings are
    /// recognised. This is the one test that touches the real book.
    #[gpui::test]
    fn the_corpus_loads_and_wraps(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| CoolScroll::new());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        let document = cx.update(|_window, app| view.read(app).document.clone());

        assert_eq!(
            document.first().map(|line| line.style),
            Some(Style::Body),
            "the book opens with a paragraph, not a chapter"
        );
        assert!(
            document[0].text.starts_with("\"Well, Prince, so Genoa"),
            "the byte order mark should be gone: {:?}",
            document[0].text
        );
        assert!(
            document
                .iter()
                .filter(|line| line.style == Style::Heading)
                .count()
                > 10,
            "chapter headings should be recognised"
        );
        assert_eq!(
            document.len(),
            document::MAX_LINES,
            "a book this long should fill the line cap"
        );
    }

    /// A wheel event at `position`, in gpui's convention: a negative delta y
    /// scrolls down.
    fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>, delta_y: f32) {
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.0), px(delta_y))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
    }

    /// The document's scroll target: where the app has been asked to go.
    fn target(cx: &mut VisualTestContext, view: &Entity<CoolScroll>) -> f32 {
        cx.update(|_window, app| view.read(app).motion.target_scroll)
    }

    /// The document's position: where it actually is, which is not the same
    /// question as where it has been asked to go.
    fn offset(cx: &mut VisualTestContext, view: &Entity<CoolScroll>) -> f32 {
        cx.update(|_window, app| view.read(app).motion.offset())
    }

    /// A wheel event over the panel belongs to the panel: it scrolls the panel's
    /// own content, and never the document behind it.
    #[gpui::test]
    fn wheel_over_the_panel_does_not_scroll_the_document(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        // Short enough that the panel's content overflows and has somewhere to go.
        cx.simulate_resize(size(px(1000.0), px(400.0)));
        cx.run_until_parked();

        let panel = cx
            .debug_bounds("controls")
            .expect("the panel should be rendered");
        // A point in the panel's padding, clear of any switch or slider.
        let over_panel = point(panel.left() + px(8.0), panel.bottom() - px(8.0));
        let over_document = point(px(300.0), px(300.0));

        let before = target(cx, &view);
        let switch_before = cx
            .debug_bounds("switch-altitude")
            .expect("the altitude switch should be rendered");

        wheel(cx, over_panel, -300.0);
        pump(cx);

        assert_eq!(
            target(cx, &view),
            before,
            "a wheel over the panel should leave the document alone"
        );

        let switch_after = cx
            .debug_bounds("switch-altitude")
            .expect("the altitude switch should still be rendered");
        assert!(
            switch_after.top() < switch_before.top(),
            "the panel should have scrolled its own content: {switch_before:?} then {switch_after:?}"
        );

        // And the document still answers the wheel when the pointer is over it.
        wheel(cx, over_document, -300.0);
        assert!(
            target(cx, &view) > before,
            "a wheel over the document should still scroll it"
        );
    }

    /// Flipping a switch springs its knob across, and the spring is dropped once
    /// it has settled so the window can go quiet again.
    #[gpui::test]
    fn toggling_a_switch_springs_its_knob(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        // Click the parallax switch, which starts armed. At rest the knob is
        // drawn statically, so where it sits now is exact.
        let switch = cx
            .debug_bounds("switch-parallax")
            .expect("the parallax switch should be rendered");
        let armed = knob(cx);
        assert!(
            f32::from(armed.origin.x - (switch.origin.x + px(knob_offset(true)))).abs() < 2.0,
            "an armed switch should place its knob at the armed offset: {armed:?} vs {switch:?}"
        );

        cx.simulate_mouse_move(switch.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(switch.center(), Modifiers::default());
        assert!(!cx.update(|_window, app| view.read(app).settings.is_on(Effect::Parallax)));
        assert!(
            cx.update(
                |_window, app| view.read(app).switches.knob[Effect::Parallax.index()].is_some()
            ),
            "flipping the switch should arm a spring"
        );

        // A spring advances in real time, so let some pass before handing the
        // element the frame it asked for.
        std::thread::sleep(Duration::from_millis(60));
        pump(cx);
        let moving = knob(cx);
        assert!(
            moving.origin.x < armed.origin.x,
            "the knob should be springing left: {armed:?} then {moving:?}"
        );

        // The track is tweening from #333 to the accent on the same clock, so
        // one quad should be sitting between the two rather than at either end.
        // Everything else in the frame is either grey (saturation 0) or the
        // accent itself.
        let tweening: Vec<f32> = cx.update(|window, _app| {
            window
                .painted_quads()
                .into_iter()
                .map(|quad| quad.background.solid.s)
                .filter(|saturation| *saturation > 0.05 && *saturation < 0.95)
                .collect()
        });
        assert_eq!(
            tweening.len(),
            1,
            "expected one switch fill mid-tween, got {tweening:?}"
        );

        // Once settled the transition is dropped, and the knob is drawn from the
        // switch's own state rather than from an animation.
        std::thread::sleep(Duration::from_millis(1000));
        pump(cx);
        assert!(
            cx.update(
                |_window, app| view.read(app).switches.knob[Effect::Parallax.index()].is_none()
            ),
            "a settled spring should be dropped"
        );
        let settled = knob(cx);
        assert!(
            f32::from(settled.origin.x - (switch.origin.x + px(knob_offset(false)))).abs() < 2.0,
            "the knob should end where the unarmed switch puts it: {settled:?}"
        );
    }

    /// Dragging takes the document with the pointer, and letting go of a drag
    /// that was still moving throws it: a kinetic flick carries the document on
    /// past where the pointer last asked it to be.
    ///
    /// Drawn as bars, and in a window small enough that a frame of it is quick,
    /// because what is being tested here is the pointer — see [`draw_bars`] for
    /// what a slow frame does to a flick's measurement. For the same reason the
    /// moves are a hand's short flick rather than a leisurely one: the samples a
    /// flick is built from are taken against the clock, and a frame drawn between
    /// two of them must not cost so much that the next lands stale.
    #[gpui::test]
    fn dragging_throws_the_document(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(400.0), px(300.0)));
        cx.run_until_parked();
        draw_bars(&view, cx);

        let before = offset(cx, &view);
        // Clear of the panel, which owns the top-right corner.
        let (x, mut y) = (40.0, 200.0);
        cx.simulate_mouse_down(point(px(x), px(y)), MouseButton::Left, Modifiers::default());

        for _ in 0..6 {
            // Real gaps between the moves: a velocity tracker needs an interval
            // to measure, and a hand takes one. Short, so that several land
            // inside the window the tracker keeps whichever the frame costs.
            std::thread::sleep(Duration::from_millis(10));
            y -= 12.0;
            cx.simulate_mouse_move(point(px(x), px(y)), MouseButton::Left, Modifiers::default());
        }

        let dragged = offset(cx, &view);
        assert!(
            dragged > before,
            "dragging up should carry the document on: {before} -> {dragged}"
        );

        // Where the pointer asked the document to be, against where the flick
        // then takes it: letting go should overshoot the drag by itself.
        let aim = target(cx, &view);
        cx.simulate_mouse_up(point(px(x), px(y)), MouseButton::Left, Modifiers::default());

        for _ in 0..60 {
            pump(cx);
        }
        let coasted = offset(cx, &view);
        assert!(
            coasted > aim,
            "letting go of a moving drag should throw the document past where the \
             pointer asked for it: {aim} -> {coasted}"
        );
    }

    /// Grabbing the document calls off a flick that is already in flight.
    #[gpui::test]
    fn grabbing_calls_off_a_flick(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();
        draw_bars(&view, cx);

        // Throw the document by hand, and let it get up to speed.
        cx.update(|_window, app| view.update(app, |this, _cx| this.motion.release(3000.0)));
        for _ in 0..8 {
            pump(cx);
        }
        assert!(
            cx.update(|_window, app| view.read(app).motion.needs_frames()),
            "the flick should be in flight"
        );

        // Grabbing it stops it where it is, rather than letting it run on.
        cx.simulate_mouse_down(
            point(px(500.0), px(350.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        let grabbed = offset(cx, &view);

        for _ in 0..60 {
            pump(cx);
        }
        let settled = offset(cx, &view);
        assert!(
            (settled - grabbed).abs() < 0.01,
            "a grabbed document should not move: {grabbed} -> {settled}"
        );
    }

    /// The skeleton mode is the same document drawn differently: it paints the
    /// lines as bars, and switching back paints their text again.
    #[gpui::test]
    fn skeleton_mode_draws_bars_instead_of_text(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        let text = quads(cx);
        let skeleton = mode_cell(cx, Mode::Skeleton);

        cx.simulate_mouse_move(skeleton.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(skeleton.center(), Modifiers::default());
        assert_eq!(
            cx.update(|_window, app| view.read(app).mode),
            Mode::Skeleton
        );
        pump(cx);

        // Text is glyphs rather than quads, so a screenful of bars is a
        // screenful of quads that were not there before.
        let skeleton = quads(cx);
        assert!(
            skeleton > text + 100,
            "a screenful of bars should be a screenful of quads: {text} -> {skeleton}"
        );

        let back = mode_cell(cx, Mode::Text);
        cx.simulate_click(back.center(), Modifiers::default());
        assert_eq!(cx.update(|_window, app| view.read(app).mode), Mode::Text);
        pump(cx);

        let back = quads(cx);
        assert!(
            back < skeleton - 100,
            "switching back should take the bars away again: {skeleton} -> {back}"
        );
        // Within a couple of quads: the selector's own cells are painted from an
        // animation or from their state, which need not be the same number of
        // quads, but the document's own bars must all be gone.
        assert!(
            (back as f32 - text as f32).abs() <= 2.0,
            "text mode should draw the document as it did before: {text} -> {back}"
        );
    }

    /// Selecting a mode lights its cell up on the same clock the switches use, and
    /// the animation is dropped once it has settled, so a settled selector asks
    /// for no frames.
    #[gpui::test]
    fn selecting_a_mode_animates_its_cell(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        let cell = mode_cell(cx, Mode::Vector);
        cx.simulate_mouse_move(cell.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(cell.center(), Modifiers::default());

        // The cell giving the selection up and the one taking it both move, or
        // the selection would jump out from under the one that is leaving.
        let tweening = cx.update(|_window, app| view.read(app).switches.mode);
        assert!(
            tweening[Mode::Text.index()].is_some() && tweening[Mode::Vector.index()].is_some(),
            "both cells should be tweening"
        );

        std::thread::sleep(Duration::from_millis(1000));
        pump(cx);
        let settled = cx.update(|_window, app| view.read(app).switches.mode);
        assert!(
            settled.iter().all(Option::is_none),
            "a settled selector should be dropped"
        );
    }

    /// The vector mode draws the document as the outlines of its glyphs: the
    /// walk places triangles for every line, and shapes each of them once.
    #[gpui::test]
    fn vector_mode_draws_glyph_outlines(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        let vector = walk(cx, &view, Mode::Vector);
        assert!(
            vector.triangles > 1000,
            "a screenful of outlines should be thousands of triangles, placed {}",
            vector.triangles
        );

        // Outlines are neither quads nor glyphs, so the headless window has no
        // way to show them; the walk's own count is what says they were placed.
        // What the same walk *does* have to hand over is one shaped line per
        // line drawn — not one per frame, which is the whole point of the mode.
        let shaped = cx.update(|_window, app| view.read(app).shaped.borrow().len());
        assert_eq!(
            shaped, vector.lines,
            "the vector mode should shape each line once, not once a frame"
        );

        walk(cx, &view, Mode::Vector);
        let again = cx.update(|_window, app| view.read(app).shaped.borrow().len());
        assert_eq!(again, shaped, "a second frame should shape nothing new");

        // And the same lines, still drawn by the same walk.
        let cell = mode_cell(cx, Mode::Vector);
        cx.simulate_mouse_move(cell.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(cell.center(), Modifiers::default());
        assert_eq!(cx.update(|_window, app| view.read(app).mode), Mode::Vector);
        pump(cx);
        assert_eq!(
            walk(cx, &view, Mode::Vector).lines,
            vector.lines,
            "the mode should not have moved the document"
        );
    }

    /// A line drawn as vectors is the line the shaper laid out: the path spans
    /// it, starts where it starts, and its ink is the right size and the right
    /// way up.
    ///
    /// The window cannot show the outlines, but the path that carries them is
    /// just geometry, so where it landed can be read off its own vertices.
    #[gpui::test]
    fn a_vector_line_follows_the_shaped_line(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        // Where the walk would put the line, and at the size the document asks
        // for, so the numbers below are the document's own.
        const LEFT: f32 = 100.0;
        const TOP: f32 = 0.0;
        let (line, path, shaped) = cx.update(|window, app| {
            let view = view.read(app);
            let triangles = Cell::new(0);
            let scene = Scene {
                document: &view.document,
                motion: &view.motion,
                settings: &view.settings,
                mode: Mode::Vector,
                text_system: window.text_system(),
                font: document_font(&window.text_style()),
                shaped: &view.shaped,
                triangles: &triangles,
                scale_factor: window.scale_factor(),
            };
            let index = view
                .document
                .iter()
                .position(|line| line.style == Style::Body)
                .expect("the corpus should wrap into prose");
            let line = view.document[index].clone();
            let path = vector_path(&scene, &line, index, LEFT, TOP, 1.0);
            // Drawing a line shapes it and keeps the result, so the shaper's own
            // numbers can be read back off the cache rather than asked for twice.
            let shaped = view
                .shaped
                .borrow()
                .get(&index)
                .cloned()
                .expect("the vector mode should have shaped the line it drew");
            (line, path, shaped)
        });

        let (mut ink_left, mut ink_top) = (f32::INFINITY, f32::INFINITY);
        let (mut ink_right, mut ink_bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for vertex in &path.vertices {
            let x = f32::from(vertex.xy_position.x);
            let y = f32::from(vertex.xy_position.y);
            ink_left = ink_left.min(x);
            ink_top = ink_top.min(y);
            ink_right = ink_right.max(x);
            ink_bottom = ink_bottom.max(y);
        }

        let size = line.style.font_size();
        let width = f32::from(shaped.width);
        // Where the shaper says the baseline is. Its glyphs sit on it already —
        // `position.y` is measured down from the top of the line's box — and the
        // text mode's own painter centres the box's leading around it, which is
        // the sum this repeats. A line drawn as vectors has to land on the same
        // baseline, or the two modes could not be compared.
        let baseline = TOP
            + (line.height() - f32::from(shaped.ascent) - f32::from(shaped.descent)) / 2.0
            + f32::from(shaped.ascent)
            + f32::from(shaped.runs[0].glyphs[0].position.y);

        // Outlines come back in em units, so the ink comes to about the size of
        // the text, standing on the baseline: a cap's height above it, and no
        // more than a descender below. A missing or doubled size would show up
        // as a sliver or as something four times the size, and an outline drawn
        // the wrong way up would hang its ink below the baseline instead.
        let above = baseline - ink_top;
        let below = ink_bottom - baseline;
        assert!(
            above > 0.6 * size && above < 1.05 * size,
            "the tallest ink should stand a cap above the baseline, not {above} on {size}px text"
        );
        assert!(
            (-1.0..0.35 * size).contains(&below),
            "the lowest ink should hang below the baseline by a descender at most, not {below}"
        );

        // And the line is the shaper's line: starting where the line starts,
        // give or take the first glyph's left side bearing, and spanning it.
        assert!(
            (0.0..20.0).contains(&(ink_left - LEFT)),
            "the line should start at its own origin: {}",
            ink_left - LEFT
        );
        let ink_width = ink_right - ink_left;
        assert!(
            ink_width > width - 10.0 && ink_width <= width,
            "the line's ink should span the shaper's own line: {ink_width} of {width}"
        );
    }

    /// Draw the document as skeleton bars rather than as text.
    ///
    /// The tests that drive the pointer want this, and not for speed alone. A
    /// pointer's speed is measured in wall-clock time — the flick a release
    /// throws is built from samples taken inside a fifth of a second of it — and
    /// the harness draws a frame between two events. A debug build spends about
    /// a second on a screenful of text, and rather more while a flick has the
    /// transform magnifying lines, so every sample but the last would fall
    /// outside that window and the flick would be measured as no movement at
    /// all: the assertions would then hold or fail on how fast the machine is.
    /// Bars are quads, and cost the same to draw at any magnification.
    fn draw_bars(view: &Entity<CoolScroll>, cx: &mut VisualTestContext) {
        cx.update(|_window, app| view.update(app, |this, cx| this.set_mode(Mode::Skeleton, cx)));
    }

    /// A stack resolves in order, and a script none of its families has got falls
    /// back to a face of its own.
    ///
    /// The first half is what makes a stack worth naming: the families are tried in
    /// the order written and the first that can shape the text is the one it is set
    /// in, so naming the desktop's own chain gets the desktop's own font. The second
    /// is what makes naming one safe: a character the stack cannot draw is shaped
    /// from whatever can, and that run has to carry the face it was shaped with —
    /// its glyph ids mean nothing to any other face.
    #[test]
    fn a_script_the_stack_has_not_got_falls_back_to_its_own_face() {
        let text_system = ParleyTextSystem::new();
        text_system
            .add_fonts(loose_fonts())
            .expect("the fonts under assets/fonts should load");

        let layout = |text: &str| {
            let run = gpui::FontRun {
                font_id: text_system.resolve_font(&Font {
                    family: document_family(),
                    ..Default::default()
                }),
                len: text.len(),
            };
            text_system.layout_line(text, px(14.0), &[run], None)
        };
        // The stack has an answer for Latin: it ends in a generic, so it resolves,
        // and Latin comes from the one face it resolves to.
        let latin = layout("The quick brown fox");
        assert!(latin.width.0 > 0.0, "the default stack should shape");
        let mut latin_faces: Vec<usize> = latin.runs.iter().map(|run| run.font_id.0).collect();
        latin_faces.dedup();
        assert_eq!(
            latin_faces.len(),
            1,
            "Latin should come from one face of the stack, not {latin_faces:?}"
        );

        // Japanese, which no Latin face in the stack has got. A machine that can
        // draw it at all draws it from some other face; one that cannot leaves the
        // run on the stack's own face, and then there is nothing here to check.
        let japanese = layout("日本語");
        let mut japanese_faces: Vec<usize> =
            japanese.runs.iter().map(|run| run.font_id.0).collect();
        japanese_faces.dedup();
        if japanese_faces == latin_faces {
            eprintln!("no font here draws 日本語; the stack's own face stands in for it");
            return;
        }

        // And a line mixing the two draws from both faces, each run keeping the
        // face it was shaped with rather than the family that was asked for.
        let mixed = layout("The quick brown fox 日本語");
        let mut faces: Vec<usize> = mixed.runs.iter().map(|run| run.font_id.0).collect();
        faces.dedup();
        assert!(
            faces.len() >= 2 && japanese_faces.iter().all(|face| faces.contains(face)),
            "a mixed line should be shaped from both faces, not {faces:?}"
        );
    }

    /// A family that was never loaded draws the compiled-in one rather than nothing.
    ///
    /// Loading a font is additive: `add_fonts` puts it in the one collection the
    /// shaper and the rasterizer both read, so this is the guard on that — a name
    /// nobody loaded, which is what the readout asks for with its `monospace`, has
    /// to fall back instead of shaping to an empty line.
    #[test]
    fn an_unknown_family_falls_back_to_the_loaded_one() {
        let text_system = ParleyTextSystem::new();
        text_system
            .add_fonts(loose_fonts())
            .expect("the fonts under assets/fonts should load");

        let width = |family: &str| {
            let font = Font {
                family: SharedString::from(family),
                ..Default::default()
            };
            let run = gpui::FontRun {
                font_id: text_system.resolve_font(&font),
                len: 19,
            };
            f32::from(
                text_system
                    .layout_line("The quick brown fox", px(14.0), &[run], None)
                    .width,
            )
        };

        assert!(
            text_system
                .all_font_names()
                .iter()
                .any(|name| name == FONT_FAMILY),
            "the compiled-in family should always be loaded"
        );
        assert!(
            width(FONT_FAMILY) > 0.0,
            "the compiled-in family should shape"
        );
        assert_eq!(
            width("No Such Family"),
            width(FONT_FAMILY),
            "an unknown family should draw the compiled-in one"
        );
    }

    /// Where a frame goes, in this build and on this machine.
    ///
    /// Prints rather than asserts, so it is ignored by default:
    ///
    ///     cargo test --features test-support -- --ignored --nocapture a_frame
    ///
    /// Each row is a mode drawn at rest and then with a flick in flight, since
    /// the two are very different pictures: a flick has the transform magnifying
    /// lines, and in a debug build a magnified glyph is the most expensive thing
    /// a frame can contain. The interval is the whole frame; the walk is the
    /// share of it spent in this file, building the document's elements.
    #[gpui::test]
    #[ignore = "prints timings instead of asserting"]
    fn a_frame_is_spent(cx: &mut TestAppContext) {
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        for mode in Mode::ALL {
            for action in ["rest", "flick"] {
                cx.update(|_window, app| {
                    view.update(app, |this, cx| {
                        this.set_mode(mode, cx);
                        if action == "flick" {
                            this.motion.release(2000.0);
                        } else {
                            this.motion.grab();
                        }
                    })
                });

                let mut frames = Vec::new();
                let mut walks = Vec::new();
                for _ in 0..8 {
                    pump(cx);
                    cx.update(|_window, app| {
                        let view = view.read(app);
                        frames.push(view.frame_time.as_secs_f32() * 1000.0);
                        walks.push(view.walk_time.as_secs_f32() * 1000.0);
                    });
                }
                let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
                println!(
                    "{:<9} {action:<6} frame {:>8.1} ms   walk {:>7.1} ms",
                    mode.label(),
                    mean(&frames),
                    mean(&walks),
                );
            }
        }
    }

    /// The app puts its windows on a throttled frame pipeline, and a frame asked
    /// for sooner than the cap allows is deferred rather than dropped: the
    /// window is left dirty and a later frame draws it.
    ///
    /// The cap here is not the app's. A frame of this document costs tens of
    /// milliseconds even in a debug build, which is already slower than
    /// `FRAME_CAP` allows, so a test cannot hold a frame back with it — what is
    /// being tested is that the window is on a throttled pipeline at all, and
    /// that is the same wiring whatever the rate. The rest is the pipeline's.
    ///
    /// It also has to be asked the way the platform asks. `Window::draw` draws
    /// whatever the pipeline says, because deciding *whether* to draw belongs to
    /// the requester; the harness's own frame flush is the thing that asks, and
    /// so the thing a notify is left to.
    #[gpui::test]
    fn a_frame_inside_the_cap_is_deferred(cx: &mut TestAppContext) {
        const TEST_CAP: u32 = 5;
        let cap = Duration::from_secs_f32(1.0 / TEST_CAP as f32);

        // The same factory the application method forwards to, so this is the
        // wiring `main` uses.
        cx.update(|cx| {
            cx.set_frame_pipeline_factory(Rc::new(|_window_id| {
                Box::new(StandardImmediatePipeline.max_fps(TEST_CAP))
            }))
        });
        use_parley(cx);
        let (view, cx) = cx.add_window_view(|_window, _cx| view());
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();

        // What the last *drawn* frame left behind: only a frame that was built
        // can change it.
        let drawn = |cx: &mut VisualTestContext| cx.update(|_window, app| view.read(app).walk_time);
        let ask = |view: &Entity<CoolScroll>, cx: &mut VisualTestContext| {
            cx.update(|_window, app| view.update(app, |_this, cx| cx.notify()));
        };

        let first = drawn(cx);

        // Asked for immediately, which is well inside the cap: the frame the
        // notify asks for should be deferred.
        ask(&view, cx);
        assert_eq!(
            drawn(cx),
            first,
            "a frame inside the cap should be deferred, not built"
        );

        // And asked for once the cap has passed, which should be drawn.
        std::thread::sleep(cap);
        ask(&view, cx);
        let built = drawn(cx);
        assert_ne!(
            built, first,
            "a frame the cap allows should be built, not deferred"
        );
    }

    /// Where the document's mode selector draws the cell for `mode`.
    fn mode_cell(cx: &mut VisualTestContext, mode: Mode) -> Bounds<Pixels> {
        cx.debug_bounds(mode.key())
            .expect("the document selector should be rendered")
    }

    /// How many lines the view lays out for a 1000x700 viewport in `mode`,
    /// counted the way a frame counts them, and how many triangles the vector
    /// mode placed for them.
    struct Walk {
        lines: usize,
        triangles: usize,
    }

    fn walk(cx: &mut VisualTestContext, view: &Entity<CoolScroll>, mode: Mode) -> Walk {
        cx.update(|window, app| {
            let view = view.read(app);
            let triangles = Cell::new(0);
            let lines = visible_lines(
                Scene {
                    document: &view.document,
                    motion: &view.motion,
                    settings: &view.settings,
                    mode,
                    text_system: window.text_system(),
                    font: document_font(&window.text_style()),
                    shaped: &view.shaped,
                    triangles: &triangles,
                    scale_factor: window.scale_factor(),
                },
                size(px(1000.0), px(700.0)),
            )
            .len();
            Walk {
                lines,
                triangles: triangles.get(),
            }
        })
    }

    /// How many quads the last drawn frame painted.
    fn quads(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, _app| window.painted_quads().len())
    }

    /// Deliver the frame the animating element asked for, then draw it.
    fn pump(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }

    /// Where the parallax knob is in the last drawn frame.
    fn knob(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("knob-parallax")
            .expect("the parallax knob should be rendered")
    }
}
