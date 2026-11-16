//! Motion tokens and easing helpers, matching the design mock's CSS.
//!
//! gpui's `Animation` asserts its eased value stays in 0..=1, so animations
//! here run linear and the curve (which may overshoot) is applied in the animator.

use gpui::{Animation, AnimationExt, ElementId, IntoElement, Styled, px};
use std::time::Duration;

/// Row enter rise.
pub const RISE_MS: u64 = 380;
/// Delay between rows when a list first appears.
pub const STAGGER_MS: u64 = 34;
/// Most rows that get their own stagger slot.
const STAGGER_CAP: usize = 10;

/// Value of a CSS `cubic-bezier(x1, y1, x2, y2)` at time `t` (0..=1).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let curve = |a: f32, b: f32, s: f32| 3. * a * s * (1. - s).powi(2) + 3. * b * s * s * (1. - s) + s.powi(3);
    // Bisection on x(s) = t; x is monotonic for x1, x2 in 0..=1.
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if curve(x1, x2, mid) < t { lo = mid } else { hi = mid }
    }
    curve(y1, y2, (lo + hi) / 2.)
}

/// `--ease`: cubic-bezier(0.32, 0.72, 0, 1).
pub fn ease(t: f32) -> f32 {
    cubic_bezier(0.32, 0.72, 0., 1., t)
}

/// `--spring`: cubic-bezier(0.34, 1.56, 0.64, 1); overshoots past 1.
pub fn spring(t: f32) -> f32 {
    cubic_bezier(0.34, 1.56, 0.64, 1., t)
}

/// CSS `ease-in-out`.
pub fn ease_in_out(t: f32) -> f32 {
    cubic_bezier(0.42, 0., 0.58, 1., t)
}

/// CSS `ease-out`.
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0., 0., 0.58, 1., t)
}

/// Progress of an animation that starts `delay_ms` into a `total_ms` run and lasts `dur_ms`.
pub fn delayed(delta: f32, total_ms: u64, delay_ms: u64, dur_ms: u64) -> f32 {
    let now = delta * total_ms as f32;
    ((now - delay_ms as f32) / dur_ms as f32).clamp(0., 1.)
}

/// One-shot linear animation of `ms` milliseconds.
pub fn once(ms: u64) -> Animation {
    Animation::new(Duration::from_millis(ms))
}

/// Rise-and-fade on first appearance: 12px up, staggered by `index`.
///
/// gpui keeps animation state per element id while the element keeps rendering,
/// so the rise plays once when the row (or its view) first shows up.
pub fn rise<E>(el: E, id: impl Into<ElementId>, index: usize) -> impl IntoElement
where
    E: IntoElement + Styled + 'static,
{
    let delay = index.min(STAGGER_CAP) as u64 * STAGGER_MS;
    let total = delay + RISE_MS;
    el.with_animation(id, once(total), move |el, d| {
        let p = ease(delayed(d, total, delay, RISE_MS));
        el.relative().top(px(12. * (1. - p))).opacity(p)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn bezier_endpoints() {
        for f in [ease, spring, ease_in_out, ease_out] {
            assert!(close(f(0.), 0.));
            assert!(close(f(1.), 1.));
        }
    }

    #[test]
    fn linear_bezier_is_identity() {
        for t in [0.1, 0.25, 0.5, 0.9] {
            assert!(close(cubic_bezier(0.25, 0.25, 0.75, 0.75, t), t));
        }
    }

    #[test]
    fn ease_in_out_is_symmetric() {
        assert!(close(ease_in_out(0.5), 0.5));
        assert!(close(ease_in_out(0.2) + ease_in_out(0.8), 1.));
    }

    #[test]
    fn spring_overshoots_then_settles() {
        let peak = (1..100).map(|i| spring(i as f32 / 100.)).fold(0f32, f32::max);
        assert!(peak > 1.05, "peak {peak}");
        assert!(close(spring(1.), 1.));
    }

    #[test]
    fn ease_is_front_loaded() {
        assert!(ease(0.3) > 0.6);
    }

    #[test]
    fn delayed_progress() {
        assert_eq!(delayed(0.0, 400, 100, 300), 0.);
        assert_eq!(delayed(0.25, 400, 100, 300), 0.);
        assert!(close(delayed(0.625, 400, 100, 300), 0.5));
        assert_eq!(delayed(1.0, 400, 100, 300), 1.);
    }
}
