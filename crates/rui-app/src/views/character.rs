//! Rui, the animated robot character from the design mock's `rui()` SVG.
//!
//! Each part is its own one-color SVG layer (`assets/rui/*.svg`, 56x56 view box),
//! stacked in a relative div. Every frame computes a pose per layer from the
//! mock's keyframes and paints it with `svg().with_transformation(..)`.

use super::motion::{ease_in_out, ease_out, spring};
use crate::api::LiveState;
use crate::theme::Theme;
use gpui::{
    Animation, AnimationExt, AnyElement, Div, ElementId, Hsla, IntoElement, Pixels, Styled,
    Transformation, div, hsla, point, prelude::*, px, radians, size, svg,
};
use rui_core::TaskStatus;
use std::time::Duration;

/// Every layer file under `assets/rui/`.
#[cfg(test)]
pub const LAYERS: [&str; 17] = [
    "shadow", "stem", "bulb", "shell", "visor", "eyes", "eyes-wide", "eyes-happy", "mouth-flat",
    "mouth-smile", "mouth-o", "mouth-wave", "band", "think1", "think2", "spark", "ping",
];

/// What the character is doing; each state has its own motion.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CharState {
    Thinking,
    Doing,
    Standing,
    Blocked,
    Landed,
}

/// State for a thread: owner-blocked and done win, then live activity, then status.
pub fn char_state(status: &TaskStatus, live: Option<&LiveState>) -> CharState {
    match (status, live) {
        (TaskStatus::WaitingAnswer | TaskStatus::Stuck, _) => CharState::Blocked,
        (TaskStatus::Done, _) => CharState::Landed,
        (_, Some(l)) if l.thinking => CharState::Thinking,
        (_, Some(_)) => CharState::Doing,
        (TaskStatus::Clarifying, None) => CharState::Thinking,
        (TaskStatus::InProgress, None) => CharState::Doing,
        _ => CharState::Standing,
    }
}

impl CharState {
    fn name(self) -> &'static str {
        match self {
            CharState::Thinking => "thinking",
            CharState::Doing => "doing",
            CharState::Standing => "standing",
            CharState::Blocked => "blocked",
            CharState::Landed => "landed",
        }
    }

    fn bulb(self, th: &Theme) -> Hsla {
        match self {
            CharState::Thinking => th.think.base,
            CharState::Doing => th.run.base,
            CharState::Standing => th.wait.base,
            CharState::Blocked => th.block.base,
            CharState::Landed => th.done.base,
        }
    }
}

/// 2D affine map in the 56-unit view box: (x, y) -> (a x + c y + tx, b x + d y + ty).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    tx: f32,
    ty: f32,
}

impl Affine {
    const ID: Affine = Affine { a: 1., b: 0., c: 0., d: 1., tx: 0., ty: 0. };

    /// CSS `translate(dx,dy) rotate(deg) scale(sx,sy)` around origin (ox, oy).
    fn css(o: (f32, f32), d: (f32, f32), deg: f32, s: (f32, f32)) -> Affine {
        let (sin, cos) = deg.to_radians().sin_cos();
        let (a, b, c, dd) = (cos * s.0, sin * s.0, -sin * s.1, cos * s.1);
        let tx = o.0 + d.0 - (a * o.0 + c * o.1);
        let ty = o.1 + d.1 - (b * o.0 + dd * o.1);
        Affine { a, b, c, d: dd, tx, ty }
    }

    fn translate(dx: f32, dy: f32) -> Affine {
        Affine { tx: dx, ty: dy, ..Affine::ID }
    }

    fn scale_at(o: (f32, f32), sx: f32, sy: f32) -> Affine {
        Affine::css(o, (0., 0.), 0., (sx, sy))
    }

    /// `self` applied after `inner`.
    fn then(self, inner: Affine) -> Affine {
        let m = self;
        Affine {
            a: m.a * inner.a + m.c * inner.b,
            b: m.b * inner.a + m.d * inner.b,
            c: m.a * inner.c + m.c * inner.d,
            d: m.b * inner.c + m.d * inner.d,
            tx: m.a * inner.tx + m.c * inner.ty + m.tx,
            ty: m.b * inner.tx + m.d * inner.ty + m.ty,
        }
    }

    /// (angle rad, sx, sy, tx, ty) for gpui's center-based rotate*scale + translate.
    fn decompose(self) -> (f32, f32, f32, f32, f32) {
        let angle = self.b.atan2(self.a);
        let (sin, cos) = angle.sin_cos();
        let sx = self.a.hypot(self.b);
        let sy = -self.c * sin + self.d * cos;
        let tx = self.tx + (self.a + self.c - 1.) * 28.;
        let ty = self.ty + (self.b + self.d - 1.) * 28.;
        (angle, sx, sy, tx, ty)
    }

    fn to_transformation(self, unit: f32) -> Transformation {
        let (angle, sx, sy, tx, ty) = self.decompose();
        Transformation::rotate(radians(angle))
            .with_scaling(size(sx, sy))
            .with_translation(point(px(tx * unit), px(ty * unit)))
    }
}

/// Value of CSS keyframes at progress `p`, easing each segment with `f`.
fn kf(p: f32, keys: &[(f32, f32)], f: fn(f32) -> f32) -> f32 {
    let p = p.clamp(0., 1.);
    let Some(i) = keys.windows(2).position(|w| p <= w[1].0) else { return keys[keys.len() - 1].1 };
    let ((t0, v0), (t1, v1)) = (keys[i], keys[i + 1]);
    let local = if t1 > t0 { (p - t0) / (t1 - t0) } else { 1. };
    v0 + (v1 - v0) * f(local)
}

/// Progress through a repeating cycle of `period` seconds, started `delay` seconds in.
fn cycle(t: f32, period: f32, delay: f32) -> Option<f32> {
    let t = t - delay;
    (t >= 0.).then(|| (t % period) / period)
}

/// Transforms and opacities of the animated parts for one frame.
struct Pose {
    all: Affine,
    body: Affine,
    ant: Affine,
    eyes: Affine,
    bulb: (Affine, f32),
    think: [(Affine, f32); 2],
    spark: (Affine, f32),
    ping: (Affine, f32),
    mouth: f32,
}

impl Pose {
    fn rest() -> Pose {
        let hidden = (Affine::ID, 0.);
        Pose {
            all: Affine::ID,
            body: Affine::ID,
            ant: Affine::ID,
            eyes: Affine::ID,
            bulb: (Affine::ID, 1.),
            think: [hidden, hidden],
            spark: hidden,
            ping: hidden,
            mouth: 1.,
        }
    }
}

const ALL_O: (f32, f32) = (28., 42.);
const BODY_O: (f32, f32) = (28., 44.);
const ANT_O: (f32, f32) = (28., 16.);
const EYES_O: (f32, f32) = (28., 27.);
const BULB_O: (f32, f32) = (28., 7.);

fn pose(state: CharState, t: f32) -> Pose {
    match state {
        CharState::Thinking => thinking(t),
        CharState::Doing => doing(t),
        CharState::Standing => standing(t),
        CharState::Blocked => blocked(t),
        CharState::Landed => landed(t),
    }
}

/// One floating thought dot: rises up-right and fades, 2.1 s.
fn float_dot(t: f32, delay: f32, o: (f32, f32)) -> (Affine, f32) {
    let Some(p) = cycle(t, 2.1, delay) else { return (Affine::ID, 0.) };
    let alpha = kf(p, &[(0., 0.), (0.35, 0.95), (1., 0.)], ease_in_out);
    let e = ease_in_out(p);
    let s = 0.6 + 0.4 * e;
    (Affine::css(o, (5. * e, 4. - 13. * e), 0., (s, s)), alpha)
}

fn thinking(t: f32) -> Pose {
    let rock = cycle(t, 2.9, 0.).unwrap_or(0.);
    let wave = |a: f32, b: f32| kf(rock, &[(0., a), (0.5, b), (1., a)], ease_in_out);
    let pulse = cycle(t, 1.7, 0.).unwrap_or(0.);
    let s = kf(pulse, &[(0., 0.84), (0.5, 1.12), (1., 0.84)], ease_in_out);
    let o = kf(pulse, &[(0., 0.45), (0.5, 1.), (1., 0.45)], ease_in_out);
    Pose {
        body: Affine::css(BODY_O, (0., 0.), wave(-4.5, 4.5), (1., 1.)),
        eyes: Affine::translate(wave(-2., 2.), wave(-1.6, -2.4)),
        bulb: (Affine::scale_at(BULB_O, s, s), o),
        think: [float_dot(t, 0., (44., 13.)), float_dot(t, 0.34, (48.5, 8.5))],
        ..Pose::rest()
    }
}

fn doing(t: f32) -> Pose {
    let p = cycle(t, 0.66, 0.).unwrap_or(0.);
    let y = kf(p, &[(0., 0.), (0.4, -2.2), (0.7, 0.8), (1., 0.)], ease_in_out);
    let r = kf(p, &[(0., 0.), (0.4, 2.4), (0.7, -1.2), (1., 0.)], ease_in_out);
    let blip = kf(p, &[(0., 1.), (0.55, 0.3), (1., 1.)], ease_in_out);
    let sp = cycle(t, 0.9, 0.).unwrap_or(0.);
    let e = ease_out(sp);
    let spark_alpha = kf(sp, &[(0., 0.), (0.3, 1.), (1., 0.)], ease_out);
    Pose {
        all: Affine::css(ALL_O, (0., y), r, (1., 1.)),
        eyes: Affine::scale_at(EYES_O, 1., 0.44),
        bulb: (Affine::ID, blip),
        spark: (Affine::translate(-3. + 7. * e, 3. - 7. * e), spark_alpha),
        ..Pose::rest()
    }
}

fn standing(t: f32) -> Pose {
    let b = cycle(t, 3.8, 0.).unwrap_or(0.);
    let s = kf(b, &[(0., 1.), (0.5, 1.038), (1., 1.)], ease_in_out);
    let blink = cycle(t, 4.6, 0.).unwrap_or(0.);
    let sy = kf(blink, &[(0., 1.), (0.92, 1.), (0.96, 0.1), (1., 1.)], ease_in_out);
    Pose {
        all: Affine::scale_at(ALL_O, s, s),
        ant: Affine::css(ANT_O, (0., 0.), 17., (1., 1.)),
        eyes: Affine::scale_at(EYES_O, 1., sy),
        mouth: 0.8,
        ..Pose::rest()
    }
}

fn blocked(t: f32) -> Pose {
    let n = cycle(t, 1.5, 0.).unwrap_or(0.);
    let y = kf(n, &[(0., 0.), (0.28, -3.4), (0.52, 0.), (1., 0.)], ease_in_out);
    let alert = if t % 1. < 0.5 { 1. } else { 0.25 };
    let pg = cycle(t, 1.9, 0.).unwrap_or(0.);
    let e = ease_out(pg);
    let s = 0.62 + 0.5 * e;
    Pose {
        all: Affine::translate(0., y),
        bulb: (Affine::ID, alert),
        ping: (Affine::scale_at((28., 30.), s, s), 0.5 * (1. - e)),
        ..Pose::rest()
    }
}

fn landed(t: f32) -> Pose {
    let p = t / 0.68;
    let y = kf(p, &[(0., 0.), (0.35, -9.), (0.7, 0.), (1., 0.)], spring);
    let sx = kf(p, &[(0., 1.), (0.35, 1.06), (0.7, 0.96), (1., 1.)], spring);
    let sy = kf(p, &[(0., 1.), (0.35, 0.95), (0.7, 1.05), (1., 1.)], spring);
    Pose { all: Affine::css(ALL_O, (0., y), 0., (sx, sy)), ..Pose::rest() }
}

/// Colors of the body parts, from the client's color and the theme.
#[derive(Clone, Copy)]
struct Palette {
    shell: Hsla,
    visor: Hsla,
    eye: Hsla,
    band: Hsla,
    bulb: Hsla,
    shadow: Hsla,
}

fn palette(state: CharState, client: Hsla, th: &Theme) -> Palette {
    let h = client.h;
    let (shell, visor, eye) = if th.dark {
        (hsla(h, 0.2, 0.33, 1.), hsla(h, 0.22, 0.16, 1.), hsla(h, 0.25, 0.94, 1.))
    } else {
        (hsla(h, 0.24, 0.66, 1.), hsla(h, 0.2, 0.38, 1.), hsla(h, 0.3, 0.985, 1.))
    };
    let shadow = if th.dark { hsla(0., 0., 0., 0.22) } else { hsla(h, 0.2, 0.25, 0.13) };
    Palette { shell, visor, eye, band: client, bulb: state.bulb(th), shadow }
}

fn layer(name: &str, unit: f32, color: Hsla, alpha: f32, m: Affine) -> Option<AnyElement> {
    if alpha <= 0.01 {
        return None;
    }
    let el = svg()
        .path(format!("rui/{name}.svg"))
        .absolute()
        .top_0()
        .left_0()
        .size(px(56. * unit))
        .text_color(color.opacity(alpha))
        .with_transformation(m.to_transformation(unit));
    Some(el.into_any_element())
}

/// Eye and mouth layer names for a state.
fn face(state: CharState) -> (&'static str, &'static str) {
    match state {
        CharState::Thinking => ("eyes", "mouth-flat"),
        CharState::Doing => ("eyes", "mouth-o"),
        CharState::Standing => ("eyes", "mouth-flat"),
        CharState::Blocked => ("eyes-wide", "mouth-wave"),
        CharState::Landed => ("eyes-happy", "mouth-smile"),
    }
}

fn frame(state: CharState, t: f32, unit: f32, c: Palette) -> Div {
    let p = pose(state, t);
    let (eyes, mouth) = face(state);
    let (all, body, ant) = (p.all, p.all.then(p.body), p.all.then(p.ant));
    let layers = [
        layer("shadow", unit, c.shadow, 1., Affine::ID),
        layer("stem", unit, c.shell, 1., ant),
        layer("bulb", unit, c.bulb, p.bulb.1, ant.then(p.bulb.0)),
        layer("shell", unit, c.shell, 1., body),
        layer("visor", unit, c.visor, 1., body),
        layer(eyes, unit, c.eye, 1., body.then(p.eyes)),
        layer(mouth, unit, c.eye, 0.72 * p.mouth, body),
        layer("band", unit, c.band, 1., body),
        layer("think1", unit, c.bulb, p.think[0].1, all.then(p.think[0].0)),
        layer("think2", unit, c.bulb, p.think[1].1, all.then(p.think[1].0)),
        layer("spark", unit, c.bulb, p.spark.1, all.then(p.spark.0)),
        layer("ping", unit, c.bulb, p.ping.1, all.then(p.ping.0)),
    ];
    div().relative().flex_none().size(px(56. * unit)).children(layers.into_iter().flatten())
}

/// Seconds in one repeat of the looping states; long so the wrap is rarely seen.
const LOOP_SECS: f32 = 3600.;

/// The animated character, `size` square, for a client's color.
///
/// The animation id includes the state, so a state change restarts the motion
/// (and the landed hop plays once).
pub fn character(state: CharState, size: Pixels, id: impl Into<ElementId>, client: Hsla, th: &Theme) -> AnyElement {
    let unit = f32::from(size) / 56.;
    let colors = palette(state, client, th);
    let id = ElementId::NamedChild(Box::new(id.into()), state.name().into());
    let (anim, secs) = match state {
        CharState::Landed => (Animation::new(Duration::from_millis(680)), 0.68),
        _ => (Animation::new(Duration::from_secs_f32(LOOP_SECS)).repeat(), LOOP_SECS),
    };
    div()
        .with_animation(id, anim, move |_, d| frame(state, d * secs, unit, colors))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(thinking: bool, text: &str) -> LiveState {
        LiveState { text: text.into(), thinking, ..Default::default() }
    }

    #[test]
    fn state_mapping() {
        use CharState::*;
        assert_eq!(char_state(&TaskStatus::InProgress, Some(&live(false, "hi"))), Doing);
        assert_eq!(char_state(&TaskStatus::InProgress, Some(&live(true, ""))), Thinking);
        assert_eq!(char_state(&TaskStatus::WaitingAnswer, Some(&live(false, "x"))), Blocked);
        assert_eq!(char_state(&TaskStatus::Stuck, None), Blocked);
        assert_eq!(char_state(&TaskStatus::Done, None), Landed);
        assert_eq!(char_state(&TaskStatus::Open, None), Standing);
        assert_eq!(char_state(&TaskStatus::New, None), Standing);
        assert_eq!(char_state(&TaskStatus::Clarifying, None), Thinking);
        assert_eq!(char_state(&TaskStatus::InProgress, None), Doing);
    }

    fn apply(m: Affine, x: f32, y: f32) -> (f32, f32) {
        (m.a * x + m.c * y + m.tx, m.b * x + m.d * y + m.ty)
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn css_transform_keeps_origin() {
        let m = Affine::css((28., 44.), (0., 0.), 30., (1.2, 0.8));
        assert!(near(apply(m, 28., 44.), (28., 44.)));
    }

    #[test]
    fn decompose_round_trips_through_center() {
        let m = Affine::css((28., 42.), (1., -2.), 12., (1.1, 0.9));
        let (angle, sx, sy, tx, ty) = m.decompose();
        let (sin, cos) = angle.sin_cos();
        // gpui maps x -> center + t + R*S*(x - center), center = (28, 28).
        let gpui = |x: f32, y: f32| {
            let (dx, dy) = ((x - 28.) * sx, (y - 28.) * sy);
            (28. + tx + cos * dx - sin * dy, 28. + ty + sin * dx + cos * dy)
        };
        for (x, y) in [(0., 0.), (56., 10.), (28., 42.)] {
            assert!(near(gpui(x, y), apply(m, x, y)));
        }
    }

    #[test]
    fn keyframes_hit_their_values() {
        let keys = [(0., 0.), (0.4, -2.2), (0.7, 0.8), (1., 0.)];
        assert!(kf(0., &keys, ease_in_out).abs() < 1e-3);
        assert!((kf(0.4, &keys, ease_in_out) + 2.2).abs() < 1e-3);
        assert!((kf(0.7, &keys, ease_in_out) - 0.8).abs() < 1e-3);
    }

    #[test]
    fn cycle_respects_delay() {
        assert_eq!(cycle(0.2, 2.1, 0.34), None);
        assert!((cycle(0.34 + 1.05, 2.1, 0.34).unwrap() - 0.5).abs() < 1e-4);
    }
}
