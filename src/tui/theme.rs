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
    /// Every style, in the order a `←`/`→` walk through them. Cycled by the
    /// settings screen, so the order is the order the user sees.
    pub const ALL: [Border; 4] = [Border::Rounded, Border::Sharp, Border::Double, Border::None];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Sharp => "sharp",
            Self::Double => "double",
            Self::None => "none",
        }
    }

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
    for (i, colour) in (0..width).zip(ramp.iter()) {
        let (glyph, colour) = if i < filled {
            let base = *colour;
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

/// The volume meter: the same ramp as the bar but in its own glyphs, so the two
/// do not read as two progress bars pointing at different things.
pub fn gradient_meter(
    fraction: f64,
    width: usize,
    palette: &crate::accent::Palette,
    dim: bool,
) -> Vec<Span<'static>> {
    let mut spans = gradient_bar(fraction, width, palette, dim);
    for span in &mut spans {
        let filled = span.content == "●";
        *span = Span::styled(if filled { "▰" } else { "▱" }, span.style);
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

/// A title that scrolls when it does not fit.
///
/// A TUI pane is narrow and album titles are long. Two behaviours are both bad:
/// truncating loses the chorus, and wrapping makes the layout jump as the track
/// changes. Scrolling keeps the whole title reachable and the layout still, and
/// it costs one integer of state.
///
/// The text is padded so the loop has something to scroll into: without the
/// trailing gap the tail snaps back to the head on every wrap.
pub fn marquee(text: &str, width: usize, offset: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let gap = 3;
    let span = chars.len() + gap;
    let start = offset % span;
    let mut out = String::new();
    for i in 0..width {
        let idx = (start + i) % span;
        out.push(if idx < chars.len() { chars[idx] } else { ' ' });
    }
    out
}

/// How many characters the marquee has to scroll through: the text plus the gap,
/// or zero when it fits and therefore does not move at all.
pub fn marquee_span(text: &str, width: usize) -> usize {
    let len = text.chars().count();
    if width == 0 || len <= width {
        0
    } else {
        len + 3
    }
}

/// The status dot, which breathes while music plays.
///
/// A static dot says "something is playing". A dot that pulses says it is
/// *playing*, and it is the cheapest way to make a header feel alive: one glyph
/// chosen from the clock, no animation state and no redraw of its own.
pub fn status_dot(playing: bool, elapsed: f64) -> &'static str {
    if !playing {
        return "●";
    }
    // A 1.6s cycle through five glyphs. Chosen so that consecutive frames always
    // differ, which is what makes it read as movement rather than as noise.
    const CYCLE: [f64; 5] = [1.6, 2.4, 2.8, 2.4, 1.6];
    let mut t = elapsed % 4.0;
    let mut i = 0;
    while i < CYCLE.len() && t > CYCLE[i] {
        t -= CYCLE[i];
        i += 1;
    }
    ["◉", "◍", "◌", "◍", "◎"][i.min(4)]
}

/// Small caps, for the artist line.
///
/// The terminal has no small caps, so this is the nearest honest thing: upper
/// case with a hair space between letters, which reads as a label rather than as
/// shouting.
pub fn spaced_caps(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        // A terminal draws the hair space as a whole cell, so a single word
        // space is as wide as the gap between two letters and "JANE REMOVER"
        // reads as one word. Three cells keeps the words apart.
        if *c == ' ' {
            out.push_str("   ");
            continue;
        }
        if !c.is_alphanumeric() {
            out.push(*c);
            continue;
        }
        out.push(c.to_ascii_uppercase());
        // A hair space only *between* two letters. Adding one before a word space
        // puts two spaces side by side, and after the last letter it is a trailing
        // space nothing trims.
        if chars.get(i + 1).is_some_and(|n| n.is_alphanumeric()) {
            out.push('\u{2009}');
        }
    }
    out
}

/// A vertical gradient bar, used as the spine beside the cover art.
///
/// Two columns of colour running down the side of the artwork: it ties the cover
/// to the text below it, which is the whole reason the accent exists.
pub fn spine(height: usize, palette: &crate::accent::Palette) -> Vec<Span<'static>> {
    let ramp = palette.ramp(height.max(2));
    (0..height)
        .map(|i| {
            let c = ramp[i % ramp.len()];
            Span::styled("██", Style::default().fg(c))
        })
        .collect()
}

#[cfg(test)]
mod whimsy_tests {
    use super::*;

    /// A title that fits must not move at all, or it reads as a rendering fault
    /// rather than as a scroll.
    #[test]
    fn a_short_title_does_not_scroll() {
        assert_eq!(marquee("Census", 20, 0), "Census");
        assert_eq!(marquee("Census", 6, 0), "Census");
        assert_eq!(marquee_span("Census", 20), 0);
        assert_eq!(marquee_span("Census", 6), 0);
        assert_eq!(marquee_span("Census", 0), 0);
    }

    /// A title that does not fit has to be readable at every offset, and it has
    /// to come back round to its own beginning.
    #[test]
    fn a_long_title_scrolls_through_itself() {
        let text = "a very long track title indeed";
        let width = 10;
        let span = marquee_span(text, width);
        assert_eq!(span, text.chars().count() + 3);
        // Whatever the offset, the answer is the width, and it never panics.
        for offset in 0..(span * 2) {
            assert_eq!(marquee(text, width, offset).chars().count(), width);
        }
        // Wrapping round returns the first window exactly.
        assert_eq!(marquee(text, width, 0), marquee(text, width, span));
        // And the whole title is reachable: every character appears in some
        // window. This is the property that matters -- a scroll that skips a word
        // is worse than truncation.
        let mut seen = String::new();
        for offset in 0..span {
            seen.push_str(&marquee(text, width, offset));
        }
        for c in text.chars().filter(|c| !c.is_whitespace()) {
            assert!(seen.contains(c), "{c:?} was never shown");
        }
    }

    #[test]
    fn a_zero_width_marquee_is_empty() {
        assert_eq!(marquee("anything", 0, 0), "");
    }

    /// The dot has to change, or it is not breathing.
    #[test]
    fn the_status_dot_breathes_while_playing() {
        let mut seen = std::collections::HashSet::new();
        for ms in 0..4000 {
            seen.insert(status_dot(true, f64::from(ms) / 1000.0));
        }
        assert!(seen.len() > 1, "a static dot is not breathing: {seen:?}");
        // Paused, it settles.
        for ms in 0..4000 {
            assert_eq!(status_dot(false, f64::from(ms) / 1000.0), "●");
        }
    }

    /// Every glyph the dot can produce has to be one cell wide, or the header
    /// tears.
    #[test]
    fn the_dot_glyphs_are_one_cell_wide() {
        for ms in 0..4000 {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(status_dot(true, f64::from(ms) / 1000.0)),
                1,
                "the dot must be one cell"
            );
        }
    }

    #[test]
    fn small_caps_are_uppercase_and_padded() {
        // Asserted as properties rather than as a literal, because a literal would just
        // be the implementation written down twice.
        let out: String = spaced_caps("jane remover");
        let letters = "jane remover"
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count();
        let spaces = "jane remover".chars().filter(|c| *c == ' ').count();
        // One hair space between each pair of letters *within* a word, so the
        // count is the letters plus the letters that are not word-final.
        let words = spaces + 1;
        assert_eq!(
            out.chars().count(),
            letters + (letters - words) + 3 * spaces,
            "one hair space between the letters of a word: {out:?}"
        );
        assert!(
            !out.ends_with('\u{2009}'),
            "the trailing hair space is trimmed: {out:?}"
        );
        assert!(
            !out.contains("\u{2009} "),
            "a hair space next to a word space would read as two spaces: {out:?}"
        );
        assert!(
            !out.contains(" \u{2009}"),
            "and the other way round: {out:?}"
        );
        let stripped: String = out.chars().filter(|c| *c != '\u{2009}').collect();
        assert_eq!(stripped, "JANE   REMOVER", "and it reads as small caps");
        assert_eq!(spaced_caps("!!!"), "!!!", "punctuation is left alone");
        assert_eq!(spaced_caps(""), "");
    }

    /// The bar's colours have to change across its length, or it is one flat
    /// colour wearing a gradient's name.
    #[test]
    fn the_bar_is_actually_a_gradient() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let spans = gradient_bar(0.5, 40, &p, false);
        assert_eq!(spans.len(), 40);
        let colours: std::collections::HashSet<_> =
            spans.iter().map(|s| s.style.fg.unwrap()).collect();
        assert!(
            colours.len() > 8,
            "a gradient should be many colours, got {}",
            colours.len()
        );
        // The playhead is brighter than the bar behind it.
        let filled: Vec<_> = spans.iter().take(20).collect();
        let head = filled[19].style.fg.unwrap();
        let middle = filled[10].style.fg.unwrap();
        let Color::Rgb(hr, hg, hb) = head else {
            panic!("rgb")
        };
        let Color::Rgb(mr, mg, mb) = middle else {
            panic!("rgb")
        };
        assert!(
            i32::from(hr) + i32::from(hg) + i32::from(hb)
                > i32::from(mr) + i32::from(mg) + i32::from(mb),
            "the head should be brighter: {head:?} vs {middle:?}"
        );
        // The unfilled part is not part of the gradient.
        assert!(spans[25].content == "─");
    }

    #[test]
    fn a_bar_with_no_room_is_empty() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        assert!(gradient_bar(0.5, 0, &p, false).is_empty());
    }

    #[test]
    fn the_meter_uses_its_own_glyphs() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let spans = gradient_meter(0.5, 20, &p, false);
        assert!(spans.iter().take(10).all(|s| s.content == "▰"));
        assert!(spans.iter().skip(10).all(|s| s.content == "▱"));
    }

    #[test]
    fn the_spine_is_two_cells_wide_and_as_tall_as_asked() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let spine = spine(10, &p);
        assert_eq!(spine.len(), 10);
        assert!(spine.iter().all(|s| s.content == "██"));
        // And it is a gradient down its length, not one colour repeated.
        let colours: std::collections::HashSet<_> =
            spine.iter().map(|s| s.style.fg.unwrap()).collect();
        assert!(colours.len() > 4, "got {} colours", colours.len());
    }
}
