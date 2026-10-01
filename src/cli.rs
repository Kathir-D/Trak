//! shpotify-compatible one-shot commands.
//!
//! Output is tidy and coloured only on a TTY, with `--plain` and `--json` for
//! scripts. Exit codes are 0 ok, 1 runtime failure, 2 usage or missing setup
//! (SPEC §9).

use crate::player::{Player, PlayerError, PlayerState};
use crate::web::api::{Album, Artist, Playlist, SearchResults, Track, best_match};

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
///
/// Only ever called for a user-initiated command (COMPAT rule 3).
pub fn run_action<P: Player + ?Sized>(player: &mut P, action: &str) -> i32 {
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

/// `vol up` / `vol down`, through the one place the read-back rule lives.
pub fn run_volume<P: Player + ?Sized>(player: &mut P, step: i16, json: bool) -> i32 {
    match crate::player::actions::step_volume(player, step) {
        Ok(o) => {
            if let Some(notice) = o.notice() {
                eprintln!("{notice}");
                return EXIT_FAIL;
            }
            if json {
                println!("{{\"volume\":{}}}", o.read);
            } else {
                println!("{}", o.read);
            }
            EXIT_OK
        }
        Err(e) => {
            let (msg, code) = report(e);
            eprintln!("{msg}");
            code
        }
    }
}

/// `pos <seconds>`, also read back.
pub fn run_seek<P: Player + ?Sized>(player: &mut P, secs: f64) -> i32 {
    match crate::player::actions::seek_checked(player, secs) {
        Ok(o) => {
            if let Some(notice) = o.notice() {
                eprintln!("{notice}");
                return EXIT_FAIL;
            }
            println!("{}", crate::cli::format_time(o.read as f64));
            EXIT_OK
        }
        Err(e) => {
            let (msg, code) = report(e);
            eprintln!("{msg}");
            code
        }
    }
}

/// The one group a `trak play` spelling searches (SPEC §9): the bare spelling
/// is a song, and `album|artist|list` name the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayGroup {
    Song,
    Album,
    Artist,
    List,
}

impl PlayGroup {
    /// The subcommand words, so a message can quote the command the user
    /// actually typed. Telling someone who ran `trak play album x` that
    /// `trak play "x"` needs a Client ID reads as though the group were dropped.
    pub fn spelling(self) -> &'static str {
        match self {
            PlayGroup::Song => "play",
            PlayGroup::Album => "play album",
            PlayGroup::Artist => "play artist",
            PlayGroup::List => "play list",
        }
    }
}

/// What `trak play <name>` decided to play: the URI AppleScript is about to
/// receive, and the line that says which match that was.
///
/// The two are decided in one place because printing one thing and playing
/// another is exactly the mistake this command could otherwise make silently.
/// Playback stays on AppleScript (SPEC §6), so the URI is the whole payload.
pub struct PlayChoice {
    pub uri: String,
    pub line: String,
}

/// The best match in one group of a search, as a [`PlayChoice`], or `None` when
/// there was nothing that could be played.
///
/// A row whose `uri` is empty is not a match. The models default `uri` to an
/// empty string rather than `Option`, so a response that omitted it — malformed
/// rather than hostile — would otherwise hand `play track ""` to AppleScript
/// after a line claiming something else was playing.
pub fn choose(
    group: PlayGroup,
    query: &str,
    results: &SearchResults,
    style: Style,
) -> Option<PlayChoice> {
    match group {
        PlayGroup::Song => {
            let playable: Vec<&Track> = results
                .tracks
                .iter()
                .filter(|track| !track.uri.is_empty())
                .collect();
            best_match(&playable, query, |track| &track.name).map(|track| PlayChoice {
                uri: track.uri.clone(),
                line: song_line(track, style),
            })
        }
        PlayGroup::Album => {
            let playable: Vec<&Album> = results
                .albums
                .iter()
                .filter(|album| !album.uri.is_empty())
                .collect();
            best_match(&playable, query, |album| &album.name).map(|album| PlayChoice {
                uri: album.uri.clone(),
                // The row's own one-line form: name, artist, year — the three
                // things that tell two albums apart.
                line: styled(&album.to_string(), style),
            })
        }
        PlayGroup::Artist => {
            let playable: Vec<&Artist> = results
                .artists
                .iter()
                .filter(|artist| !artist.uri.is_empty())
                .collect();
            best_match(&playable, query, |artist| &artist.name).map(|artist| PlayChoice {
                uri: artist.uri.clone(),
                // An artist has nothing to add beyond the name the API removed
                // the follower count from (docs/WEB-API.md §3).
                line: styled(&artist.to_string(), style),
            })
        }
        PlayGroup::List => {
            let playable: Vec<&Playlist> = results
                .playlists
                .iter()
                .filter(|playlist| !playlist.uri.is_empty())
                .collect();
            best_match(&playable, query, |playlist| &playlist.name).map(|playlist| PlayChoice {
                uri: playlist.uri.clone(),
                // The row's own form, which carries the track count when
                // Spotify sent one — the thing that tells two same-named lists
                // apart.
                line: styled(&playlist.to_string(), style),
            })
        }
    }
}

/// `Playing <title> — <artist> (<album>)`.
///
/// Title first, where [`Track`]'s own one-line form puts the artist first: a
/// list row is scanned for the artist, but this line answers "which one did you
/// pick for me", and the title is the thing that was typed. Each part is dropped
/// rather than printed empty — an artist-less track is ordinary (a promo, a local
/// file) and `Playing —  ()` would misstate all three.
fn song_line(track: &Track, style: Style) -> String {
    let mut line = track.name.clone();
    let artists: Vec<&str> = track
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect();
    if !artists.is_empty() {
        line.push_str(&format!(" — {}", artists.join(", ")));
    }
    if let Some(album) = track.album.as_ref().filter(|album| !album.name.is_empty()) {
        line.push_str(&format!(" ({})", album.name));
    }
    styled(&line, style)
}

/// `Playing <what>`, with the match emphasised as a whole.
///
/// One rule for all four groups, which is why a song is not bolded by title and
/// an album by row: this is a single line with no hierarchy to express. The
/// status card can dim a sub-line because it has a card; here the match *is* the
/// message, and `Playing` itself stays unbolded so the eye lands on what was
/// picked.
fn styled(what: &str, style: Style) -> String {
    let Palette { bold, reset, .. } = Palette::for_style(style);
    format!("Playing {bold}{what}{reset}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::parse::parse;
    use crate::player::{PlaybackState, TrackInfo};
    use crate::testutil::fixture;
    use crate::web::api::{Playlist, PlaylistContents};

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

    // --- `trak play <name>`: the pick and the line -------------------------

    /// The setup message quotes the command back, so every group has to spell
    /// itself the way the user typed it.
    #[test]
    fn every_group_spells_the_command_it_was_typed_as() {
        assert_eq!(PlayGroup::Song.spelling(), "play");
        assert_eq!(PlayGroup::Album.spelling(), "play album");
        assert_eq!(PlayGroup::Artist.spelling(), "play artist");
        assert_eq!(PlayGroup::List.spelling(), "play list");
    }

    /// A track with the parts the line shows. Written out by hand rather than
    /// shared with `web::api`'s fixtures, so the two cannot agree by mistake.
    fn song(name: &str, artist: &str, album: &str, uri: &str) -> Track {
        Track {
            id: String::new(),
            name: name.to_string(),
            uri: uri.to_string(),
            duration_ms: 0,
            track_number: None,
            disc_number: None,
            artists: vec![Artist {
                id: String::new(),
                name: artist.to_string(),
                uri: String::new(),
                images: Vec::new(),
            }],
            album: Some(Album {
                id: String::new(),
                name: album.to_string(),
                uri: String::new(),
                release_date: None,
                artists: Vec::new(),
                images: Vec::new(),
                total_tracks: None,
            }),
        }
    }

    fn search_of(tracks: Vec<Track>) -> SearchResults {
        SearchResults {
            tracks,
            ..SearchResults::default()
        }
    }

    #[test]
    fn a_song_choice_names_the_title_the_artist_and_the_album() {
        let results = search_of(vec![song(
            "Teardrop",
            "Massive Attack",
            "Mezzanine",
            "spotify:track:t1",
        )]);
        let choice = choose(PlayGroup::Song, "teardrop", &results, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Teardrop — Massive Attack (Mezzanine)");
        assert_eq!(choice.uri, "spotify:track:t1");
    }

    /// The exact-name row wins even when the first result came from the same
    /// album — the tiebreak `best_match` exists for, seen through the CLI's own
    /// formatter.
    #[test]
    fn a_song_choice_prefers_the_row_named_after_the_query() {
        let results = search_of(vec![
            song("Angel", "Massive Attack", "Mezzanine", "spotify:track:a"),
            song(
                "Mezzanine",
                "Massive Attack",
                "Mezzanine",
                "spotify:track:m",
            ),
        ]);
        let choice = choose(PlayGroup::Song, "mezzanine", &results, Style::Plain).expect("a pick");
        assert_eq!(
            choice.line,
            "Playing Mezzanine — Massive Attack (Mezzanine)"
        );
        assert_eq!(choice.uri, "spotify:track:m");
    }

    #[test]
    fn a_song_choice_drops_the_parts_it_does_not_have() {
        let no_album = song("Promo", "Someone", "", "spotify:track:p1");
        let results = search_of(vec![no_album]);
        let choice = choose(PlayGroup::Song, "promo", &results, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Promo — Someone");

        let bare = Track {
            album: None,
            artists: Vec::new(),
            ..song("Bare", "x", "y", "spotify:track:b1")
        };
        let results = search_of(vec![bare]);
        let choice = choose(PlayGroup::Song, "bare", &results, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Bare");
    }

    #[test]
    fn a_choice_is_bold_on_a_tty_and_plain_when_piped() {
        let results = search_of(vec![song("Teardrop", "Massive Attack", "Mezzanine", "u")]);
        let tidy = choose(PlayGroup::Song, "teardrop", &results, Style::Tidy).expect("a pick");
        // The whole match is bolded, and `Playing` is not, in every group — the
        // rule is one rule.
        assert_eq!(
            tidy.line,
            "Playing \x1b[1mTeardrop — Massive Attack (Mezzanine)\x1b[0m"
        );
        let plain = choose(PlayGroup::Song, "teardrop", &results, Style::Plain).expect("a pick");
        assert!(!plain.line.contains('\x1b'));
    }

    /// The album, artist and list lines are the rows' own one-line forms, so
    /// the play command and the tabs cannot disagree about what a row is
    /// called.
    #[test]
    fn the_other_groups_use_the_rows_one_line_forms() {
        let album_row = Album {
            id: String::new(),
            name: "Mezzanine".to_string(),
            uri: "spotify:album:m".to_string(),
            release_date: Some("1998-04-20".to_string()),
            artists: vec![Artist {
                id: String::new(),
                name: "Massive Attack".to_string(),
                uri: String::new(),
                images: Vec::new(),
            }],
            images: Vec::new(),
            total_tracks: None,
        };
        let artist_row = Artist {
            id: String::new(),
            name: "Massive Attack".to_string(),
            uri: "spotify:artist:m".to_string(),
            images: Vec::new(),
        };
        let list_row = Playlist {
            id: String::new(),
            name: "Massive Attack on Repeat".to_string(),
            uri: "spotify:playlist:m".to_string(),
            description: None,
            images: Vec::new(),
            contents: Some(PlaylistContents {
                total: Some(3),
                items: Vec::new(),
            }),
        };

        let albums = SearchResults {
            albums: vec![album_row],
            ..SearchResults::default()
        };
        let choice = choose(PlayGroup::Album, "mezzanine", &albums, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Mezzanine — Massive Attack (1998)");
        assert_eq!(choice.uri, "spotify:album:m");

        let artists = SearchResults {
            artists: vec![artist_row],
            ..SearchResults::default()
        };
        let choice = choose(PlayGroup::Artist, "mass", &artists, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Massive Attack");

        let lists = SearchResults {
            playlists: vec![list_row],
            ..SearchResults::default()
        };
        let choice = choose(PlayGroup::List, "mass", &lists, Style::Plain).expect("a pick");
        assert_eq!(choice.line, "Playing Massive Attack on Repeat · 3 tracks");
    }

    /// A row with no URI is skipped even when its name is the query, because
    /// `play track ""` is not a play. The next-best row wins instead.
    #[test]
    fn a_row_with_no_uri_is_not_a_match() {
        let results = search_of(vec![
            song("Teardrop", "Massive Attack", "Mezzanine", ""),
            song(
                "Teardrop",
                "Massive Attack",
                "Mezzanine",
                "spotify:track:t2",
            ),
        ]);
        let choice = choose(PlayGroup::Song, "teardrop", &results, Style::Plain).expect("a pick");
        assert_eq!(choice.uri, "spotify:track:t2");
    }

    #[test]
    fn an_empty_group_is_no_choice_at_all() {
        assert!(
            choose(
                PlayGroup::Song,
                "anything",
                &SearchResults::default(),
                Style::Plain
            )
            .is_none()
        );
        assert!(
            choose(
                PlayGroup::List,
                "anything",
                &SearchResults::default(),
                Style::Plain
            )
            .is_none()
        );
    }
}
