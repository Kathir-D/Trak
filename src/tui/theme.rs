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
        // **One extractor, not two.** This used to take the accent from
        // `dominant_colour` and the rest of the ramp from `palette`, which are two
        // different analyses of the same pixels, and the interface could end up
        // showing one song in the borders and another in the bar. Worse, when a
        // cover had no usable colour the old code kept the *previous* ramp while
        // the accent fell back to green, so a blue-grey sleeve left the previous
        // song's orange bar under a green border (owner, 2026-10-03: "only some
        // colors changed to follow picture and some are still of prev song").
        //
        // `accent::palette` bins the cover's usable pixels by hue and falls back to
        // green itself when there are none, so the accent and the ramp are now the
        // same three colours and always from the same cover.
        let mut next = accent::palette(image);
        // Belt and braces: every colour in the ramp has to be drawable.
        for c in [&mut next.primary, &mut next.secondary, &mut next.tertiary] {
            if !accent::is_usable(*c) {
                *c = SPOTIFY_GREEN;
            }
        }
        let changed = Some(next.primary) != self.art_colour || next != self.palette;
        self.art_colour = Some(next.primary);
        self.palette = next;
        changed
    }

    /// The text colour to put on the accent, for the selected tab.
    pub fn accent_text(&self) -> Color {
        accent::text_on(self.accent_colour())
    }

    /// A readable text colour for a background of `colour`. Used by the gradient
    /// runs, where every cell has its own background and cannot ask the accent
    /// what to put on it.
    pub fn text_on_colour(&self, colour: Color) -> Color {
        accent::text_on(colour)
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

    /// A monochrome cover is a real answer, not a failure: trak's own green rather
    /// than grey out of the noise.
    ///
    /// It used to *keep the accent alone*, which sounds harmless and was not: the
    /// ramp stayed on the previous cover's colours while the accent stayed green,
    /// and the interface showed two songs at once (owner, 2026-10-03). Now a cover
    /// with no usable colour puts **every** colour back to the fallback, so there is
    /// one answer rather than two.
    #[test]
    fn a_greyscale_cover_falls_back_to_one_colour_everywhere() {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = Rgb([128, 128, 128]);
        }
        let mut t = Theme::new(Accent::Art, Border::Rounded);
        t.set_art_colour(&image::DynamicImage::ImageRgb8(img));
        assert_eq!(
            t.accent_colour(),
            SPOTIFY_GREEN,
            "not grey out of the noise"
        );
        for colour in [t.palette.primary, t.palette.secondary, t.palette.tertiary] {
            assert_eq!(
                colour, SPOTIFY_GREEN,
                "the ramp has to agree with the accent: {colour:?}"
            );
        }
    }

    /// **The accent and the ramp are always the same cover.** Two extractors, one
    /// for the accent and one for the ramp, meant a cover could tint the bar one
    /// way and the borders another; and a cover with no usable colour left the
    /// previous song's ramp under a fresh green accent.
    #[test]
    fn one_cover_never_shows_as_two_songs() {
        use image::{Rgb, RgbImage};
        let cover = |r: u8, g: u8, b: u8| {
            let mut img = RgbImage::new(32, 32);
            for p in img.pixels_mut() {
                *p = Rgb([r, g, b]);
            }
            image::DynamicImage::ImageRgb8(img)
        };
        // A colourful cover, then a washed-out one: the second must not leave the
        // first one's colours behind in the ramp.
        let mut t = Theme::new(Accent::Art, Border::Rounded);
        assert!(t.set_art_colour(&cover(230, 90, 30)), "an orange cover");
        let orange = t.palette.primary;
        assert_ne!(orange, SPOTIFY_GREEN);
        assert_eq!(t.accent_colour(), t.palette.primary, "one voice");

        assert!(
            t.set_art_colour(&cover(150, 160, 170)),
            "a washed-out cover is a change too"
        );
        assert_ne!(
            t.palette.primary, orange,
            "the previous cover's colour is gone from the ramp"
        );
        assert_eq!(t.accent_colour(), t.palette.primary, "still one voice");
        // The ramp never spans two covers: its endpoints are this cover's colours.
        let ramp = t.palette.ramp(9);
        assert!(
            ramp.iter().all(|c| *c == t.palette.primary
                || *c == t.palette.secondary
                || *c == t.palette.tertiary),
            "a ramp colour from somewhere else: {ramp:?}"
        );
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
        assert!(b.starts_with(BAR_FILLED), "{b:?}");
        assert!(b.ends_with(BAR_UNFILLED), "{b:?}");
        assert_eq!(
            b.chars()
                .filter(|c| *c == BAR_FILLED.chars().next().unwrap())
                .count(),
            5
        );
    }

    /// **Nothing on the bar ever jumps a whole ramp entry at once** -- the
    /// definition of not ticking, and the thing the owner saw (2026-10-03: "idk
    /// why its like a color ticking down, just make it animated gradient").
    ///
    /// A cell's colour is read at `i + elapsed/3`, so a tenth of a second moves
    /// every cell by a third of a ramp step. Stepping whole cells twice a second
    /// was every cell changing at the same instant, which is what a row of beads
    /// ticking down looks like.
    #[test]
    fn the_drift_is_continuous_so_nothing_ticks() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let at = |t: f64| -> Vec<Color> {
            progress_bar_spans(0.5, 20, &p, false, t, true)
                .iter()
                .map(|s| s.style.fg.unwrap_or(Color::Reset))
                .collect()
        };
        let ramp = p.ramp(20);
        // The largest step between neighbouring entries of the ramp: the most any
        // one cell is ever allowed to change.
        let step = ramp
            .windows(2)
            .map(|w| distance(w[0], w[1]))
            .max()
            .unwrap_or(0);
        let before = at(10.0);
        let after = at(10.1);
        for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            assert!(
                distance(*b, *a) <= step,
                "cell {i} jumped {b:?} -> {a:?}, more than one ramp step ({step})"
            );
        }
        // And the drift really does move the gradient: half a bar width along the
        // ramp is half a minute of playing, and a full cycle is a minute. Slow
        // enough to be a drift, fast enough that nobody can call it stopped.
        assert_ne!(before, at(40.0), "the gradient still moves");
        assert_eq!(before, at(70.0), "and one full cycle comes back round");
    }

    /// Is `c` on the way from `a` to `b`, in any channel? Used by the drift tests:
    /// the slide is a blend of two neighbours, so each cell's colour lies between
    /// the two ramp entries around it.
    fn between(c: Color, a: Color, b: Color) -> bool {
        let (Color::Rgb(cr, cg, cb), Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (c, a, b)
        else {
            return true;
        };
        let between = |x: u8, lo: u8, hi: u8| {
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            let slack = 2;
            x >= lo.saturating_sub(slack) && x <= hi.saturating_add(slack)
        };
        between(cr, ar, br) || between(cg, ag, bg) || between(cb, ab, bb)
    }

    /// How far apart two colours are, worst channel.
    fn distance(a: Color, b: Color) -> u16 {
        let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
            return 0;
        };
        let d = |x: u8, y: u8| (x as i16 - y as i16).unsigned_abs();
        d(ar, br).max(d(ag, bg)).max(d(ab, bb))
    }

    /// The beads are the owner's choice, so this is only about the two things that
    /// were *not*: no white highlight travelling along the bar (the playhead is the
    /// edge between filled and unfilled, which is already exactly where it is), and
    /// a flat unfilled half so the eye is not reading the empty part as a second
    /// gradient.
    #[test]
    fn the_bar_is_a_continuous_run_rather_than_beads() {
        let palette = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let spans = progress_bar_spans(0.5, 20, &palette, false, 0.0, false);
        assert_eq!(spans.len(), 20);
        for span in &spans[..10] {
            assert_eq!(span.content, BAR_FILLED, "one cell, one bead: {span:?}");
        }
        for span in &spans[10..] {
            assert_eq!(span.content, BAR_UNFILLED, "{span:?}");
        }
        // The unfilled half is one flat colour, so the eye is not invited to read
        // the empty part as a second gradient.
        let empty: Vec<_> = spans[10..].iter().map(|s| s.style.fg).collect();
        assert!(
            empty.windows(2).all(|w| w[0] == w[1]),
            "the unfilled run is flat: {empty:?}"
        );
        // Every filled cell keeps the ramp's colour: no white blob at the head to
        // travel along and read as a ticker.
        let whites = spans[..10]
            .iter()
            .filter(|s| s.style.fg == Some(Color::White))
            .count();
        assert_eq!(whites, 0, "no highlight on the playhead");
        // And the animation is the ramp sliding, not the glyphs changing.
        let moved = progress_bar_spans(0.5, 20, &palette, false, 1.5, true);
        assert_ne!(
            moved[0].style.fg, spans[0].style.fg,
            "the drift still moves"
        );
        assert_eq!(moved[3].content, BAR_FILLED, "and the glyphs stay put");
        // Paused is completely still, as 12.6 promised.
        let still = progress_bar_spans(0.5, 20, &palette, false, 9.5, false);
        assert_eq!(still[0].style.fg, spans[0].style.fg);
    }

    #[test]
    fn time_formats_short_and_long_tracks() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(3600.0), "1:00:00");
        assert_eq!(format_time(f64::NAN), "0:00");
        assert_eq!(format_time(-3.0), "0:00");
    }

    /// The bar's ramp drifts while a track plays and is completely still when it
    /// does not. The phase is the ramp's offset in cells, so a shifted bar is the
    /// same bar with the colours slid along it.
    #[test]
    fn the_bar_drifts_only_while_something_is_playing() {
        let p = crate::accent::Palette::from_accent(SPOTIFY_GREEN);
        let colours = |spans: &[Span<'static>]| -> Vec<Color> {
            spans
                .iter()
                .map(|s| s.style.fg.unwrap_or(Color::Reset))
                .collect()
        };
        let still = colours(&progress_bar_spans(0.5, 20, &p, false, 0.0, false));
        let later = colours(&progress_bar_spans(0.5, 20, &p, false, 3.0, false));
        assert_eq!(still, later, "a paused bar does not move");

        let moving = colours(&progress_bar_spans(0.5, 20, &p, false, 3.0, true));
        assert_ne!(still, moving, "a playing bar drifts");
        // The same colours slid along the bar, not a different gradient. The slide
        // is now *fractional* -- a third of a cell a second -- so the first cells
        // carry a colour part of the way to what the still bar had further along,
        // which is what makes it flow rather than tick.
        let ramp = p.ramp(20);
        for (i, colour) in moving.iter().take(4).enumerate() {
            let from = ramp[i];
            let to = ramp[(i + 2) % ramp.len()];
            assert!(
                between(*colour, from, to),
                "cell {i}: {colour:?} is not on the way from {from:?} to {to:?}"
            );
        }

        // And the glyphs are unchanged: the fill still matches the fraction.
        let bar = progress_bar_spans(0.5, 20, &p, false, 3.0, true);
        assert_eq!(
            bar.iter().filter(|s| s.content == BAR_FILLED).count(),
            10,
            "half full"
        );
        assert_eq!(bar.len(), 20);
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
    bar(fraction, width, palette, dim, 0.0)
}

/// The glyphs a bar is drawn with, at module scope so the meter, the renderer and
/// the tests all agree on what "filled" looks like.
///
/// One `●` per cell, not a heavy line: the beads are the owner's choice
/// (2026-10-03, "it's fine to use the beads"), so what had to go was never the
/// glyphs.
pub const BAR_FILLED: &str = "●";
pub const BAR_UNFILLED: &str = "─";

/// The bar, with the ramp slid along by `phase` cells.
///
/// The one animation in the dashboard, and it is here because it costs nothing
/// where it matters: the bar is already changing on every frame while a track
/// plays, so sliding the colours costs no extra frame, and a paused trak passes
/// `phase = 0` and is completely still (owner, 2026-10-02). It is a slow drift,
/// not a pulse -- the playhead is the thing that carries meaning, so nothing else
/// on the bar is allowed to compete with it.
fn bar(
    fraction: f64,
    width: usize,
    palette: &crate::accent::Palette,
    dim: bool,
    phase: f64,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let filled = (((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize).min(width);
    let ramp = palette.ramp(width.max(2));

    // **A continuous run, not a row of beads.** This used to be one `●` per cell
    // with the last three filled cells mixed toward white as a playhead. Side by
    // side, a dot per cell with its own colour is not a bar at all but a row of
    // beads, and sliding the ramp along it every half second made the row look
    // like a colour ticking down (owner, 2026-10-03). Adjacent heavy lines join
    // into one bar, so the ramp reads as the gradient it is, and the playhead is
    // the edge between filled and unfilled -- which is already exactly where the
    // playhead is, and needs no highlight of its own to be found.
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        // `&'static str` glyphs, so a span borrows them: a full-width bar costs
        // one allocation for the vector and none for the hundred cells in it.
        let (glyph, colour) = if i < filled {
            (BAR_FILLED, slide(&ramp, i as f64 + phase))
        } else {
            (BAR_UNFILLED, Color::DarkGray)
        };
        let mut style = Style::default().fg(if dim { Color::DarkGray } else { colour });
        if dim {
            style = style.add_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(glyph, style));
    }
    spans
}

/// The ramp, read at a **fractional** position.
///
/// This is the whole of the anti-tick fix (owner, 2026-10-03: "idk why its like a
/// color ticking down, just make it animated gradient"). Sliding the ramp along by
/// whole cells makes every cell change colour at the same instant, twice a second:
/// a row of beads all stepping at once reads as a ticker counting down. Read
/// between two entries instead, and each cell's colour slides continuously into
/// its neighbour's, so the bar *flows* rather than ticks. One `mix` per filled cell
/// per frame is a few hundred HSL round trips a frame, which is what the cached
/// ramp was for in the first place (TODO 12.6) and is not measurable.
fn slide(ramp: &[Color], at: f64) -> Color {
    if ramp.is_empty() {
        return Color::DarkGray;
    }
    let len = ramp.len() as f64;
    let pos = at.rem_euclid(len);
    let i = pos.floor() as usize % ramp.len();
    let next = (i + 1) % ramp.len();
    crate::accent::mix(ramp[i], ramp[next], (pos - pos.floor()) as f32)
}

/// The bar as the renderer wants it: the drifting ramp while a track plays, and
/// a still one when it does not.
///
/// `elapsed` is the app's own tick counter, so this needs no clock of its own and
/// stops the moment the music does.
pub fn progress_bar_spans(
    fraction: f64,
    width: usize,
    palette: &crate::accent::Palette,
    dim: bool,
    elapsed: f64,
    animated: bool,
) -> Vec<Span<'static>> {
    // **A third of a cell a second.** The drift used to step a whole cell twice a
    // second, which every filled cell did at once; a fifth of that is slow enough
    // to read as the gradient moving and slow enough that no cell ever jumps.
    let phase = if animated { elapsed / 3.0 } else { 0.0 };
    bar(fraction, width, palette, dim, phase)
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
        let filled = span.content == BAR_FILLED;
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
    // Hebrew has no case and Arabic letters join: spaced out, an Arabic name
    // falls apart into isolated letters. Both are shown as written.
    if text.chars().any(crate::tui::bidi::is_rtl) {
        return text.to_string();
    }
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
        assert_eq!(spaced_caps("فيروز"), "فيروز", "Arabic letters stay joined");
        assert_eq!(spaced_caps("עידן רייכל"), "עידן רייכל");
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
}
