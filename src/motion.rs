//! Scroll physics and the perceptual transform.
//!
//! The simulation is written in the prototype's own units: pixels per 60fps
//! frame. [`Motion::advance`] takes elapsed real time and converts it into
//! those units, so the feel of the original per-frame prototype survives on a
//! 120Hz display or a machine that drops frames.
//!
//! Where the document *is* is not simulated by hand. It is a [`Prop<f32>`] —
//! one of `gpui_animotion`'s interruptible properties — and every way of moving
//! it is one of that crate's segments: a wheel tick tweens to the accumulated
//! target, a pointer move re-aims a spring, letting go of a drag hands its
//! velocity to a kinetic flick, and taking hold stops it dead. What is
//! simulated here is everything the prototype layered on top of position: the
//! blend that switches the effects on, the altitude the document climbs, and
//! the direction it is tilting.

use std::time::Duration;

use gpui_animotion::{Ease, FlickParams, Prop, SpringParams};

/// How many of the engine's frames make a second.
pub const FRAMES_PER_SECOND: f32 = 60.0;

/// One of the five candidate approaches the demo lets you compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Linear tilt that flips with your direction of travel.
    Parallax,
    /// A symmetrical cylinder: the centre bulges, the edges curve away.
    Barrel,
    /// A kinetic gravity well: the centre stays calm while the edges run away.
    Gravity,
    /// Fading motion blur, applied to whatever is currently large and fast.
    Comfort,
    /// Climbing: shrink the document instead of distorting it.
    Altitude,
}

impl Effect {
    /// Every effect, in the order the panel shows them.
    pub const ALL: [Effect; 5] = [
        Effect::Parallax,
        Effect::Barrel,
        Effect::Gravity,
        Effect::Comfort,
        Effect::Altitude,
    ];

    pub const fn index(self) -> usize {
        match self {
            Effect::Parallax => 0,
            Effect::Barrel => 1,
            Effect::Gravity => 2,
            Effect::Comfort => 3,
            Effect::Altitude => 4,
        }
    }

    /// A short stable name, for element ids and test selectors.
    pub const fn key(self) -> &'static str {
        match self {
            Effect::Parallax => "parallax",
            Effect::Barrel => "barrel",
            Effect::Gravity => "gravity",
            Effect::Comfort => "comfort",
            Effect::Altitude => "altitude",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Effect::Parallax => "1. Parallax Horizon",
            Effect::Barrel => "2. Rotating Barrel",
            Effect::Gravity => "3. Black Hole",
            Effect::Comfort => "4. Kinetic Fading (Comfort)",
            Effect::Altitude => "5. Altitude",
        }
    }

    /// What this effect is going for, and where it fights itself. Copied from
    /// the prototype: the claims are the interesting part of the demo.
    pub const fn description(self) -> &'static str {
        match self {
            Effect::Parallax => {
                "Linear tilt. Direction dynamically flips based on your scroll travel. \
                 Stable IF you're looking at the reference band — not otherwise."
            }
            Effect::Barrel => {
                "Symmetrical cylinder. Center bulges into a lens, edges curve away. \
                 Washes out the front-and-center content itself, rather than relying on \
                 gaze position."
            }
            Effect::Gravity => {
                "Kinetic response. Center stays small and calm, edges go large/fast/blurred. \
                 Tends to jitter at low scroll speed — the center is asked to be both large \
                 and slow, which fights itself."
            }
            Effect::Comfort => {
                "Washes out large, fast-moving foreground text — the side-window blur, applied \
                 to whatever's currently large and fast rather than only the screen edges."
            }
            Effect::Altitude => {
                "No speed cap, no distortion field. Faster scrolling climbs higher — text shrinks \
                 the way the ground does from a plane — so a fast flick covers more distance \
                 without fighting your hand, and landing is legible because you've zoomed back \
                 in by the time you arrive."
            }
        }
    }
}

/// One effect's slider, if it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntensitySpec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    /// Digits after the decimal point when showing the value.
    pub decimals: usize,
    /// Appended to the shown value.
    pub suffix: &'static str,
}

/// The slider for `effect`, or `None` if it has no intensity control.
pub const fn intensity_spec(effect: Effect) -> Option<IntensitySpec> {
    match effect {
        // Fading is driven entirely by how large and fast something already is.
        Effect::Comfort => None,
        Effect::Altitude => Some(IntensitySpec {
            label: "Climb sensitivity",
            min: 4.0,
            max: 40.0,
            step: 1.0,
            decimals: 0,
            suffix: "",
        }),
        _ => Some(IntensitySpec {
            label: "Intensity",
            min: 0.1,
            max: 3.0,
            step: 0.1,
            decimals: 1,
            suffix: "x",
        }),
    }
}

/// Which effects are armed, and how strongly.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    enabled: [bool; Effect::ALL.len()],
    parallax_intensity: f32,
    barrel_intensity: f32,
    gravity_intensity: f32,
    /// Velocity (px/frame) at which Altitude reaches full climb. Not a cap:
    /// scrolling faster than this just means you are already at altitude.
    climb_velocity: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let mut settings = Self {
            enabled: [false; Effect::ALL.len()],
            parallax_intensity: 1.0,
            barrel_intensity: 1.0,
            gravity_intensity: 1.0,
            climb_velocity: 14.0,
        };
        settings.set(Effect::Parallax, true);
        settings.set(Effect::Comfort, true);
        settings
    }
}

impl Settings {
    pub fn is_on(&self, effect: Effect) -> bool {
        self.enabled[effect.index()]
    }

    /// Arm or disarm an effect.
    ///
    /// Altitude is a different philosophy from the distortion fields: it
    /// removes the condition that requires distortion, rather than distorting.
    /// Running the two together muddies what you are feeling, so switching one
    /// on switches the other off.
    pub fn set(&mut self, effect: Effect, on: bool) {
        self.enabled[effect.index()] = on;
        if !on {
            return;
        }

        if effect == Effect::Altitude {
            for other in Effect::ALL {
                if other != Effect::Altitude {
                    self.enabled[other.index()] = false;
                }
            }
        } else {
            self.enabled[Effect::Altitude.index()] = false;
        }
    }

    pub fn intensity(&self, effect: Effect) -> f32 {
        match effect {
            Effect::Parallax => self.parallax_intensity,
            Effect::Barrel => self.barrel_intensity,
            Effect::Gravity => self.gravity_intensity,
            Effect::Altitude => self.climb_velocity,
            Effect::Comfort => 1.0,
        }
    }

    /// Set `effect`'s intensity, clamped to its slider's range.
    pub fn set_intensity(&mut self, effect: Effect, value: f32) {
        let Some(spec) = intensity_spec(effect) else {
            return;
        };
        let value = value.clamp(spec.min, spec.max);

        match effect {
            Effect::Parallax => self.parallax_intensity = value,
            Effect::Barrel => self.barrel_intensity = value,
            Effect::Gravity => self.gravity_intensity = value,
            Effect::Altitude => self.climb_velocity = value,
            Effect::Comfort => {}
        }
    }
}

/// How quickly a flick loses speed, per second — `gpui_animotion`'s own default
/// drag, kept so a flick here decays like a flick anywhere else in the crate.
const FLICK_FRICTION: f32 = 4.2;
/// The speed, in pixels per second, at which a flick counts as over. The
/// crate's default of 0.5 would leave the document creeping for seconds after
/// it had visibly stopped.
const FLICK_REST: f32 = 20.0;
/// How long a wheel tick takes to catch up with itself. A tween this shape
/// starts out at twice its average speed and eases out of it, which is close to
/// the prototype's fixed 10%-per-frame chase.
const WHEEL_SECS: f32 = 0.3;
const WHEEL_EASE: Ease = Ease::OutQuad;

/// The spring the pointer drags the document on: critically damped, so it
/// arrives without hunting, and stiff enough (ω ≈ 22 rad/s) that a drag reads
/// as direct manipulation rather than as slop.
///
/// The overshoot a drag does have comes from the speed
/// [`Prop::interrupt_spring`] carries over from the previous pointer move, not
/// from the spring itself, and it is what makes a drag one continuous motion
/// instead of a series of arrivals.
fn follow_spring() -> SpringParams {
    SpringParams {
        // The crate's default resting threshold is 0.05px, which spends a long
        // tail settling on detail nobody can see.
        threshold: 0.5,
        ..SpringParams::from_damping_ratio(500.0, 1.0)
    }
}

/// How far a flick at `velocity` travels before friction brings it under
/// [`FLICK_REST`] — where `gpui_animotion`'s own flick segment comes to rest.
fn flick_travel(velocity: f32) -> f32 {
    (velocity / FLICK_FRICTION) * (1.0 - (FLICK_REST / velocity.abs()).min(1.0))
}

/// The initial velocity of a flick that comes to rest after `distance` pixels.
///
/// The inverse of [`flick_travel`], so a flick can be aimed at a point rather
/// than merely thrown.
fn flick_velocity(distance: f32) -> f32 {
    FLICK_FRICTION * distance + FLICK_REST * distance.signum()
}

/// Whether a property has finished animating.
///
/// [`Prop`] has no such query, but its track does not need one: once a track has
/// run out of segments, or run past the last of them, `Track::sample` answers
/// with that segment's end value rather than resampling it, so "is the property
/// at its endpoint" is an exact test and not a tolerance. It is also the
/// cheapest way to know whether the window has anything left to draw.
trait Settled {
    fn settled(&self) -> bool;
}

impl Settled for Prop<f32> {
    fn settled(&self) -> bool {
        let track = self.track.lock().expect("animotion track poisoned");
        self.get() == track.current_end_value()
    }
}

/// Where the document is, how fast it is moving, and how far each eased
/// quantity has travelled towards its target.
#[derive(Clone)]
pub struct Motion {
    /// Where the document is, and what is moving it. Every input interrupts this
    /// rather than feeding a simulation of its own: see [`Motion::wheel`],
    /// [`Motion::drag`], [`Motion::release`] and [`Motion::grab`].
    ///
    /// Its own value is whatever the animation says, which a spring can carry a
    /// little past either end of the document; [`Motion::offset`] is the clamped
    /// reading, and is what everything else uses.
    offset: Prop<f32>,
    /// Animation time, advanced a whole number of frames at a time. Segments are
    /// sampled on this clock rather than on the wall clock, because a window
    /// that has been idle for a minute would otherwise hand the next animation a
    /// minute of elapsed time and have it land instantly.
    clock: Duration,
    /// The position the last frame drew, so that velocity can be measured from
    /// one frame to the next.
    previous: f32,
    /// Where the input has asked the document to go. Only the inputs read this;
    /// where the document actually is comes from `offset`.
    pub target_scroll: f32,
    /// Pixels per 60fps frame, signed: positive is scrolling down.
    pub velocity: f32,
    /// 0 when at rest, 1 once the effects are fully engaged.
    pub effect_blend: f32,
    /// 0 on the ground, 1 at full altitude.
    pub altitude_blend: f32,
    /// -1 or 1: which way the document is currently tilting.
    pub tilt_direction: f32,
    /// The document's length, in unscaled pixels. Unknown until the document has
    /// been built, which needs the window's text system.
    content_height: f32,
}

impl Motion {
    pub fn new() -> Self {
        Self {
            offset: Prop::new(0.0),
            clock: Duration::ZERO,
            previous: 0.0,
            target_scroll: 0.0,
            velocity: 0.0,
            effect_blend: 0.0,
            altitude_blend: 0.0,
            tilt_direction: 1.0,
            content_height: 0.0,
        }
    }

    /// Where the document is right now, in unscaled document pixels.
    ///
    /// Clamped to the document's ends. Nothing aims a wheel tick or a flick
    /// past them, so the clamp only ever does something while a spring is still
    /// carrying momentum into an end: a few pixels, for a few frames.
    pub fn offset(&self) -> f32 {
        self.offset.get().clamp(0.0, self.content_height)
    }

    /// Set the document's length, and start halfway down it so there is
    /// somewhere to scroll in either direction.
    pub fn set_content(&mut self, content_height: f32) {
        self.content_height = content_height;
        let middle = content_height / 2.0;
        self.offset = Prop::new(middle);
        // A property starts with no elapsed time of its own, so the first
        // interrupt would sample it an aeon into its animation and land it on
        // the spot. Priming it on the engine's clock keeps the two in step.
        self.offset.update(self.clock);
        self.target_scroll = middle;
        self.previous = middle;
    }

    /// A wheel tick: `delta` pixels of document travel, positive scrolling
    /// down.
    ///
    /// A tick is a deliberate step, so it takes the document over rather than
    /// blending with it — a tween, which abandons whatever the document was
    /// already doing, a flick in flight included.
    pub fn wheel(&mut self, delta: f32) {
        self.target_scroll = self.aim(self.target_scroll + delta);
        self.offset
            .interrupt_tween(self.target_scroll, WHEEL_SECS, WHEEL_EASE);
    }

    /// Take hold of the document. Whatever was moving it is called off and it
    /// stops where it is, so that a drag continues from here rather than from
    /// wherever a flick was going to leave it.
    pub fn grab(&mut self) {
        self.offset.stop();
        self.target_scroll = self.offset();
        self.velocity = 0.0;
    }

    /// Move the document with the pointer: one of these per pointer move.
    ///
    /// Each one re-aims the spring from wherever the document has got to,
    /// carrying the speed it already had, so that a drag is one continuous
    /// motion and not a series of arrivals.
    pub fn drag(&mut self, delta: f32) {
        self.target_scroll = self.aim(self.target_scroll + delta);
        self.offset
            .interrupt_spring(self.target_scroll, follow_spring());
    }

    /// Let go of the document at `velocity`, in pixels per second, and let it
    /// coast.
    ///
    /// The flick is aimed at where friction would carry it, but no further than
    /// the document's ends: a flick into an end comes to rest exactly on it
    /// rather than running past it, which keeps the document's own coordinates
    /// meaningful and means that scrolling back out of an end answers the very
    /// next input instead of travelling back from somewhere off the page.
    pub fn release(&mut self, velocity: f32) {
        // Aimed from the animation's own value rather than the clamped read: if
        // a spring has carried the document past an end, the flick that follows
        // should bring it back rather than carry it further out.
        let position = self.offset.get();
        let rest = self.aim(position + flick_travel(velocity));
        let initial_velocity = flick_velocity(rest - position);

        // An initial velocity of zero means something else to
        // `interrupt_flick`: not "stop" but "carry on at whatever speed you
        // had". A flick at or under its own threshold settles immediately
        // instead, and the two agree on when that is.
        if initial_velocity.abs() <= FLICK_REST {
            self.offset.stop();
        } else {
            self.offset.interrupt_flick(FlickParams {
                initial_velocity,
                friction: FLICK_FRICTION,
                threshold: FLICK_REST,
                duration: None,
            });
        }
        self.target_scroll = rest;
    }

    /// Hold `value` inside the document.
    fn aim(&self, value: f32) -> f32 {
        value.clamp(0.0, self.content_height)
    }

    pub fn abs_velocity(&self) -> f32 {
        self.velocity.abs()
    }

    /// Whether the document is moving fast enough to count as scrolling.
    pub fn is_scrolling(&self) -> bool {
        self.abs_velocity() > 0.5
    }

    /// Whether anything is still visibly moving, i.e. whether the window should
    /// ask for another frame.
    pub fn needs_frames(&self) -> bool {
        !self.offset.settled()
            || self.effect_blend > 0.002
            || self.altitude_blend > 0.002
            || (self.tilt_direction.abs() - 1.0).abs() > 0.002
    }

    /// Advance the simulation by `frames` (elapsed time in 60fps frames).
    pub fn advance(&mut self, frames: f32, settings: &Settings) {
        self.clock += Duration::from_secs_f32(frames / FRAMES_PER_SECOND);
        self.offset.update(self.clock);

        // Velocity is measured from where the document actually is, rather than
        // read back out of whatever is animating it. That is the speed the
        // effects are about — how fast the text is crossing the screen — and it
        // stays honest whether the document is being tweened, sprung, flung, or
        // simply held still.
        let position = self.offset();
        self.velocity = if frames > 0.0 {
            (position - self.previous) / frames
        } else {
            0.0
        };
        self.previous = position;

        // Once nothing is animating, the input's target is wherever the document
        // came to rest: a flick has no target of its own, and one cut short by
        // the end of the document stops short of the one it was given.
        if self.offset.settled() {
            self.target_scroll = position;
        }

        let abs_velocity = self.abs_velocity();
        let is_scrolling = self.is_scrolling();

        // Ease the effects in quickly and out slowly, so a flick engages at
        // once and a stop does not snap the document back.
        let blend_target = if is_scrolling { 1.0 } else { 0.0 };
        let blend_rate = if is_scrolling { 0.1 } else { 0.03 };
        self.effect_blend += (blend_target - self.effect_blend) * approach(blend_rate, frames);

        // Altitude climbs with how fast you are actually moving right now, not
        // with a scrolling on/off flag: a slow drift stays low, a hard flick
        // climbs high. It rises quickly (get out of the way fast) and descends
        // more slowly (landing should feel like arriving, not snapping back).
        let altitude_target = if settings.is_on(Effect::Altitude) {
            (abs_velocity / settings.intensity(Effect::Altitude).max(0.001)).min(1.0)
        } else {
            0.0
        };
        let altitude_rate = if altitude_target > self.altitude_blend {
            0.18
        } else {
            0.045
        };
        self.altitude_blend +=
            (altitude_target - self.altitude_blend) * approach(altitude_rate, frames);

        // The tilt follows the direction of travel, but eases across the flip
        // rather than snapping. When the document comes to rest it keeps
        // settling towards the direction it was last travelling in.
        let tilt_target = if is_scrolling {
            self.velocity.signum()
        } else {
            self.tilt_direction.signum()
        };
        self.tilt_direction += (tilt_target - self.tilt_direction) * approach(0.08, frames);
    }

    /// How much to scale a line sitting `rel_y` pixels from the anchor (the
    /// line at the centre of the viewport).
    ///
    /// This is the whole perceptual transform: every effect is a multiplier on
    /// the same scale. Distances stay in unscaled document coordinates, so the
    /// scale itself changes how fast the document moves past you — which is
    /// exactly the thing being tuned.
    pub fn scale_at(&self, rel_y: f32, settings: &Settings) -> f32 {
        let abs_velocity = self.abs_velocity();
        let blend = self.effect_blend;
        let mut scale = 1.0;

        // Altitude: no distortion field at all, just a uniform shrink, so a
        // fast flick covers more ground without fighting your hand. While the
        // mode is still easing out, whatever is left of the blend is applied
        // to the rest of the effects below instead.
        if self.altitude_blend > 0.001 {
            let altitude_scale = 1.0 - (0.82 * self.altitude_blend);
            if settings.is_on(Effect::Altitude) {
                return altitude_scale;
            }
            scale *= altitude_scale;
        }

        if settings.is_on(Effect::Parallax) {
            let tilt_strength = 0.001 * blend * settings.intensity(Effect::Parallax);
            scale *= (rel_y * tilt_strength * self.tilt_direction).exp();
        }

        if settings.is_on(Effect::Barrel) {
            let intensity = settings.intensity(Effect::Barrel);
            let bulge = 1.0 + (0.5 * blend * intensity * gaussian(rel_y / 400.0));
            let edge_shrink = (-rel_y.abs() * 0.0012 * blend * intensity).exp();
            scale *= bulge * edge_shrink;
        }

        if settings.is_on(Effect::Gravity) {
            let intensity = settings.intensity(Effect::Gravity);
            let center_proximity = gaussian(rel_y / 500.0);
            let pinch = (abs_velocity * 0.015).min(0.9) * blend * intensity;
            let stretch = (abs_velocity * 0.005).min(2.5) * blend * intensity;
            scale *= (1.0 - pinch * center_proximity) * (1.0 + stretch * (1.0 - center_proximity));
        }

        scale.clamp(0.01, 8.0)
    }
}

/// `exp(-x²)`.
fn gaussian(x: f32) -> f32 {
    (-(x * x)).exp()
}

/// Fraction of the way to move this frame, for a rate that moves `rate` of the
/// way there in a single 60fps frame.
///
/// Without this, the same physics would run twice as fast on a 120Hz display.
fn approach(rate: f32, frames: f32) -> f32 {
    1.0 - (1.0 - rate).powf(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document long enough to scroll around in: 2000 lines, as the prototype
    /// had.
    fn motion() -> Motion {
        let mut motion = Motion::new();
        motion.set_content(2000.0 * crate::document::BASE_SPACING);
        motion
    }

    /// Run `count` frames of the simulation.
    fn run(motion: &mut Motion, settings: &Settings, count: usize) {
        for _ in 0..count {
            motion.advance(1.0, settings);
        }
    }

    #[test]
    fn altitude_climbs_with_speed_and_falls_slowly() {
        let mut motion = motion();
        let mut settings = Settings::default();
        settings.set(Effect::Altitude, true);

        // A hard flick: 3600px/s is 60px a frame, well past the altitude
        // threshold, so the climb should reach the top while it is still moving.
        let start = motion.offset();
        motion.release(3600.0);
        let mut climbed: f32 = 0.0;
        for _ in 0..30 {
            motion.advance(1.0, &settings);
            climbed = climbed.max(motion.altitude_blend);
        }
        assert!(climbed > 0.5, "a hard flick should get airborne: {climbed}");
        assert!(motion.is_scrolling(), "the flick should still be coasting");
        assert!(motion.offset() > start, "and should have travelled on");

        // Letting go lands slowly: a single frame of descent should barely
        // dent the altitude.
        motion.grab();
        let landed = motion.altitude_blend;
        motion.advance(1.0, &settings);
        assert!(
            motion.altitude_blend > landed * 0.9,
            "altitude should descend more slowly than it climbs"
        );
    }

    #[test]
    fn altitude_and_distortion_are_mutually_exclusive() {
        let mut settings = Settings::default();
        assert!(settings.is_on(Effect::Parallax));

        settings.set(Effect::Altitude, true);
        assert!(settings.is_on(Effect::Altitude));
        assert!(!settings.is_on(Effect::Parallax));
        assert!(!settings.is_on(Effect::Comfort));

        settings.set(Effect::Barrel, true);
        assert!(settings.is_on(Effect::Barrel));
        assert!(!settings.is_on(Effect::Altitude));
    }

    #[test]
    fn at_rest_nothing_is_distorted() {
        let motion = motion();
        let settings = Settings::default();

        assert!(!motion.needs_frames());
        for rel_y in [-800.0, -100.0, 0.0, 100.0, 800.0] {
            assert_eq!(motion.scale_at(rel_y, &settings), 1.0);
        }
    }

    #[test]
    fn a_wheel_tick_settles_on_the_target() {
        let mut motion = motion();
        let settings = Settings::default();
        let target = motion.target_scroll + 900.0;

        motion.wheel(900.0);
        // A few seconds of frames: long enough for the target to be reached
        // and for the effect blend to fade back out afterwards.
        run(&mut motion, &settings, 500);

        assert!((motion.offset() - target).abs() < 0.01);
        assert!(!motion.needs_frames());
    }

    #[test]
    fn a_flick_coasts_to_rest_where_friction_predicts() {
        let mut motion = motion();
        let settings = Settings::default();

        let start = motion.offset();
        motion.release(1200.0);
        assert!(motion.needs_frames(), "a flick should ask for frames");

        run(&mut motion, &settings, 600);
        assert!(!motion.needs_frames(), "a flick should come to rest");
        let travelled = motion.offset() - start;
        assert!(
            (travelled - flick_travel(1200.0)).abs() < 2.0,
            "the flick should land where its decay says: {travelled}"
        );

        // And the same throw the other way travels the other way.
        let start = motion.offset();
        motion.release(-1200.0);
        run(&mut motion, &settings, 600);
        let travelled = motion.offset() - start;
        assert!(
            (travelled + flick_travel(1200.0)).abs() < 2.0,
            "a throw the other way should travel back: {travelled}"
        );
    }

    #[test]
    fn a_flick_into_the_end_of_the_document_stops_on_it() {
        let mut motion = motion();
        let settings = Settings::default();

        // Scroll to the very end of the document, then back off it a little, so
        // there is room to throw the document further than it can go.
        motion.wheel(1.0e9);
        run(&mut motion, &settings, 60);
        let end = motion.offset();
        assert!(
            (end - motion.content_height).abs() < 0.01,
            "the wheel should have taken the document to its end: {end}"
        );

        motion.wheel(-2000.0);
        run(&mut motion, &settings, 60);
        let back = motion.offset();
        assert!(flick_travel(9000.0) > 2000.0, "the throw should overshoot");

        motion.release(9000.0);
        for _ in 0..600 {
            motion.advance(1.0, &settings);
            assert!(
                motion.offset() <= motion.content_height,
                "the document should not be drawn past its end"
            );
        }

        assert!(!motion.needs_frames(), "the flick should have settled");
        assert!(
            (motion.offset() - end).abs() < 1.0,
            "a flick into an end should come to rest on it: {back} -> {}",
            motion.offset()
        );
    }

    #[test]
    fn a_wheel_tick_takes_the_document_over_from_a_flick() {
        let mut motion = motion();
        let settings = Settings::default();

        // Throw it, let it get up to speed, then turn it around mid-flight.
        motion.release(2400.0);
        run(&mut motion, &settings, 10);
        let thrown = motion.offset();

        motion.wheel(-400.0);
        run(&mut motion, &settings, 600);

        assert!(!motion.needs_frames(), "the tick should have settled");
        assert!(
            (motion.offset() - motion.target_scroll).abs() < 0.01,
            "the document should be where the wheel asked for it"
        );
        assert!(
            motion.offset() < thrown,
            "a tick back should reverse a flick in flight: {thrown} -> {}",
            motion.offset()
        );
    }
}
