//! Color tokens for the light and dark appearances.

// Some tokens are consumed by views that are restyled later.

use gpui::{App, BoxShadow, Global, Hsla, WindowAppearance, point, px, rgb};

/// One status color family.
#[derive(Clone)]
pub struct Status {
    pub base: Hsla,
    pub fg: Hsla,
    pub dim: Hsla,
    pub rim: Hsla,
}

/// All color tokens used by the UI.
#[derive(Clone)]
pub struct Theme {
    pub stage: Hsla,
    pub surface: Hsla,
    pub surface_2: Hsla,
    pub well: Hsla,
    pub ctl: Hsla,
    pub fg: Hsla,
    pub fg_2: Hsla,
    pub muted: Hsla,
    pub hairline: Hsla,
    pub accent_text: Hsla,
    pub accent_fill: Hsla,
    pub accent_fill_hover: Hsla,
    pub accent_fill_active: Hsla,
    pub sel: Hsla,
    pub on_accent: Hsla,
    pub glass: Hsla,
    pub glass_rim: Hsla,
    pub badge_text: Hsla,
    pub run: Status,
    pub think: Status,
    pub wait: Status,
    pub done: Status,
    pub block: Status,
    pub dark: bool,
}

impl Global for Theme {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, a: f32) -> Hsla {
    c(hex).opacity(a)
}

fn status(base: u32, fg: u32, dim_a: f32, rim_a: f32) -> Status {
    Status { base: c(base), fg: c(fg), dim: ca(base, dim_a), rim: ca(base, rim_a) }
}

/// Theme for the given window appearance.
pub fn for_appearance(a: WindowAppearance) -> Theme {
    match a {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
        _ => Theme::light(),
    }
}

/// Shorthand for the global theme.
pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            stage: c(0x151b17),
            surface: c(0x1f2520),
            surface_2: c(0x282f2a),
            well: c(0x0f1511),
            ctl: c(0x333a34),
            fg: c(0xf1f7f2),
            fg_2: c(0xc2cac3),
            muted: c(0x97a19a),
            hairline: ca(0xffffff, 0.075),
            accent_text: c(0x67c986),
            accent_fill: c(0x0e6936),
            accent_fill_hover: c(0x005729),
            accent_fill_active: c(0x004820),
            sel: c(0x096331),
            on_accent: c(0xfdfdfd),
            glass: ca(0x3b5b43, 0.20),
            glass_rim: ca(0xffffff, 0.17),
            badge_text: c(0x150a09),
            run: status(0x3fb8be, 0x3fb8be, 0.20, 0.36),
            think: status(0xaaa8f1, 0xb3b1fc, 0.20, 0.36),
            wait: status(0xefb146, 0xefb146, 0.20, 0.36),
            done: status(0x869889, 0x98aa9b, 0.20, 0.36),
            block: status(0xff645d, 0xff645d, 0.20, 0.36),
            dark: true,
        }
    }

    pub fn light() -> Theme {
        Theme {
            stage: c(0xeef7f0),
            surface: c(0xffffff),
            surface_2: c(0xeef6f0),
            well: c(0xe6f1e9),
            ctl: c(0xffffff),
            fg: c(0x18211a),
            fg_2: c(0x3f4841),
            muted: c(0x58605a),
            hairline: ca(0x000000, 0.10),
            accent_text: c(0x0e6936),
            glass: ca(0xcbfbd6, 0.28),
            glass_rim: ca(0x1e3424, 0.14),
            run: status(0x00868c, 0x006167, 0.15, 0.30),
            think: status(0x6a63b5, 0x554da0, 0.15, 0.30),
            wait: status(0xd78e28, 0x804c00, 0.15, 0.30),
            done: status(0x6e8071, 0x445547, 0.15, 0.30),
            block: status(0xd22e30, 0xac0216, 0.15, 0.30),
            dark: false,
            ..Theme::dark()
        }
    }

    /// Accent color for a client, cycling through four hues.
    pub fn client_color(&self, client_id: i64) -> Hsla {
        let dark = [0x48a260, 0x00a2b1, 0x857fd7, 0xcf6963];
        let light = [0x258343, 0x008292, 0x6861b6, 0xad4b47];
        let set = if self.dark { dark } else { light };
        c(set[(client_id - 1).rem_euclid(4) as usize])
    }

    /// Three-layer card shadow.
    pub fn card_shadow(&self) -> Vec<BoxShadow> {
        let (a, b, d) = if self.dark { (0.42, 0.20, 0.38) } else { (0.10, 0.06, 0.10) };
        let black = |alpha| ca(0x000000, alpha);
        vec![
            BoxShadow {
                color: black(a),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(0.5),
            },
            BoxShadow {
                color: black(b),
                offset: point(px(0.), px(1.)),
                blur_radius: px(2.),
                spread_radius: px(0.),
            },
            BoxShadow {
                color: black(d),
                offset: point(px(0.), px(6.)),
                blur_radius: px(16.),
                spread_radius: px(-8.),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_differ() {
        assert_ne!(Theme::dark().stage, Theme::light().stage);
        assert!(!Theme::light().dark);
    }

    #[test]
    fn client_color_cycles() {
        let t = Theme::dark();
        assert_eq!(t.client_color(1), t.client_color(5));
        assert_ne!(t.client_color(1), t.client_color(2));
    }
}
