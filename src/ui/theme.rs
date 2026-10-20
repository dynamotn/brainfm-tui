//! Brain.fm's palette: near-black surfaces with pink→purple gradient accents.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Glyphs {
    pub play: &'static str,
    pub pause: &'static str,
    pub skip: &'static str,
    pub heart: &'static str,
    pub heart_empty: &'static str,
    pub dislike: &'static str,
    pub volume: &'static str,
    pub bar_full: &'static str,
    pub bar_empty: &'static str,
    pub dropdown: &'static str,
    pub dot: &'static str,
    pub check_on: &'static str,
    pub check_off: &'static str,
    pub favorite_header: &'static str,
    pub recent_header: &'static str,
}

const UNICODE: Glyphs = Glyphs {
    play: "▶",
    pause: "⏸",
    skip: "⏭",
    heart: "♥",
    heart_empty: "♡",
    dislike: "⊘",
    volume: "🔊",
    bar_full: "━",
    bar_empty: "─",
    dropdown: "▾",
    dot: "·",
    check_on: "◉",
    check_off: "○",
    favorite_header: "♥",
    recent_header: "◷",
};

const ASCII: Glyphs = Glyphs {
    play: ">",
    pause: "||",
    skip: ">>",
    heart: "<3",
    heart_empty: "<3",
    dislike: "x",
    volume: "vol",
    bar_full: "=",
    bar_empty: "-",
    dropdown: "v",
    dot: "·",
    check_on: "[x]",
    check_off: "[ ]",
    favorite_header: "*",
    recent_header: "~",
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub surface_hi: Color,
    pub text: Color,
    pub text_dim: Color,
    pub pink: Color,
    pub purple: Color,
    pub orange: Color,
    pub cyan: Color,
    pub truecolor: bool,
    pub glyphs: Glyphs,
}

const PINK: (u8, u8, u8) = (0xf3, 0x46, 0x75);
const PURPLE: (u8, u8, u8) = (0x91, 0x30, 0xbb);

impl Theme {
    fn rgb() -> Self {
        Self {
            bg: Color::Rgb(0x0d, 0x0c, 0x11),
            surface: Color::Rgb(0x1d, 0x19, 0x26),
            surface_hi: Color::Rgb(0x32, 0x2c, 0x44),
            text: Color::Rgb(0xff, 0xff, 0xff),
            text_dim: Color::Rgb(0xd4, 0xd2, 0xea),
            pink: Color::Rgb(PINK.0, PINK.1, PINK.2),
            purple: Color::Rgb(PURPLE.0, PURPLE.1, PURPLE.2),
            orange: Color::Rgb(0xf3, 0x99, 0x46),
            cyan: Color::Rgb(0x0b, 0xb9, 0xff),
            truecolor: true,
            glyphs: UNICODE,
        }
    }

    /// Nearest xterm-256 colors for terminals without truecolor.
    fn indexed() -> Self {
        Self {
            bg: Color::Indexed(233),
            surface: Color::Indexed(235),
            surface_hi: Color::Indexed(237),
            text: Color::Indexed(231),
            text_dim: Color::Indexed(189),
            pink: Color::Indexed(204),
            purple: Color::Indexed(97),
            orange: Color::Indexed(215),
            cyan: Color::Indexed(39),
            truecolor: false,
            glyphs: UNICODE,
        }
    }

    /// The truecolor palette, independent of the environment (screenshots).
    #[cfg(test)]
    pub(crate) fn truecolor() -> Self {
        Self::rgb()
    }

    /// Plain glyphs with the indexed palette, for stable test snapshots.
    #[cfg(test)]
    pub(crate) fn ascii() -> Self {
        Self {
            glyphs: ASCII,
            ..Self::indexed()
        }
    }

    pub(crate) fn detect() -> Self {
        let truecolor =
            std::env::var("COLORTERM").is_ok_and(|v| matches!(v.as_str(), "truecolor" | "24bit"));
        let mut t = if truecolor {
            Self::rgb()
        } else {
            Self::indexed()
        };
        if std::env::var("BRAINFM_ASCII").is_ok_and(|v| v == "1") {
            t.glyphs = ASCII;
        }
        t
    }

    /// Pink at 0.0 through purple at 1.0.
    pub(crate) fn gradient(&self, t: f64) -> Color {
        if !self.truecolor {
            return if t < 0.5 { self.pink } else { self.purple };
        }
        let t = t.clamp(0.0, 1.0);
        // `t` is clamped to 0..=1, so every mix stays between two u8 values.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
        Color::Rgb(
            mix(PINK.0, PURPLE.0),
            mix(PINK.1, PURPLE.1),
            mix(PINK.2, PURPLE.2),
        )
    }

    pub(crate) fn base(&self) -> Style {
        Style::new().fg(self.text).bg(self.bg)
    }

    pub(crate) fn dim(&self) -> Style {
        Style::new().fg(self.text_dim)
    }

    pub(crate) fn accent(&self) -> Style {
        Style::new().fg(self.pink).add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_runs_pink_to_purple() {
        let t = Theme::rgb();
        assert_eq!(t.gradient(0.0), t.pink);
        assert_eq!(t.gradient(1.0), t.purple);
        assert_eq!(t.gradient(2.0), t.purple);
    }
}
