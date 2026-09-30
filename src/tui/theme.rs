//! Colours and borders.
//!
//! Kept in one place so the settings screen, the renderer and the tests all agree
//! on what "rounded" and "accent green" mean (SPEC §2, §8).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::BorderType;

use crate::accent::{self, Palette};
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
    /// The dominant colour of the current cover (TODO 4.2). `None` until an
    /// image has been extracted, and for a cover that has no usable colour --
    /// a black-and-white sleeve should leave the accent alone.
    pub art_colour: Option<Color>,
    /// The whole ramp, so bars and borders can be gradients rather than one flat
    /// colour (TODO 4.2). Falls back to rotations of the accent, so `green` and
    /// `terminal` get a gradient too.
    pub palette: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(Accent::Art, Border::Rounded)
    }
}

impl Theme {
    pub fn new(accent: Accent, border: Border) -> Self {
        Self {
            accent,
            border,
            art_colour: None,
            palette: Palette::from_accent(match accent {
                Accent::Terminal => Color::Reset,
                _ => SPOTIFY_GREEN,
            }),
        }
    }

    /// The one accent colour. `Terminal` deliberately means "no colour from us".
    ///
    /// `Accent::Art` uses the cover's colour when there is a usable one and falls
    /// back to green otherwise, so a monochrome sleeve never leaves the interface
    /// looking broken (SPEC §8: `accent = "art" | "green" | "terminal"`).
    pub fn accent_colour(&self) -> Color {
        match self.accent {
            Accent::Green => SPOTIFY_GREEN,
            Accent::Art => self
                .art_colour
                .filter(|c| accent::is_usable(*c))
                .unwrap_or(SPOTIFY_GREEN),
            Accent::Terminal => Color::Reset,
        }
    }

    /// Set the accent and the ramp from a freshly fetched cover.
    ///
    /// Returns whether anything changed, so the caller can tell "this cover has
    /// no colour" from "the colour is unchanged" without keeping the old value
    /// around -- and, more usefully, whether a redraw is needed at all.
    pub fn set_art_colour(&mut self, image: &image::DynamicImage) -> bool {
        let found = accent::dominant_colour(image).map(accent::ensure_contrast);
        let usable = found.filter(|c| accent::is_usable(*c));
        let mut next = match usable {
            Some(c) => Palette {
                primary: c,
                secondary: accent::palette(image).secondary,
                tertiary: accent::palette(image).tertiary,
            },
            // No usable colour in the cover: keep whatever ramp is in place. A
            // black-and-white sleeve should leave the interface alone, and
            // jumping to a synthetic ramp would be a worse answer than the green
            // it already had.
            None => self.palette,
        };
        // Every colour in the ramp has to be drawable.
        for c in [&mut next.primary, &mut next.secondary, &mut next.tertiary] {
            if !accent::is_usable(*c) {
                *c = SPOTIFY_GREEN;
            }
        }
        let changed = usable != self.art_colour || next != self.palette;
        self.art_colour = usable;
        self.palette = next;
        changed
    }

    /// The text colour to put on the accent, for the selected tab.
    pub fn accent_text(&self) -> Color {
        accent::text_on(self.accent_colour())
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

    /// `art` must never render an interface that looks broken, and it must never
    /// render one that cannot be read (TODO 4.2).
    #[test]
    fn the_art_accent_is_used_only_when_it_is_usable() {
        let t = Theme::new(Accent::Art, Border::Rounded);
        assert_eq!(t.accent_colour(), SPOTIFY_GREEN, "never render un-accented");

        // A usable cover colour is used.
        let t = Theme {
            accent: Accent::Art,
            border: Border::Rounded,
            art_colour: Some(Color::Rgb(200, 40, 40)),
            palette: Palette::from_accent(SPOTIFY_GREEN),
        };
        assert_eq!(t.accent_colour(), Color::Rgb(200, 40, 40));

        // An unusable one -- near-black here, and near-white below -- is refused
        // and green takes over. This used to assert the opposite, and the
        // opposite is the bug: an interface tinted Rgb(1, 2, 3) is unreadable on
        // a dark terminal.
        for unusable in [
            Color::Rgb(1, 2, 3),
            Color::Rgb(255, 255, 255),
            Color::Rgb(128, 128, 128),
        ] {
            let t = Theme {
                accent: Accent::Art,
                border: Border::Rounded,
                art_colour: Some(unusable),
                palette: Palette::from_accent(SPOTIFY_GREEN),
            };
            assert_eq!(
                t.accent_colour(),
                SPOTIFY_GREEN,
                "{unusable:?} should have been refused"
            );
        }

        // `green` ignores the cover entirely, and `terminal` borrows the palette.
        let t = Theme {
            accent: Accent::Green,
            border: Border::Rounded,
            art_colour: Some(Color::Rgb(200, 40, 40)),
            palette: Palette::from_accent(SPOTIFY_GREEN),
        };
        assert_eq!(t.accent_colour(), SPOTIFY_GREEN);
        let t = Theme {
            accent: Accent::Terminal,
            border: Border::Rounded,
            art_colour: Some(Color::Rgb(200, 40, 40)),
            palette: Palette::from_accent(Color::Reset),
        };
        assert_eq!(t.accent_colour(), Color::Reset);
    }

    /// The three modes of SPEC §8, end to end from an actual cover.
    #[test]
    fn the_three_accent_modes_all_work() {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = Rgb([210, 60, 30]);
        }
        let art = image::DynamicImage::ImageRgb8(img);

        let mut t = Theme::new(Accent::Art, Border::Rounded);
        assert!(t.set_art_colour(&art), "a strong cover changes the accent");
        let from_art = t.accent_colour();
        assert_ne!(from_art, SPOTIFY_GREEN, "and it is not the fallback");

        let mut green = Theme::new(Accent::Green, Border::Rounded);
        green.set_art_colour(&art);
        assert_eq!(green.accent_colour(), SPOTIFY_GREEN);

        let mut terminal = Theme::new(Accent::Terminal, Border::Rounded);
        terminal.set_art_colour(&art);
        assert_eq!(terminal.accent_colour(), Color::Reset);

        // And the text on the accent is legible whatever the accent turned out to
        // be, which is the property that makes all three modes safe.
        for theme in [&t, &green, &terminal] {
            let bg = theme.accent_colour();
            let fg = theme.accent_text();
            if bg != Color::Reset {
                assert!(
                    crate::accent::contrast_ratio(bg, fg) >= 3.0,
                    "{bg:?} with {fg:?} is not readable"
                );
            }
        }
    }

    /// A monochrome cover is a real answer, not a failure: it leaves the accent
    /// alone rather than picking grey out of the noise.
    #[test]
    fn a_greyscale_cover_leaves_the_accent_alone() {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = Rgb([128, 128, 128]);
        }
        let mut t = Theme::new(Accent::Art, Border::Rounded);
        assert!(!t.set_art_colour(&image::DynamicImage::ImageRgb8(img)));
        assert_eq!(t.art_colour, None);
        assert_eq!(t.accent_colour(), SPOTIFY_GREEN);
    }

    /// Setting the same cover twice is not a change, so the caller can skip a
    /// redraw.
    #[test]
    fn setting_the_same_cover_twice_reports_no_change() {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = Rgb([30, 120, 220]);
        }
        let art = image::DynamicImage::ImageRgb8(img);
        let mut t = Theme::new(Accent::Art, Border::Rounded);
        assert!(t.set_art_colour(&art));
        assert!(!t.set_art_colour(&art), "nothing changed the second time");
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

/// The gradient a bar is drawn with, and the ramp it comes from.
///
/// A bar drawn in one colour is a bar. Drawn as a ramp it reads as part of the
/// album rather than as a widget, and the eye can read the position off the
/// colour as well as the length — which is the one thing a bar is for.
pub fn gradient_bar(
    fraction: f64,
    width: usize,
    palette: &crate::accent::Palette,
    dim: bool,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let filled = (((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize).min(width);
    let ramp = palette.ramp(width.max(2));

    // The last three filled cells get the head colour, so the playhead reads as a
    // bright point travelling along the bar rather than a hard edge. This is the
    // whimsy that costs nothing: it is three cells.
    let head_from = filled.saturating_sub(3);
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        let (glyph, colour) = if i < filled {
            let base = ramp[i];
            let colour = if i >= head_from && filled > 3 {
                crate::accent::mix(base, Color::White, 0.35)
            } else {
                base
            };
            ("●", colour)
        } else {
            ("─", Color::DarkGray)
        };
        let mut style = Style::default().fg(if dim { Color::DarkGray } else { colour });
        if dim {
            style = style.add_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(glyph, style));
    }
    spans
}

/// A whole line as one ramp, used for the tab-strip underline and the art frame.
///
/// The last cell is nudged so the run does not end on exactly the colour the
/// next element starts with, which is what makes a gradient look like it was
/// placed rather than assembled.
pub fn gradient_line<'a>(text: &str, palette: &crate::accent::Palette) -> Line<'a> {
    let cells = text.chars().count().max(1);
    let ramp = palette.ramp(cells);
    let spans: Vec<Span<'a>> = text
        .chars()
        .enumerate()
        .map(|(i, c)| Span::styled(c.to_string(), Style::default().fg(ramp[i % ramp.len()])))
        .collect();
    Line::from(spans)
}

/// A two-tone border: the top and left edges in one colour, the bottom and right
/// in another. A box with a gradient on its edge looks lit from one side, which
/// is the cheapest depth a terminal can do.
pub fn edge_styles(palette: &crate::accent::Palette) -> [Style; 4] {
    [
        Style::default().fg(palette.primary), // top
        Style::default().fg(crate::accent::mix(palette.primary, palette.secondary, 0.4)), // right
        Style::default().fg(palette.end()),   // bottom
        Style::default().fg(crate::accent::mix(palette.primary, palette.tertiary, 0.5)), // left
    ]
}
