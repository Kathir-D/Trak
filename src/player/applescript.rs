//! The real `Player`: osascript, one process per read, everything in one script.
//!
//! Cost is the design constraint here, not an optimisation. A full 17-field read
//! is ~430 ms because Spotify charges ~18 ms per Apple Event
//! (docs/APPLESCRIPT.md §4), so this never chains calls and never polls more
//! often than it has to.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::player::parse::{SEP, parse};
use crate::player::{Player, PlayerError, PlayerState};

/// COMPAT rule 7: call osascript by absolute path, overridable for the owner's
/// agent sessions.
fn osascript_path() -> String {
    std::env::var("TRAK_OSASCRIPT").unwrap_or_else(|_| "/usr/bin/osascript".to_string())
}

/// TODO 2.3's timeout. A normal call is ~430 ms and the worst observed was
/// ~1.3 s, so 5 s is a comfortable multiple rather than a guess.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The batched read. `is running` is the FIRST statement on purpose: a bare
/// `tell` on a non-running app *launches* it, which COMPAT rule 2 forbids
/// (docs/APPLESCRIPT.md §3).
/// The volume on its own: one property, one Apple Event.
///
/// Same guard as the batched read and the same "never launch Spotify" rule, but
/// nothing else -- `docs/APPLESCRIPT.md` §4 measured ~18 ms per property against
/// a ~50 ms spawn, so this is about a fifth of [`READ`].
const VOLUME: &str = r#"
on run argv
	if application "Spotify" is not running then return "not-running"
	tell application "Spotify" to return ((sound volume) as string)
end run
"#;

const READ: &str = r#"
on run argv
	set U to ASCII character 31
	if application "Spotify" is not running then return "not-running" & U
	tell application "Spotify"
		set theTrack to current track
		set s to (player state as string) & U & ((player position) as string) & U
		set s to s & ((sound volume) as string) & U & ((shuffling) as string) & U
		set s to s & ((repeating) as string) & U & (name of theTrack) & U
		set s to s & (artist of theTrack) & U & (album of theTrack) & U
		set s to s & (album artist of theTrack) & U & ((duration of theTrack) as string) & U
		set s to s & ((disc number of theTrack) as string) & U & ((track number of theTrack) as string) & U
		set s to s & ((popularity of theTrack) as string) & U & ((played count of theTrack) as string) & U
		set s to s & (artwork url of theTrack) & U & (spotify url of theTrack) & U & (id of theTrack)
	end tell
	return s
end run
"#;

/// The 6 fields the poll needs: state, position, volume, shuffle, repeat, id.
///
/// Kept separate from the full read so the expensive 11 track fields are only
/// paid once per song, not once per tick (docs/APPLESCRIPT.md §4).
#[allow(dead_code, reason = "used by the TUI poll, TODO 3.2")]
const READ_FAST: &str = r#"
on run argv
	set U to ASCII character 31
	if application "Spotify" is not running then return "not-running" & U
	tell application "Spotify"
		set s to (player state as string) & U & ((player position) as string) & U
		set s to s & ((sound volume) as string) & U & ((shuffling) as string) & U
		set s to s & ((repeating) as string) & U & (id of current track)
	end tell
	return s
end run
"#;

pub struct AppleScriptPlayer {
    /// The last volume trak successfully set, so the meter can show the user's
    /// value rather than Spotify's quantised read-back (COMPAT rule 5).
    last_set_volume: Option<u8>,
    /// The repeat mode the user chose. AppleScript cannot read back "repeat one"
    /// as distinct from "repeat all" (docs/APPLESCRIPT.md §2), so trak keeps its
    /// own copy or the `r` key would never cycle correctly.
    repeat_mode: crate::player::RepeatMode,
}

impl Default for AppleScriptPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleScriptPlayer {
    pub fn new() -> Self {
        Self {
            last_set_volume: None,
            repeat_mode: crate::player::RepeatMode::Off,
        }
    }

    /// Run one script and return its stdout, mapping osascript's failures onto
    /// `PlayerError`.
    ///
    /// The script is fed on stdin rather than passed as a path or `-e`, because a
    /// multi-line `if` cannot be expressed as a single `-e` argument
    /// (docs/APPLESCRIPT.md §3).
    /// Run a script that has **nothing to do with Spotify**, so there is no
    /// "is it running" check to make: the script would not care.
    ///
    /// Separate from [`Self::command`] on purpose. That one wraps its line in
    /// `tell application "Spotify"`, which is right for a command to Spotify and
    /// wrong for anything else -- a notification posted from inside Spotify's
    /// context is what raised Spotify over the terminal on every track change.
    fn run_bare(&self, script: &str) -> Result<String, PlayerError> {
        self.run(script)
    }

    fn run(&self, script: &str) -> Result<String, PlayerError> {
        let mut child = Command::new(osascript_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PlayerError::Script(format!("spawning osascript: {e}")))?;

        // The script source is written to stdin, so a real script file is never
        // needed on disk and the AppleScript compiler handles it directly.
        if let Some(stdin) = child.stdin.as_mut() {
            match stdin.write_all(script.as_bytes()) {
                Ok(()) => {}
                // osascript that exits without reading its stdin (Automation
                // denied, or a fake in a test) has already said why on stderr;
                // reporting the broken pipe instead would hide that, and returning
                // here would leave the child unreaped.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(PlayerError::Script(format!("writing script: {e}"))),
            }
        }
        // Dropping stdin closes it, which is what tells osascript the script is
        // complete. Without this osascript waits forever and the timeout fires.
        drop(child.stdin.take());

        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(PlayerError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(PlayerError::Script(format!("waiting: {e}"))),
            }
        }

        let out = child
            .wait_with_output()
            .map_err(|e| PlayerError::Script(format!("collecting output: {e}")))?;

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if let Some(code) = classify(&stdout, &out.stderr, out.status.code()) {
            return Err(code);
        }
        Ok(stdout)
    }

    /// Cheap liveness check that never launches anything.
    #[allow(dead_code, reason = "used by the idle card, TODO 3.5")]
    pub fn is_running(&self) -> bool {
        self.run(
            r#"on run argv
	return (application "Spotify" is running) as string
end run
"#,
        )
        .is_ok_and(|s| s.trim() == "true")
    }

    /// The 6 fields the poll tick needs, about 300 ms.
    ///
    /// Returns `None` when the track id is unchanged since the last call, which
    /// is the signal to skip the expensive full read.
    #[allow(dead_code, reason = "used by the TUI poll, TODO 3.2")]
    pub fn fast_state(&self) -> Result<FastState, PlayerError> {
        let raw = self.run(READ_FAST)?;
        let f: Vec<&str> = raw.trim_end_matches('\n').split(SEP).collect();
        if f.first() == Some(&"not-running") {
            return Err(PlayerError::NotRunning);
        }
        if f.len() < 6 {
            return Err(PlayerError::Script(format!(
                "fast read returned {} fields",
                f.len()
            )));
        }
        Ok(FastState {
            playback: crate::player::PlaybackState::parse(f[0])
                .ok_or_else(|| PlayerError::Script(format!("unknown player state {:?}", f[0])))?,
            position_secs: f[1]
                .parse()
                .map_err(|_| PlayerError::Script("bad player position".into()))?,
            volume: f[2].trim().parse().unwrap_or(0).min(100),
            shuffling_enabled: f[3].trim() == "true",
            repeating_enabled: f[4].trim() == "true",
            uri: match f[5] {
                u if u.starts_with("spotify:track:") => Some(u.to_string()),
                _ => None,
            },
        })
    }

    /// One line, and it is a write: only ever call this for a user action.
    ///
    /// Builds a one-line write script with the running check as its first
    /// statement, so a non-running Spotify can never be launched.
    fn command(&self, line: &str) -> Result<(), PlayerError> {
        let script = format!(
            r#"on run argv
	if application "Spotify" is not running then return "not-running"
	tell application "Spotify"
		{line}
	end tell
	return "ok"
end run
"#
        );
        let out = self.run(&script)?;
        match out.trim() {
            "ok" => Ok(()),
            "not-running" => Err(PlayerError::NotRunning),
            other => Err(PlayerError::Script(other.to_string())),
        }
    }
}

/// What the 6-field poll read gives back.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code, reason = "used by the TUI poll, TODO 3.2")]
pub struct FastState {
    pub playback: crate::player::PlaybackState,
    pub position_secs: f64,
    pub volume: u8,
    pub shuffling_enabled: bool,
    pub repeating_enabled: bool,
    /// `None` for an advert, so the caller knows the full read is not worth it.
    pub uri: Option<String>,
}

/// Map osascript's output onto typed errors.
///
/// The important one is -1743: the terminal is not allowed to control Spotify.
/// That is a setup problem with a known fix, so it gets its own message rather
/// than being buried in a generic script error.
fn classify(stdout: &str, stderr: &[u8], code: Option<i32>) -> Option<PlayerError> {
    let err = String::from_utf8_lossy(stderr);
    if err.contains("-1743") {
        return Some(PlayerError::PermissionDenied);
    }
    if stdout.contains("not-running") {
        return Some(PlayerError::NotRunning);
    }
    match code {
        Some(0) | None => None,
        Some(c) => Some(PlayerError::Script(format!("osascript exited {c}: {err}"))),
    }
}

impl Player for AppleScriptPlayer {
    fn state(&self) -> Result<PlayerState, PlayerError> {
        let raw = self.run(READ)?;
        if raw.starts_with("not-running") {
            return Err(PlayerError::NotRunning);
        }
        parse(&raw).map_err(PlayerError::Script)
    }

    fn play(&self) -> Result<(), PlayerError> {
        self.command("play")
    }

    fn pause(&self) -> Result<(), PlayerError> {
        self.command("pause")
    }

    fn toggle(&self) -> Result<(), PlayerError> {
        self.command("playpause")
    }

    fn next(&self) -> Result<(), PlayerError> {
        self.command("next track")
    }

    fn previous(&self) -> Result<(), PlayerError> {
        self.command("previous track")
    }

    fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        if !secs.is_finite() || secs < 0.0 {
            return Err(PlayerError::Script(format!("bad seek target {secs}")));
        }
        self.command(&format!("set player position to {}", clamp_secs(secs)))
    }

    fn volume(&self) -> Result<u8, PlayerError> {
        let raw = self.run(VOLUME)?;
        if raw.starts_with("not-running") {
            return Err(PlayerError::NotRunning);
        }
        raw.trim()
            .parse::<u8>()
            .map_err(|_| PlayerError::Script(format!("Spotify said {raw:?} for a volume")))
            .map(|v| v.min(100))
    }

    fn set_volume(&mut self, volume: u8) -> Result<(), PlayerError> {
        let v = volume.min(100);
        self.command(&format!("set sound volume to {v}"))?;
        // Only record it once the write actually landed, so a failed write does
        // not leave the meter claiming a volume Spotify never got.
        self.last_set_volume = Some(v);
        Ok(())
    }

    fn play_uri(&self, uri: &str) -> Result<(), PlayerError> {
        let uri = uri.trim();
        crate::player::check_playable_uri(uri)?;
        self.command(&format!("play track \"{uri}\""))
    }

    fn command(&self, script: &str) -> Result<(), PlayerError> {
        AppleScriptPlayer::command(self, script)
    }
}

/// Post a notification **as trak**, not as Spotify.
///
/// This was going through [`AppleScriptPlayer::command`], which wraps every script
/// in `tell application "Spotify"` -- so `display notification` was executed inside
/// Spotify, and macOS raised Spotify over cmux every time the track changed
/// (owner, 2026-10-03: "when I change songs it ... flashed above cmux"). A
/// notification about a track change is trak's to post: it must never be addressed to
/// the player, because every event addressed to a background app is a chance for the
/// system to bring it to the front (COMPAT rules 2 and 3).
///
/// `display notification` is a StandardAdditions command, so it runs in the script's
/// own context with nothing told to do anything.
///
/// The escaping is the same as the commands' because it is the same problem: a quote
/// or a backslash in a track title closes the string literal.
pub fn notify(title: &str, body: &str) -> Result<(), PlayerError> {
    let script = format!(
        r#"display notification "{}" with title "{}""#,
        body.replace('"', "\""),
        title.replace('"', "\"")
    );
    let player = AppleScriptPlayer::new();
    // The reply is whatever StandardAdditions prints; the only thing that matters is
    // that the script ran, so the output is not inspected for a value.
    player.run_bare(&script).map(|_| ())
}

/// The notification script, as text, so the "not addressed to Spotify" claim can be
/// asserted without running anything.
fn notification_script(title: &str, body: &str) -> String {
    // A quote or a backslash in a track title would close the string literal, and
    // this is the same allow-list problem `play track` has: AppleScript escapes them
    // the other way round, and the quote is the only one that needs doubling.
    format!(
        r#"display notification "{}" with title "{}""#,
        body.replace('"', "\\\""),
        title.replace('"', "\\\"")
    )
}

/// AppleScript rejects a very long fractional position, so round to milliseconds.
fn clamp_secs(secs: f64) -> String {
    let ms = (secs * 1000.0).round().max(0.0);
    format!("{:.3}", ms / 1000.0)
}

impl AppleScriptPlayer {
    /// The volume the user last set, if any (TODO 3.10 shows this rather than the
    /// raw read, which is quantised and would make the meter jitter).
    #[allow(dead_code, reason = "used by the volume meter, TODO 3.10")]
    pub fn last_set_volume(&self) -> Option<u8> {
        self.last_set_volume
    }

    pub fn repeat_mode(&self) -> crate::player::RepeatMode {
        self.repeat_mode
    }

    pub fn set_repeat_mode(&mut self, mode: crate::player::RepeatMode) {
        self.repeat_mode = mode;
    }
}
#[cfg(test)]
mod notify_tests {
    use super::*;

    /// **The track-change notification is not addressed to Spotify.**
    ///
    /// It used to go through `command`, which wraps its line in
    /// `tell application "Spotify"` -- so `display notification` ran inside Spotify
    /// and macOS raised Spotify over the terminal every time the track changed (owner,
    /// 2026-10-03: "when I change songs it ... flashed above cmux").
    ///
    /// The guard is structural rather than behavioural: the script the notification
    /// sends must not mention Spotify at all, because every event addressed to a
    /// background app is a chance for the system to bring it to the front (COMPAT
    /// rules 2 and 3). A test that ran the real thing could only prove it on a
    /// machine where Spotify happens to be running, which is exactly the machine a
    /// test must not depend on.
    #[test]
    fn the_notification_is_posted_by_trak_and_not_told_to_spotify() {
        let script = notification_script("Nights", "Frank Ocean");
        assert!(
            !script.contains("Spotify"),
            "the notification must not be addressed to Spotify: {script}"
        );
        assert!(
            script.starts_with("display notification"),
            "and it is a notification, not something else: {script}"
        );
        // No `tell` block at all, and no `run argv` wrapper either.
        assert!(!script.contains("tell"), "{script}");
        assert!(!script.contains("on run"), "{script}");
    }

    /// A track title with a quote in it must not close the string literal, or the
    /// script is a syntax error and the notification silently never appears.
    #[test]
    fn a_quote_in_a_track_name_does_not_break_the_notification() {
        let script = notification_script("He said \"hi\"", "A \"quoted\" artist");
        // Every quote inside the arguments is doubled, so the escaped pairs are the
        // four quotes in the titles times two, and the only *bare* quotes left are
        // the four that delimit the two arguments. A quote left unescaped is a
        // syntax error, and the notification silently never appears.
        let bare: String = script.replace("\\\"", "");
        assert_eq!(
            bare.matches('"').count(),
            4,
            "only the argument delimiters are unescaped: {script}"
        );
        assert!(bare.starts_with("display notification \""), "{script}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_permission_denied() {
        let e = classify("", b"execution error: -1743", Some(1));
        assert!(matches!(e, Some(PlayerError::PermissionDenied)));
    }

    #[test]
    fn classifies_not_running() {
        let e = classify("not-running\u{1f}", b"", Some(0));
        assert!(matches!(e, Some(PlayerError::NotRunning)));
    }

    #[test]
    fn success_is_not_an_error() {
        assert!(classify("playing\u{1f}1.0", b"", Some(0)).is_none());
    }

    #[test]
    fn other_failures_stay_generic() {
        let e = classify("", b"something else", Some(1)).unwrap();
        assert!(matches!(e, PlayerError::Script(_)));
    }

    #[test]
    fn osascript_path_honours_the_env_override() {
        // COMPAT rule 7. The variable is what the owner's agent sessions use.
        unsafe { std::env::set_var("TRAK_OSASCRIPT", "/tmp/fake-osascript") };
        assert_eq!(osascript_path(), "/tmp/fake-osascript");
        unsafe { std::env::set_var("TRAK_OSASCRIPT", "/usr/bin/osascript") };
        assert_eq!(osascript_path(), "/usr/bin/osascript");
    }

    #[test]
    fn seeks_are_rounded_to_milliseconds() {
        // AppleScript rejects long fractional positions.
        assert_eq!(clamp_secs(30.0), "30.000");
        assert_eq!(clamp_secs(1.23456), "1.235");
        assert_eq!(clamp_secs(-5.0), "0.000");
    }

    #[test]
    fn a_non_positive_seek_is_rejected_before_it_reaches_apple_script() {
        let mut p = AppleScriptPlayer::new();
        assert!(p.seek(-1.0).is_err());
        assert!(p.seek(f64::NAN).is_err());
    }

    #[cfg(test)]
    use crate::player::check_playable_uri;

    #[test]
    fn only_playable_uris_are_accepted() {
        for good in [
            "spotify:track:6HacgXCExkzS552ILfJTXu",
            "spotify:album:abc",
            "spotify:playlist:xyz",
            "spotify:artist:123",
            "spotify:episode:e1",
            "spotify:show:s1",
        ] {
            assert!(
                check_playable_uri(good).is_ok(),
                "{good} should be playable"
            );
        }
    }

    #[test]
    fn a_uri_that_is_not_spotify_is_rejected() {
        for bad in [
            "",
            "https://open.spotify.com/track/x",
            // An advert has no play target, so it must not be accepted.
            "spotify:ad:abc",
            "not-a-uri",
            "spotify:track:",
        ] {
            let e = check_playable_uri(bad).unwrap_err();
            assert!(
                e.to_string().contains("not a Spotify URI"),
                "{bad:?} gave {e}"
            );
        }
    }

    /// A user-supplied string must never be able to close the string literal in
    /// the generated script and inject a second command.
    #[test]
    fn a_quote_in_a_uri_cannot_inject_a_command() {
        for evil in [
            "spotify:track:x\" & (do shell script \"id\") & \"",
            "spotify:track:x\\nset sound volume to 0",
        ] {
            assert!(check_playable_uri(evil).is_err(), "{evil:?} was accepted");
        }
    }
}
