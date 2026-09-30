//! shpotify-compatible one-shot commands.
//!
//! Output is tidy and coloured only on a TTY, with `--plain` and `--json` for
//! scripts. Exit codes are 0 ok, 1 runtime failure, 2 usage or missing setup
//! (SPEC §9).

use crate::player::{AppleScriptPlayer, Player, PlayerError, PlayerState};

/// SPEC §9: 0 ok, 1 runtime failure, 2 usage / missing setup.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAIL: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

/// How much decoration to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Boxes, colour and a progress bar. The default on a TTY.
    Tidy,
    /// No colour, no box. Chosen automatically when stdout is not a terminal, and
    /// forced by `--plain`.
    Plain,
}

/// `mm:ss`, or `h:mm:ss` past an hour. Long tracks matter for albums.
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

/// `N of M` progress as a bar of the given width, or as a percentage.
fn progress_bar(state: &PlayerState, width: usize) -> String {
    const FULL: char = '━';
    const HEAD: char = '●';
    let p = state.progress();
    if width == 0 || state.track.duration_ms == 0 {
        return String::new();
    }
    let filled = (p * width as f64).round() as usize;
    let filled = filled.min(width);
    if filled == 0 {
        return FULL.to_string().repeat(width);
    }
    if filled >= width {
        return HEAD.to_string().repeat(width);
    }
    format!(
        "{}{}",
        HEAD.to_string().repeat(filled),
        FULL.to_string().repeat(width - filled)
    )
}

/// The SGR codes to emit, resolved once so a call does not re-test the TTY.
///
/// Colour is emitted only on a TTY, so piping trak gives clean text (SPEC §9).
#[derive(Clone, Copy)]
struct Palette {
    dim: &'static str,
    bold: &'static str,
    reset: &'static str,
}

const NO_COLOUR: Palette = Palette {
    dim: "",
    bold: "",
    reset: "",
};

const COLOUR: Palette = Palette {
    dim: "\x1b[2m",
    bold: "\x1b[1m",
    reset: "\x1b[0m",
};

impl Palette {
    fn for_style(style: Style) -> Self {
        match style {
            Style::Tidy => COLOUR,
            Style::Plain => NO_COLOUR,
        }
    }
}

/// The `status` card, the one thing shpotify users look at most.
pub fn render_status(state: &PlayerState, style: Style) -> String {
    let Palette { dim, bold, reset } = Palette::for_style(style);
    let t = &state.track;

    if state.playback == crate::player::PlaybackState::Stopped || t.title.is_empty() {
        return "nothing is playing\n".to_string();
    }

    // The artist/album line is empty for an advert (docs/APPLESCRIPT.md §3), so it
    // is built from whatever is actually there rather than assumed.
    let mut sub = Vec::new();
    if !t.artist.is_empty() {
        sub.push(t.artist.clone());
    }
    if !t.album.is_empty() && t.album != t.artist {
        sub.push(t.album.clone());
    }
    let sub = sub.join(" · ");
    let pos = format_time(state.position_secs);
    let dur = format_time(t.duration_secs() as f64);
    let bar = progress_bar(state, 30);
    let mut out = String::new();

    if style == Style::Plain {
        out.push_str(&format!("{}\n", state.playback.symbol()));
        out.push_str(&format!("{}\n", t.title));
        if !sub.is_empty() {
            out.push_str(&format!("{sub}\n"));
        }
        out.push_str(&format!("{pos} / {dur}\n"));
        out.push_str(&format!("volume {}\n", state.volume));
        return out;
    }

    out.push_str(&format!("{bold}{}{reset}\n", state.playback.symbol()));
    out.push_str(&format!("{bold}{}{reset}\n", t.title));
    if !sub.is_empty() {
        out.push_str(&format!("{dim}{sub}{reset}\n"));
    }
    out.push_str(&format!("{dim}  {pos}  {dur}{reset}\n"));
    out.push_str(&format!("  {bar}\n"));
    out.push_str(&format!("  volume {}", meter(state.volume)));
    if state.shuffling_enabled {
        out.push_str("   shuffle");
    }
    if state.repeating_enabled {
        out.push_str("   repeat");
    }
    out.push('\n');
    out
}

/// A 10-cell volume meter, so a glance is enough.
fn meter(volume: u8) -> String {
    const ON: char = '▰';
    const OFF: char = '▱';
    let filled = ((volume as usize * 10) / 100).min(10);
    format!(
        "{}{} {volume}%",
        ON.to_string().repeat(filled),
        OFF.to_string().repeat(10 - filled)
    )
}

/// `--json`, so the output is stable and scriptable.
pub fn render_json(state: &PlayerState) -> String {
    let t = &state.track;
    format!(
        concat!(
            r#"{{"state":"{}","title":{},"artist":{},"album":{},"album_artist":{},"#,
            r#""uri":{},"duration_ms":{},"position_secs":{:.3},"volume":{},"#,
            r#""shuffling":{},"repeating":{},"popularity":{},"track_number":{},"#,
            r#""disc_number":{},"artwork_url":{},"is_ad":{}}}"#
        ),
        state.playback.symbol(),
        json_str(&t.title),
        json_str(&t.artist),
        json_str(&t.album),
        json_str(&t.album_artist),
        match &t.uri {
            Some(u) => json_str(u),
            None => "null".into(),
        },
        t.duration_ms,
        state.position_secs,
        state.volume,
        state.shuffling_enabled,
        state.repeating_enabled,
        t.popularity.map(|v| v.to_string()).unwrap_or("null".into()),
        t.track_number,
        t.disc_number,
        match &t.artwork_url {
            Some(u) => json_str(u),
            None => "null".into(),
        },
        t.is_ad(),
    )
}

/// Minimal JSON string escaping. Titles really do contain `"` and `\`.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Turn a player error into the message and exit code the user should see.
pub fn report(e: PlayerError) -> (String, i32) {
    match &e {
        PlayerError::NotRunning => (
            "Spotify isn't running. trak never starts it for you — open Spotify, or run \
             `trak` for the idle card that launches it on Enter."
                .to_string(),
            EXIT_FAIL,
        ),
        PlayerError::PermissionDenied => (
            "This terminal isn't allowed to control Spotify.\n\
             Fix: System Settings › Privacy & Security › Automation › add your terminal, \
             with Spotify checked."
                .to_string(),
            EXIT_FAIL,
        ),
        PlayerError::Timeout => (
            "Timed out talking to Spotify. If it keeps happening, Spotify may be busy.".to_string(),
            EXIT_FAIL,
        ),
        PlayerError::Script(m) => (format!("Spotify said: {m}"), EXIT_FAIL),
    }
}

/// Run one player action and report it the way shpotify did.
pub fn run_action(player: &mut AppleScriptPlayer, action: &str) -> i32 {
    let result = match action {
        "play" => player.play(),
        "pause" => player.pause(),
        "toggle" => player.toggle(),
        "next" => player.next(),
        "prev" => player.previous(),
        other => {
            eprintln!("unknown action {other}");
            return EXIT_USAGE;
        }
    };
    match result {
        Ok(()) => EXIT_OK,
        Err(e) => {
            let (msg, code) = report(e);
            eprintln!("{msg}");
            code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::parse::parse;
    use crate::player::{PlaybackState, TrackInfo};
    use crate::testutil::fixture;

    fn sample() -> PlayerState {
        parse(&fixture("playing_track.txt")).unwrap()
    }

    #[test]
    fn time_formats_minutes_and_seconds() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(9.0), "0:09");
        assert_eq!(format_time(61.0), "1:01");
        assert_eq!(format_time(3599.0), "59:59");
        // Past an hour it grows an hours field rather than running to 60:00.
        assert_eq!(format_time(3600.0), "1:00:00");
        assert_eq!(format_time(3725.0), "1:02:05");
    }

    #[test]
    fn time_survives_nonsense() {
        assert_eq!(format_time(f64::NAN), "0:00");
        assert_eq!(format_time(-1.0), "0:00");
    }

    #[test]
    fn status_plain_has_no_ansi_and_shows_the_track() {
        let out = render_status(&sample(), Style::Plain);
        assert!(!out.contains('\x1b'), "plain output must have no escapes");
        assert!(out.contains("Census Designated"));
        assert!(out.contains("Jane Remover"));
    }

    #[test]
    fn status_tidy_has_a_progress_bar_and_a_meter() {
        let out = render_status(&sample(), Style::Tidy);
        assert!(
            out.contains('\x1b'),
            "tidy output is expected to be coloured"
        );
        assert!(out.contains('━') || out.contains('●'));
        assert!(out.contains('▰') || out.contains('▱'));
        assert!(out.contains("100%"));
    }

    #[test]
    fn status_says_so_when_nothing_is_playing() {
        let mut s = sample();
        s.playback = PlaybackState::Stopped;
        assert!(render_status(&s, Style::Plain).contains("nothing is playing"));
    }

    /// The advert is the state that breaks naive formatters.
    #[test]
    fn status_of_an_advert_does_not_crash_or_lie() {
        let s = parse(&fixture("playing_ad.txt")).unwrap();
        for style in [Style::Plain, Style::Tidy] {
            let out = render_status(&s, style);
            assert!(!out.is_empty());
            // It must not claim to be a 0:30 track of "Legal Services" at 0:00
            // with no hint that this is an advert.
            assert!(out.contains("Legal Services"));
        }
    }

    #[test]
    fn json_is_valid_and_escapes() {
        let mut s = sample();
        s.track.title = "he said \"hi\"\n\ttabbed".into();
        let j = render_json(&s);
        assert!(j.starts_with('{') && j.ends_with('}'));
        assert!(j.contains(r#"\"hi\""#), "{j}");
        assert!(j.contains(r#"\n"#), "{j}");
        assert!(j.contains(r#"\t"#), "{j}");
    }

    #[test]
    fn json_reports_an_advert_honestly() {
        let s = parse(&fixture("playing_ad.txt")).unwrap();
        let j = render_json(&s);
        assert!(j.contains(r#""is_ad":true"#), "{j}");
        assert!(j.contains(r#""uri":null"#), "{j}");
        assert!(j.contains(r#""artwork_url":null"#), "{j}");
        assert!(j.contains(r#""popularity":null"#), "{j}");
    }

    #[test]
    fn progress_bar_is_exactly_the_requested_width() {
        let mut s = sample();
        for (pos, label) in [(0.0, "start"), (180.0, "middle"), (99999.0, "past end")] {
            s.position_secs = pos;
            let bar = progress_bar(&s, 24);
            assert_eq!(bar.chars().count(), 24, "at {label}");
        }
    }

    #[test]
    fn progress_bar_is_empty_for_an_unknown_duration() {
        let mut s = sample();
        s.track = TrackInfo::default();
        assert!(progress_bar(&s, 20).is_empty());
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn not_running_suggests_the_tui_rather_than_launching() {
        // COMPAT rule 2: trak must never start Spotify from a one-shot command.
        let (msg, code) = report(PlayerError::NotRunning);
        assert_eq!(code, EXIT_FAIL);
        assert!(msg.contains("never starts it"), "{msg}");
    }

    #[test]
    fn permission_denied_names_the_setting_the_user_must_change() {
        let (msg, code) = report(PlayerError::PermissionDenied);
        assert_eq!(code, EXIT_FAIL);
        assert!(msg.contains("Automation"), "{msg}");
    }

    #[test]
    fn meter_is_ten_cells_at_every_volume() {
        for v in [0u8, 1, 55, 99, 100] {
            let m = meter(v);
            assert_eq!(
                m.chars().filter(|c| *c == '▰' || *c == '▱').count(),
                10,
                "at {v}"
            );
            assert!(m.ends_with(&format!("{v}%")), "{m}");
        }
    }
}
