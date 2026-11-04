//! Design tokens.

use gpui::{App, BoxShadow, Global, Hsla, Rgba, point, px, rgb, rgba};

const PROJECT_DARK: [u32; 14] = [
    0xae7fff, 0x3ccba2, 0xffb14a, 0xff745c, 0xf270a6, 0x95d555, 0x5cb5ff, 0xffcf57, 0x78dfe1,
    0xc78fff, 0xff8b68, 0x7edc87, 0x7b9cff, 0xe880d6,
];
const PROJECT_LIGHT: [u32; 14] = [
    0x7640d4, 0x108a68, 0xb87008, 0xc83e28, 0xb83070, 0x548c18, 0x1870c0, 0xa87c00, 0x10888c,
    0x8850d4, 0xc44c28, 0x28883c, 0x385cd0, 0xb0389c,
];

fn project_slot(key: &str) -> usize {
    let hash = key
        .bytes()
        .fold(5381usize, |acc, byte| acc.wrapping_mul(33) ^ byte as usize);
    hash % PROJECT_DARK.len()
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex_rgba: u32) -> Hsla {
    rgba(hex_rgba).into()
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub chrome: Hsla,
    pub page: Hsla,
    pub panel: Hsla,
    pub raised: Hsla,
    pub pop: Hsla,
    pub ink: Hsla,
    pub body: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub accent: Hsla,
    pub tint: Hsla,
    pub tint_line: Hsla,
    pub fill: Hsla,
    pub fill_top: Hsla,
    pub fill_h: Hsla,
    pub on_fill: Hsla,
    pub shade: Hsla,
    pub bar: Hsla,
    pub warn: Hsla,
    pub bad: Hsla,
    pub good: Hsla,
    pub st_active: Hsla,
    pub st_todo: Hsla,
    pub pr_low: Hsla,
    pub pr_high: Hsla,
    pub pr_urgent: Hsla,
    /// The star marking an epic, as the TUI colours it.
    pub epic: Hsla,
    /// The check in the logo, as the theme's logo SVG fills it.
    pub logo_check: Hsla,
    /// Light thrown by the logo as it resolves in the intro.
    pub logo_glow: Hsla,
    dlg_shadow: (Hsla, f32, f32),
    menu_shadow: (Hsla, f32, f32),
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            chrome: c(0x100f0e),
            page: c(0x181716),
            panel: c(0x1f1e1b),
            raised: c(0x282623),
            pop: c(0x272420),
            ink: c(0xeeece6),
            body: c(0xc3c0b8),
            muted: c(0xaca69e),
            faint: c(0x918b83),
            line: c(0x2c2a27),
            line2: c(0x3b3834),
            accent: c(0xb68af5),
            tint: c(0x2a2233),
            tint_line: c(0x5c4676),
            fill: c(0x5a387a),
            fill_top: c(0x6a4690),
            fill_h: c(0x6a4490),
            on_fill: c(0xffffff),
            shade: ca(0x0a09088c),
            bar: c(0x4a4845),
            warn: c(0xd9851f),
            bad: c(0xe8595c),
            good: c(0x5ea732),
            st_active: c(0xd9a520),
            st_todo: c(0x4680cb),
            pr_low: c(0x7d8db0),
            pr_high: c(0xe8952a),
            pr_urgent: c(0xef5256),
            epic: c(0xffcf57),
            logo_check: c(0xffffff),
            logo_glow: c(0xe8d5ff),
            dlg_shadow: (ca(0x00000080), 20., 50.),
            menu_shadow: (ca(0x00000066), 12., 32.),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            chrome: c(0xefece6),
            page: c(0xfdfcfa),
            panel: c(0xf6f4ef),
            raised: c(0xebe7df),
            pop: c(0xfffdfa),
            ink: c(0x1a1816),
            body: c(0x3f3b37),
            muted: c(0x68625c),
            faint: c(0x756e67),
            line: c(0xe7e2d9),
            line2: c(0xd8d2c7),
            accent: c(0x6b3fc4),
            tint: c(0xece4f8),
            tint_line: c(0xb9a0e6),
            fill: c(0x6237b8),
            fill_top: c(0x7147c9),
            fill_h: c(0x5a33ab),
            on_fill: c(0xffffff),
            shade: ca(0x3c322840),
            bar: c(0xd0cbc2),
            warn: c(0xb5650f),
            bad: c(0xc93034),
            good: c(0x3f8a1c),
            st_active: c(0xb8860b),
            st_todo: c(0x2f6db5),
            pr_low: c(0x5f6f92),
            pr_high: c(0xcf7a12),
            pr_urgent: c(0xd93c40),
            epic: c(0xa87c00),
            logo_check: c(0x2a1547),
            logo_glow: c(0x965ff5),
            dlg_shadow: (ca(0x30261c24), 18., 48.),
            menu_shadow: (ca(0x30261c1f), 12., 32.),
        }
    }

    /// The same hash and palette as the TUI, so a project keeps its colour in both.
    pub fn project(&self, key: &str) -> Hsla {
        let palette = if self.dark {
            PROJECT_DARK
        } else {
            PROJECT_LIGHT
        };
        c(palette[project_slot(key)])
    }

    /// The dot beside a project keeps its raw colour in both themes.
    pub fn project_dot(&self, key: &str) -> Hsla {
        Theme::dark().project(key)
    }

    pub fn ring(&self, color: Hsla) -> BoxShadow {
        BoxShadow {
            color,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
            inset: false,
        }
    }

    pub fn inset_ring(&self, color: Hsla) -> BoxShadow {
        BoxShadow {
            inset: true,
            spread_radius: px(1.),
            ..self.ring(color)
        }
    }

    fn drop(shadow: (Hsla, f32, f32)) -> BoxShadow {
        BoxShadow {
            color: shadow.0,
            offset: point(px(0.), px(shadow.1)),
            blur_radius: px(shadow.2),
            spread_radius: px(0.),
            inset: false,
        }
    }

    pub fn dlg_shadow(&self) -> Vec<BoxShadow> {
        vec![Self::drop(self.dlg_shadow), self.ring(self.line2)]
    }

    /// The highlighted row in a menu: a warm lift of the menu surface.
    pub fn pick(&self) -> Hsla {
        mix(self.pop, self.ink, 0.1)
    }

    /// The selected row's fill and ring.
    pub fn selection(&self, active: bool) -> (Hsla, Hsla) {
        if active {
            (self.tint, self.tint_line)
        } else {
            (self.raised, self.line2)
        }
    }

    pub fn menu_shadow(&self) -> Vec<BoxShadow> {
        vec![Self::drop(self.menu_shadow), self.ring(self.line2)]
    }

    pub fn sheet_shadow(&self) -> Vec<BoxShadow> {
        let mut shadows = vec![self.ring(self.line)];
        if !self.dark {
            shadows.insert(
                0,
                BoxShadow {
                    color: ca(0x30261c0f),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                },
            );
        }
        shadows
    }
}

/// Blends two colours in RGB, so hues never swing through the wheel.
pub fn mix(a: Hsla, b: Hsla, d: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    Rgba {
        r: a.r + (b.r - a.r) * d,
        g: a.g + (b.g - a.g) * d,
        b: a.b + (b.b - a.b) * d,
        a: a.a + (b.a - a.a) * d,
    }
    .into()
}

pub fn theme(cx: &App) -> Theme {
    *cx.global::<Theme>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_lands_on_the_tui_palette_slot() {
        for (key, slot) in [("aven", 9), ("workmux", 6), ("Zendit", 11), ("inbox", 1)] {
            assert_eq!(project_slot(key), slot, "{key}");
        }
        assert_eq!(Theme::dark().project("aven"), c(0xc78fff));
        assert_eq!(Theme::light().project("aven"), c(0x8850d4));
    }
}
