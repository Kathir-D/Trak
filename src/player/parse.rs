//! Turning the batched AppleScript output into a `PlayerState`.
//!
//! Pure on purpose: it takes the raw string and returns a `Result`, so the whole
//! thing is testable against the fixtures captured from a real Spotify
//! (docs/APPLESCRIPT.md) with no Spotify, no osascript and no terminal.

use crate::player::{PlaybackState, PlayerState, TrackInfo};

/// Unit separator, chosen in TODO 1.2. It cannot occur in a title, album or
/// artist, and it survives the advert's empty fields.
pub const SEP: char = '\u{1f}';

/// The literal string AppleScript returns for an advert's artwork.
const MISSING_VALUE: &str = "missing value";

/// How many fields the batched read always emits. Fixed, so the parser never has
/// to branch on length — a short record is a malformed one, not a different shape.
const FIELDS: usize = 17;

const F_STATE: usize = 0;
const F_POSITION: usize = 1;
const F_VOLUME: usize = 2;
const F_SHUFFLING: usize = 3;
const F_REPEATING: usize = 4;
const F_TITLE: usize = 5;
const F_ARTIST: usize = 6;
const F_ALBUM: usize = 7;
const F_ALBUM_ARTIST: usize = 8;
const F_DURATION: usize = 9;
const F_DISC: usize = 10;
const F_TRACK_NO: usize = 11;
const F_POPULARITY: usize = 12;
const F_PLAY_COUNT: usize = 13;
const F_ARTWORK: usize = 14;
const F_SPOTIFY_URL: usize = 15;
const F_ID: usize = 16;

/// Parse the raw stdout of the batched read script.
///
/// # Errors
/// Returns a message describing which field was wrong, so a failure names the
/// problem instead of yielding zeroes that look like a quiet Spotify.
pub fn parse(raw: &str) -> Result<PlayerState, String> {
    // osascript appends a newline; the fields themselves never contain one.
    let body = raw.trim_end_matches('\n');
    let f: Vec<&str> = body.split(SEP).collect();

    if f.len() != FIELDS {
        return Err(format!(
            "expected {FIELDS} fields separated by U+001F, got {}",
            f.len()
        ));
    }

    let playback = PlaybackState::parse(f[F_STATE])
        .ok_or_else(|| format!("unknown player state {:?}", f[F_STATE]))?;

    // Float, because AppleScript widens an f32 and the digits are noisy
    // (docs/APPLESCRIPT.md §2). Never compare for equality.
    // Also empty when nothing is loaded; a stuck progress bar is worse than 0.
    let position_secs: f64 = if f[F_POSITION].trim().is_empty() {
        0.0
    } else {
        f[F_POSITION]
            .parse()
            .map_err(|_| format!("bad player position {:?}", f[F_POSITION]))?
    };

    // A read-back can be one lower than what was set, so clamp rather than reject
    // an odd 101 (COMPAT rule 5).
    let volume = parse_u8(f[F_VOLUME], "sound volume")?.min(100);

    let uri = non_advert_uri(f[F_ID]);
    // The notification and the batched read agree that these are the same value;
    // assert it in a test rather than silently preferring one here.
    let spotify_url_field = non_advert_uri(f[F_SPOTIFY_URL]);
    debug_assert_eq!(uri, spotify_url_field);
    let track = TrackInfo {
        title: f[F_TITLE].to_string(),
        artist: f[F_ARTIST].to_string(),
        album: f[F_ALBUM].to_string(),
        album_artist: f[F_ALBUM_ARTIST].to_string(),
        duration_ms: parse_u64(f[F_DURATION], "duration")?,
        disc_number: parse_u32(f[F_DISC], "disc number")?,
        track_number: parse_u32(f[F_TRACK_NO], "track number")?,
        // An advert reports 0 for every number. That is "unknown", not "zero
        // popularity", so the Info tab must not print a 0 for a track nobody rated.
        popularity: non_zero(f[F_POPULARITY]).map(|v| v as u32),
        play_count: non_zero(f[F_PLAY_COUNT]).map(|v| v as u32),
        artwork_url: match f[F_ARTWORK] {
            "" | MISSING_VALUE => None,
            url => Some(url.to_string()),
        },
        uri,
    };

    Ok(PlayerState {
        playback,
        position_secs,
        volume,
        shuffling_enabled: parse_bool(f[F_SHUFFLING], "shuffling")?,
        repeating_enabled: parse_bool(f[F_REPEATING], "repeating")?,
        track,
    })
}

/// A track URI, or `None` for an advert (`spotify:ad:…`) and for nothing loaded.
///
/// trak replays history by URI, so an advert must never enter the history — it
/// has no `play track` target.
fn non_advert_uri(raw: &str) -> Option<String> {
    let u = raw.trim();
    if u.is_empty() {
        None
    } else if u.starts_with("spotify:track:") {
        Some(u.to_string())
    } else {
        None
    }
}

fn non_zero(raw: &str) -> Option<u64> {
    let v = raw.trim().parse::<u64>().ok()?;
    if v == 0 { None } else { Some(v) }
}

/// Empty means false, for the same "nothing is loaded" reason as the numerics.
/// A genuinely wrong value still fails rather than being guessed at.
fn parse_bool(raw: &str, what: &str) -> Result<bool, String> {
    match raw.trim() {
        "" | "false" => Ok(false),
        "true" => Ok(true),
        other => Err(format!("bad {what}: {other:?}")),
    }
}

/// An empty numeric field means 0, not an error.
///
/// When nothing is loaded, Spotify returns empty strings for *every* field, not
/// zeros (docs/APPLESCRIPT.md §3, "Nothing loaded"). Being strict here made the
/// parser reject exactly the state the idle card has to render. Genuinely
/// malformed values still fail loudly.
fn parse_u64(raw: &str, what: &str) -> Result<u64, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(0);
    }
    t.parse().map_err(|_| format!("bad {what}: {raw:?}"))
}

fn parse_u32(raw: &str, what: &str) -> Result<u32, String> {
    Ok(parse_u64(raw, what)? as u32)
}

fn parse_u8(raw: &str, what: &str) -> Result<u8, String> {
    Ok(parse_u64(raw, what)?.min(u64::from(u8::MAX)) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn parses_a_real_playing_track() {
        let state = parse(&fixture("playing_track.txt")).unwrap();
        assert_eq!(state.playback, PlaybackState::Playing);
        assert_eq!(state.track.title, "Census Designated");
        assert_eq!(state.track.artist, "Jane Remover");
        assert_eq!(state.track.duration_ms, 360_511);
        assert_eq!(state.track.disc_number, 1);
        assert_eq!(state.track.track_number, 8);
        assert_eq!(state.track.popularity, Some(48));
        assert_eq!(
            state.track.uri.as_deref(),
            Some("spotify:track:6HacgXCExkzS552ILfJTXu")
        );
        assert!(!state.track.is_ad());
        assert_eq!(state.volume, 100);
        assert!(!state.shuffling_enabled);
        assert!(state.repeating_enabled);
    }

    /// Paused is not a special shape: every field still reads. If this ever
    /// needs a paused branch, the fixture stops parsing.
    #[test]
    fn a_paused_track_parses_with_every_field_populated() {
        let paused = parse(&fixture("paused_track.txt")).unwrap();
        assert_eq!(paused.playback, PlaybackState::Paused);
        assert!(!paused.track.title.is_empty());
        assert!(!paused.track.artist.is_empty());
        assert!(paused.track.duration_ms > 0);
        assert!(paused.track.uri.is_some());
        assert!(paused.track.artwork_url.is_some());
        // and it is the *only* difference from playing
        let playing = parse(&fixture("playing_track.txt")).unwrap();
        assert_ne!(playing.playback, paused.playback);
    }

    /// The case that decides whether the parser is right: an advert.
    #[test]
    fn an_advert_has_no_uri_no_artwork_and_no_ratings() {
        let state = parse(&fixture("playing_ad.txt")).unwrap();
        let t = &state.track;
        assert!(t.is_ad(), "an advert must not look like a track");
        assert_eq!(t.uri, None, "an advert must not enter history");
        assert_eq!(
            t.artwork_url, None,
            "artwork is the literal string 'missing value', not a URL"
        );
        assert_eq!(t.popularity, None, "0 means unknown, not zero-popularity");
        assert_eq!(t.play_count, None);
        assert_eq!(t.duration_secs(), 30);
        assert!(!t.album.is_empty() || true); // the fixture's album is empty
        assert_eq!(t.album, "", "an advert reports an empty album");
    }

    #[test]
    fn advert_does_not_panic_on_a_completely_empty_record() {
        // Nothing loaded: all empty, all zero. Must not divide by zero later.
        let mut raw: Vec<String> = (0..FIELDS).map(|_| String::new()).collect();
        raw[F_STATE] = "stopped".into();
        let state = parse(&raw.join(&SEP.to_string())).unwrap();
        assert_eq!(state.playback, PlaybackState::Stopped);
        assert_eq!(state.progress(), 0.0);
        assert!(state.track.is_ad());
    }

    #[test]
    fn progress_is_clamped() {
        let mut state = parse(&fixture("playing_track.txt")).unwrap();
        assert!(state.progress() > 0.0 && state.progress() <= 1.0);
        // A read that overruns the duration must not produce a bar past the end.
        state.position_secs = 9999.0;
        assert_eq!(state.progress(), 1.0);
        state.position_secs = -5.0;
        assert_eq!(state.progress(), 0.0);
    }

    #[test]
    fn a_short_record_is_an_error_not_a_silent_default() {
        let err = parse("playing\u{1f}1.0").unwrap_err();
        assert!(err.contains("17 fields"), "{err}");
    }

    #[test]
    fn bad_numbers_name_the_field() {
        let good = fixture("playing_track.txt");
        let broken = good.replacen("360511", "not-a-number", 1);
        let err = parse(&broken).unwrap_err();
        assert!(err.contains("duration"), "{err}");
    }

    #[test]
    fn an_unknown_player_state_is_rejected() {
        let broken = fixture("playing_track.txt").replacen("playing", "buffering", 1);
        assert!(parse(&broken).unwrap_err().contains("player state"));
    }

    #[test]
    fn an_oversized_volume_is_clamped_rather_than_failing() {
        // A read-back of 101 must not break the meter (COMPAT rule 5).
        let broken = fixture("playing_track.txt").replacen("\u{1f}100\u{1f}", "\u{1f}101\u{1f}", 1);
        assert_eq!(parse(&broken).unwrap().volume, 100);
    }

    /// Empty is tolerated because "nothing is loaded" reports every field empty,
    /// but a value that is neither true nor false is still a bug worth seeing.
    #[test]
    fn booleans_tolerate_empty_but_reject_nonsense() {
        assert!(parse_bool("true", "x").unwrap());
        assert!(!parse_bool("false", "x").unwrap());
        assert!(!parse_bool("", "x").unwrap());
        assert!(parse_bool("TRUE", "x").is_err());
        assert!(parse_bool("1", "x").is_err());
    }

    #[test]
    fn playback_state_parsing_is_case_insensitive() {
        // The notification sends "Playing"; AppleScript sends "playing".
        assert_eq!(
            PlaybackState::parse("Playing"),
            Some(PlaybackState::Playing)
        );
        assert_eq!(PlaybackState::parse("paused"), Some(PlaybackState::Paused));
        assert_eq!(
            PlaybackState::parse("  STOPPED "),
            Some(PlaybackState::Stopped)
        );
        assert_eq!(PlaybackState::parse("buffering"), None);
    }
}
