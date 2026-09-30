//! Colours and borders.
//!
//! Kept in one place so the settings screen, the renderer and the tests all agree
//! on what "rounded" and "accent green" mean (SPEC §2, §8).

use ratatui::style::{Color, Style};
use ratatui::widgets::BorderType;

use crate::player::PlaybackState;

/// The green from the Spotify brand, used when `accent = "green"` (SPEC §2).
pub const SPOTIFY_GREEN: Color = Color::Rgb(0x1D, 0xB9, 0x54);

/// The accent a piece of the UI draws with.
// The config strings are parsed here rather than in `config.rs`, so the schema
// and the renderer cannot disagree. Nothing reads them until TODO 5.1 wires up
// `config.toml`.
#[allow(dead_code, reason = "read by config.toml in TODO 5.1")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    /// A colour taken from the album cover (TODO 4.2). Until the extractor lands
    /// this falls back to the terminal's own green.
    Art,
    Green,
    /// The terminal's palette, so trak borrows the user's colours.
    Terminal,
}

impl Accent {
    #[allow(dead_code, reason = "read by config.toml in TODO 5.1")]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "art" => Some(Self::Art),
            "green" => Some(Self::Green),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[allow(dead_code, reason = "read by config.toml in TODO 5.1")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    Rounded,
    Sharp,
    Double,
    None,
}

impl Border {
    #[allow(dead_code, reason = "read by config.toml in TODO 5.1")]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "rounded" => Some(Self::Rounded),
            "sharp" => Some(Self::Sharp),
            "double" => Some(Self::Double),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn to_ratatui(self) -> Option<BorderType> {
        match self {
            Self::Rounded => Some(BorderType::Rounded),
            Self::Sharp => Some(BorderType::Plain),
            Self::Double => Some(BorderType::Double),
            Self::None => None,
        }
    }
}

/// Everything the renderer needs to know about colour.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Accent,
    pub border: Border,
    /// The dominant colour of the current cover, once TODO 4.2 extracts one.
    pub art_colour: Option<Color>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Accent::Art,
            border: Border::Rounded,
            art_colour: None,
        }
    }
}

impl Theme {
    pub fn new(accent: Accent, border: Border) -> Self {
        Self {
            accent,
            border,
            art_colour: None,
        }
    }

    /// The one accent colour. `Terminal` deliberately means "no colour from us".
    pub fn accent_colour(&self) -> Color {
        match self.accent {
            Accent::Green => SPOTIFY_GREEN,
            // Falls back to green until the art extractor exists, so the UI never
            // renders un-accented and looks broken.
            Accent::Art => self.art_colour.unwrap_or(SPOTIFY_GREEN),
            Accent::Terminal => Color::Reset,
        }
    }

    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent_colour())
    }

    /// The status dot in the header (SPEC §2): green playing, yellow paused, grey
    /// not running.
    pub fn status_colour(state: Option<PlaybackState>) -> Color {
        match state {
            Some(PlaybackState::Playing) => Color::Green,
            Some(PlaybackState::Paused) => Color::Yellow,
            Some(PlaybackState::Stopped) | None => Color::DarkGray,
        }
    }

    pub fn dim() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// The `▰▱` meter, like `cava` and every other player does it.
    pub fn volume_meter(volume: u8, width: usize) -> String {
        let filled = ((volume as usize * width) / 100).min(width);
        format!(
            "{}{}",
            "▰".repeat(filled),
            "▱".repeat(width.saturating_sub(filled))
        )
    }
}

/// A progress bar of `width` cells, with a `●` head.
///
/// Uses only characters that are one cell wide in every terminal trak supports,
/// because a bar that wraps is the first thing a user notices (SPEC §3, "Borders
/// must never wrap or tear").
pub fn progress_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let filled = filled.min(width);
    if filled == 0 {
        return "─".repeat(width);
    }
    if filled >= width {
        return "●".repeat(width);
    }
    format!("{}{}", "●".repeat(filled), "─".repeat(width - filled))
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_time(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 {
        secs as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_and_border_parse_the_config_strings() {
        assert_eq!(Accent::parse("art"), Some(Accent::Art));
        assert_eq!(Accent::parse("green"), Some(Accent::Green));
        assert_eq!(Accent::parse(" terminal "), Some(Accent::Terminal));
        assert_eq!(Accent::parse("purple"), None);

        assert_eq!(Border::parse("rounded"), Some(Border::Rounded));
        assert_eq!(Border::parse("sharp"), Some(Border::Sharp));
        assert_eq!(Border::parse("double"), Some(Border::Double));
        assert_eq!(Border::parse("none"), Some(Border::None));
        assert_eq!(Border::parse("wobbly"), None);
    }

    #[test]
    fn rounded_is_the_default_border() {
        assert_eq!(Theme::default().border, Border::Rounded);
    }

    #[test]
    fn the_accent_falls_back_to_green_until_art_works() {
        let t = Theme::new(Accent::Art, Border::Rounded);
        assert_eq!(t.accent_colour(), SPOTIFY_GREEN, "never render un-accented");
        let t = Theme {
            accent: Accent::Art,
            border: Border::Rounded,
            art_colour: Some(Color::Rgb(1, 2, 3)),
        };
        assert_eq!(t.accent_colour(), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn the_terminal_accent_borrows_no_colour() {
        let t = Theme::new(Accent::Terminal, Border::Rounded);
        assert_eq!(t.accent_colour(), Color::Reset);
    }

    #[test]
    fn the_status_dot_follows_the_spec() {
        assert_eq!(
            Theme::status_colour(Some(PlaybackState::Playing)),
            Color::Green
        );
        assert_eq!(
            Theme::status_colour(Some(PlaybackState::Paused)),
            Color::Yellow
        );
        assert_eq!(
            Theme::status_colour(Some(PlaybackState::Stopped)),
            Color::DarkGray
        );
        assert_eq!(Theme::status_colour(None), Color::DarkGray, "not running");
    }

    #[test]
    fn the_volume_meter_is_always_the_requested_width() {
        for v in [0u8, 1, 33, 50, 99, 100] {
            let m = Theme::volume_meter(v, 10);
            assert_eq!(m.chars().count(), 10, "at volume {v}");
            assert!(m.chars().all(|c| c == '▰' || c == '▱'));
        }
    }

    #[test]
    fn the_meter_fills_proportionally() {
        assert!(Theme::volume_meter(100, 10).starts_with("▰▰▰▰▰▰▰▰▰▰"));
        assert!(Theme::volume_meter(0, 10).starts_with("▱▱▱▱"));
        assert!(Theme::volume_meter(0, 10).ends_with("▱▱▱▱"));
    }

    /// A bar that is not exactly `width` cells is what tears a layout apart.
    #[test]
    fn the_progress_bar_is_exactly_the_requested_width() {
        for f in [0.0, 0.01, 0.5, 0.99, 1.0, -1.0, 2.0] {
            assert_eq!(progress_bar(f, 30).chars().count(), 30, "at {f}");
        }
        assert_eq!(progress_bar(0.5, 0), "");
    }

    #[test]
    fn the_progress_bar_clamps_out_of_range_fractions() {
        assert_eq!(progress_bar(-1.0, 4), "────");
        assert_eq!(progress_bar(2.0, 4), "●●●●");
    }

    #[test]
    fn the_progress_bar_grows_from_the_left() {
        let b = progress_bar(0.5, 10);
        assert!(b.starts_with('●'));
        assert!(b.ends_with('─'));
        assert_eq!(b.chars().filter(|c| *c == '●').count(), 5);
    }

    #[test]
    fn time_formats_short_and_long_tracks() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(3600.0), "1:00:00");
        assert_eq!(format_time(f64::NAN), "0:00");
        assert_eq!(format_time(-3.0), "0:00");
    }

    /// The only characters allowed in a bar or a meter: one cell wide everywhere.
    #[test]
    fn bars_use_only_single_width_characters() {
        for c in progress_bar(0.37, 20)
            .chars()
            .chain(Theme::volume_meter(37, 10).chars())
        {
            assert!(
                unicode_width::UnicodeWidthChar::width(c) == Some(1),
                "{c:?} is not one cell wide"
            );
        }
    }
}
