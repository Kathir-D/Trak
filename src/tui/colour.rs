//! What colour the terminal can be asked for (TODO 11.6).
//!
//! The theme is written in RGB because the accent comes off the album cover. A
//! terminal that cannot show 24-bit colour turns an unknown escape into the
//! wrong colour or none at all, and a user who set `NO_COLOR` has asked for no
//! colour whatever the app prefers. Both are fixed in one place, on the finished
//! frame, so no widget has to know: [`apply`] rewrites the cells' colours after
//! everything has been drawn.
//!
//! `NO_COLOR` is the convention at <https://no-color.org>: set and non-empty
//! means no colour. Only colour goes -- bold, dim and reverse stay, because the
//! cursor row is drawn with reverse and a list with no visible cursor is not a
//! list.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// How many colours the terminal is taken to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR`: no foreground or background colour at all.
    None,
    /// The 16 named colours, which every terminal has.
    Ansi16,
    Ansi256,
    TrueColor,
}

impl Depth {
    /// From the three environment variables that decide it. Pure, so the real
    /// environment is read once at the edge and this can be tested.
    pub fn detect(no_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>) -> Depth {
        if no_color.is_some_and(|v| !v.is_empty()) {
            return Depth::None;
        }
        if colorterm.is_some_and(|v| matches!(v, "truecolor" | "24bit")) {
            return Depth::TrueColor;
        }
        let term = term.unwrap_or("");
        // `xterm-kitty`, `alacritty` and `xterm-ghostty` (cmux's libghostty) all
        // do 24-bit and not all of them set COLORTERM in every wrapper, but they
        // all also do 256, which is the safe answer when nothing says truecolor.
        if term.contains("256color") || term.contains("kitty") || term.contains("ghostty") {
            return Depth::Ansi256;
        }
        // Nothing says more than sixteen: the answer that cannot be wrong, rather
        // than the one that would look best.
        Depth::Ansi16
    }

    pub fn from_env() -> Depth {
        Depth::detect(
            std::env::var("NO_COLOR").ok().as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }
}

/// Ask the terminal what its background is (OSC 11), for [`is_light`].
///
/// Must run in raw mode, after the alternate screen is up and before anything
/// else reads input. A Device Status Report goes out behind the question
/// because every terminal answers that one: its reply ends the read at once, so
/// a terminal that ignores OSC 11 costs a round trip, not the timeout. The read
/// is `poll` on the descriptor rather than a reader thread, because a thread
/// left blocked in `read` by a silent terminal would eat the user's first key.
pub fn query_background() -> Option<(u8, u8, u8)> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut out = std::io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\\x1b[5n").ok()?;
    out.flush().ok()?;
    let fd = std::io::stdin().as_raw_fd();
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut got = Vec::new();
    while !got.windows(4).any(|w| w == b"\x1b[0n") {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, and a buffer `read` may fill up to its
        // length; nothing else holds either.
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let mut buf = [0u8; 128];
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        got.extend_from_slice(&buf[..n as usize]);
    }
    parse_osc11(&got)
}

/// The colour in an OSC 11 reply: `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ended by BEL
/// or ST, each channel one to four hex digits (xterm's format).
pub fn parse_osc11(reply: &[u8]) -> Option<(u8, u8, u8)> {
    let text = String::from_utf8_lossy(reply);
    let start = text.find("]11;rgb:")? + "]11;rgb:".len();
    let body: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '/')
        .collect();
    let channel = |h: &str| -> Option<u8> {
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let max = (1u32 << (4 * h.len())) - 1;
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    let mut parts = body.split('/');
    let rgb = (
        channel(parts.next()?)?,
        channel(parts.next()?)?,
        channel(parts.next()?)?,
    );
    parts.next().is_none().then_some(rgb)
}

/// Relative luminance (WCAG), 0 for black to 1 for white.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Whether a background is light enough that pale text on it is unreadable.
pub fn is_light(rgb: (u8, u8, u8)) -> bool {
    luminance(rgb) > 0.4
}

/// The brightest a foreground may be on a light background: 3:1 against white,
/// WCAG's floor for large or bold text, which is most of what is coloured.
const LIGHT_MAX: f64 = 0.3;

/// Darken every RGB foreground that would wash out on a light background.
///
/// The accent comes off the album cover and is tuned for a dark terminal: a
/// pale tan cover gave pale tan borders and titles that all but vanished on
/// white (seen in cmux, 2026-10-02). Hue is kept and only lightness changes, so
/// the theme still matches the cover. Cells with a background of their own (a
/// selected tab) are left as they are: that pair was chosen together.
///
/// Dim text is the other half of it: a terminal draws DIM by blending towards
/// the background, which on white makes secondary text paler still. It loses
/// the modifier and is darkened like the rest; lightness alone keeps it
/// secondary, because it was a lighter colour to begin with.
pub fn for_light_background(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        if let (Color::Rgb(r, g, b), Color::Reset) = (cell.fg, cell.bg) {
            let (r, g, b) = darken_to((r, g, b), LIGHT_MAX);
            cell.fg = Color::Rgb(r, g, b);
            cell.modifier.remove(Modifier::DIM);
        }
    }
}

/// `rgb` scaled in linear light until its luminance is at most `max`.
fn darken_to(rgb: (u8, u8, u8), max: f64) -> (u8, u8, u8) {
    let l = luminance(rgb);
    if l <= max {
        return rgb;
    }
    let k = max / l;
    let scale = |c: u8| {
        let c = f64::from(c) / 255.0;
        let lin = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        } * k;
        let s = if lin <= 0.0031308 {
            lin * 12.92
        } else {
            1.055 * lin.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (scale(rgb.0), scale(rgb.1), scale(rgb.2))
}

/// Rewrite every cell's colours for `depth`. A no-op for true colour, so the
/// common case costs one comparison per frame.
pub fn apply(buf: &mut Buffer, depth: Depth) {
    if depth == Depth::TrueColor {
        return;
    }
    for cell in buf.content.iter_mut() {
        cell.fg = convert(cell.fg, depth);
        cell.bg = convert(cell.bg, depth);
    }
}

fn convert(color: Color, depth: Depth) -> Color {
    match depth {
        Depth::TrueColor => color,
        Depth::None => Color::Reset,
        Depth::Ansi256 => match color {
            Color::Rgb(r, g, b) => Color::Indexed(nearest_256(r, g, b)),
            other => other,
        },
        Depth::Ansi16 => match color {
            Color::Rgb(r, g, b) => nearest_16(r, g, b),
            Color::Indexed(i) => indexed_to_rgb(i)
                .map(|(r, g, b)| nearest_16(r, g, b))
                .unwrap_or(Color::Reset),
            other => other,
        },
    }
}

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The sixteen, with the RGB most terminals draw them as (xterm's defaults).
const SIXTEEN: [(Color, (u8, u8, u8)); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (205, 0, 0)),
    (Color::Green, (0, 205, 0)),
    (Color::Yellow, (205, 205, 0)),
    (Color::Blue, (0, 0, 238)),
    (Color::Magenta, (205, 0, 205)),
    (Color::Cyan, (0, 205, 205)),
    (Color::Gray, (229, 229, 229)),
    (Color::DarkGray, (127, 127, 127)),
    (Color::LightRed, (255, 0, 0)),
    (Color::LightGreen, (0, 255, 0)),
    (Color::LightYellow, (255, 255, 0)),
    (Color::LightBlue, (92, 92, 255)),
    (Color::LightMagenta, (255, 0, 255)),
    (Color::LightCyan, (0, 255, 255)),
    (Color::White, (255, 255, 255)),
];

fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    SIXTEEN
        .iter()
        .min_by_key(|(_, rgb)| dist((r, g, b), *rgb))
        .map(|(c, _)| *c)
        .unwrap_or(Color::Reset)
}

const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The nearest of the 6x6x6 cube (16..=231) and the grey ramp (232..=255).
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| l.abs_diff(v))
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let avg = ((u16::from(r) + u16::from(g) + u16::from(b)) / 3) as u8;
    let step = (i32::from(avg) - 8).div_euclid(10).clamp(0, 23) as u8;
    let grey = 8 + 10 * step;
    if dist((r, g, b), (grey, grey, grey)) < dist((r, g, b), cube) {
        232 + step
    } else {
        cube_index
    }
}

/// The RGB behind a 256-palette index, for a terminal that only has sixteen.
fn indexed_to_rgb(i: u8) -> Option<(u8, u8, u8)> {
    match i {
        0..=15 => Some(SIXTEEN[usize::from(i)].1),
        16..=231 => {
            let n = i - 16;
            Some((
                LEVELS[usize::from(n / 36)],
                LEVELS[usize::from(n / 6 % 6)],
                LEVELS[usize::from(n % 6)],
            ))
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            Some((v, v, v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};

    #[test]
    fn an_osc11_reply_parses_with_either_terminator_and_any_digit_count() {
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:fafa/fafa/fafa\x1b\\\x1b[0n"),
            Some((250, 250, 250))
        );
        assert_eq!(parse_osc11(b"\x1b]11;rgb:00/00/00\x07"), Some((0, 0, 0)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/8/0\x07"), Some((255, 136, 0)));
        // A terminal that ignores the question answers only the status report.
        assert_eq!(parse_osc11(b"\x1b[0n"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/ff\x07"), None);
    }

    #[test]
    fn light_means_a_light_background() {
        assert!(is_light((250, 250, 250)));
        assert!(is_light((238, 232, 213))); // solarized light
        assert!(!is_light((0, 0, 0)));
        assert!(!is_light((40, 42, 54)));
    }

    #[test]
    fn pale_text_on_a_light_background_is_darkened_and_keeps_its_hue() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf[(0, 0)].set_fg(Color::Rgb(230, 210, 170)); // pale tan accent
        buf[(1, 0)]
            .set_fg(Color::Rgb(40, 40, 40))
            .set_style(Style::default().add_modifier(Modifier::DIM)); // already dark
        buf[(2, 0)]
            .set_fg(Color::Rgb(250, 250, 250))
            .set_bg(Color::Rgb(200, 0, 0)); // a chosen pair
        for_light_background(&mut buf);
        let Color::Rgb(r, g, b) = buf[(0, 0)].fg else {
            panic!("still rgb")
        };
        assert!(luminance((r, g, b)) <= LIGHT_MAX + 0.01);
        assert!(r > g && g > b, "hue kept: {r} {g} {b}");
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(40, 40, 40));
        assert!(!buf[(1, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(2, 0)].fg, Color::Rgb(250, 250, 250));
    }

    #[test]
    fn no_color_set_and_non_empty_wins_over_everything() {
        assert_eq!(
            Depth::detect(Some("1"), Some("truecolor"), Some("xterm-256color")),
            Depth::None
        );
        // The convention is "non-empty", so an empty value is not a request.
        assert_eq!(
            Depth::detect(Some(""), Some("truecolor"), None),
            Depth::TrueColor
        );
        assert_eq!(Depth::detect(None, Some("24bit"), None), Depth::TrueColor);
    }

    #[test]
    fn the_depth_falls_back_from_what_the_term_says() {
        assert_eq!(
            Depth::detect(None, None, Some("xterm-256color")),
            Depth::Ansi256
        );
        assert_eq!(
            Depth::detect(None, None, Some("xterm-ghostty")),
            Depth::Ansi256
        );
        assert_eq!(Depth::detect(None, None, Some("xterm")), Depth::Ansi16);
        assert_eq!(Depth::detect(None, None, Some("dumb")), Depth::Ansi16);
        assert_eq!(Depth::detect(None, None, None), Depth::Ansi16);
    }

    #[test]
    fn rgb_lands_on_the_nearest_named_colour() {
        assert_eq!(nearest_16(255, 0, 0), Color::LightRed);
        assert_eq!(nearest_16(200, 10, 10), Color::Red);
        assert_eq!(nearest_16(0, 0, 0), Color::Black);
        assert_eq!(nearest_16(250, 250, 250), Color::White);
        // Spotify green is a green, not a grey.
        assert_eq!(nearest_16(0x1d, 0xb9, 0x54), Color::Green);
    }

    #[test]
    fn the_256_palette_uses_the_cube_for_colour_and_the_ramp_for_grey() {
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        let grey = nearest_256(128, 128, 128);
        assert!((232..=255).contains(&grey) || grey == 244, "{grey}");
        assert!((232..=255).contains(&nearest_256(100, 101, 100)));
    }

    #[test]
    fn only_colour_is_removed_for_no_color_not_the_reverse_that_marks_the_cursor() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(
            0,
            0,
            "abc",
            Style::default()
                .fg(Color::Rgb(1, 2, 3))
                .bg(Color::Red)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        );
        apply(&mut buf, Depth::None);
        for cell in &buf.content {
            assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset));
            assert!(cell.modifier.contains(Modifier::REVERSED | Modifier::BOLD));
        }
        assert_eq!(buf[(1, 0)].symbol(), "b", "the text is untouched");
    }

    #[test]
    fn true_color_is_left_alone_and_conversion_is_stable() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.set_string(0, 0, "x", Style::default().fg(Color::Rgb(9, 99, 199)));
        let before = buf.clone();
        apply(&mut buf, Depth::TrueColor);
        assert_eq!(buf, before);
        apply(&mut buf, Depth::Ansi16);
        let once = buf.clone();
        apply(&mut buf, Depth::Ansi16);
        assert_eq!(buf, once, "named colours convert to themselves");
        assert!(!matches!(
            buf[(0, 0)].fg,
            Color::Rgb(..) | Color::Indexed(_)
        ));
    }

    #[test]
    fn a_256_index_is_reduced_for_a_16_colour_terminal() {
        assert_eq!(convert(Color::Indexed(196), Depth::Ansi16), Color::LightRed);
        assert_eq!(convert(Color::Indexed(2), Depth::Ansi16), Color::Green);
    }
}
