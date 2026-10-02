//! An in-memory `Player`, so the whole app runs and tests with no Spotify, no
//! network and no terminal (ARCHITECTURE principle 2).
//!
//! It is also the only place the *behaviour* trak depends on is pinned down, which
//! matters because the real Spotify has some sharp edges: it quantises volume by
//! one, and it ignores writes some of the time. `FakePlayer` can be told to do
//! either, so the read-back logic in `player/applescript.rs` and the volume meter
//! in the TUI are tested against those cases rather than against a hope.

use std::cell::RefCell;

use crate::player::{PlaybackState, Player, PlayerError, PlayerState, TrackInfo};

/// How the fake should misbehave, so the failure paths are testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Quirks {
    /// Quantise volume down by one, the way Spotify 1.3.x does
    /// (docs/APPLESCRIPT.md §5). Default: off, so the plain case is the default.
    pub quantise_volume: bool,
    /// Ignore volume writes entirely, which is what COMPAT rule 5 is written for.
    pub ignore_volume_writes: bool,
    /// Report `NotRunning` from every call, for the idle card.
    pub not_running: bool,
    /// Return `PermissionDenied` (-1743), for the permission message.
    pub denied: bool,
    /// Take longer than the client's timeout, for the timeout path.
    pub hang: bool,
}

pub struct FakePlayer {
    inner: RefCell<Inner>,
    quirks: Quirks,
    /// Every write that was attempted, in order. Lets a test assert that a poll
    /// did not write back (COMPAT rule 3).
    writes: RefCell<Vec<String>>,
}

#[derive(Debug, Clone)]
struct Inner {
    playback: PlaybackState,
    position_secs: f64,
    volume: u8,
    shuffling: bool,
    repeating: bool,
    track: TrackInfo,
    queue: Vec<TrackInfo>,
    /// A volume write that was ignored, kept so the read-back still shows the old
    /// value the way a real ignoring Spotify would.
    ignore_next_volume: Option<u8>,
}

// A test double's builders and accessors are its public API. Most are only
// reached from tests, so the release build sees them as unused.
#[allow(
    dead_code,
    reason = "test-double API: builders and accessors are used by tests, not by trak"
)]
impl FakePlayer {
    /// A fake with a track loaded and playing, which is the state most tests want.
    pub fn playing() -> Self {
        Self::new().with_track(sample_track())
    }

    pub fn new() -> Self {
        Self {
            inner: RefCell::new(Inner {
                playback: PlaybackState::Stopped,
                position_secs: 0.0,
                volume: 80,
                shuffling: false,
                repeating: false,
                track: TrackInfo::default(),
                queue: Vec::new(),
                ignore_next_volume: None,
            }),
            quirks: Quirks::default(),
            writes: RefCell::new(Vec::new()),
        }
    }

    pub fn with_quirks(quirks: Quirks) -> Self {
        let mut p = Self::new();
        p.quirks = quirks;
        if quirks.ignore_volume_writes {
            p.inner.borrow_mut().ignore_next_volume = Some(80);
        }
        p
    }

    pub fn quirks(&self) -> Quirks {
        self.quirks
    }

    /// Load a track and start playing it.
    pub fn with_track(self, track: TrackInfo) -> Self {
        {
            let mut i = self.inner.borrow_mut();
            i.track = track;
            i.playback = PlaybackState::Playing;
        }
        self
    }

    /// Queue more tracks, so `next` has somewhere to go.
    pub fn with_queue(self, tracks: Vec<TrackInfo>) -> Self {
        self.inner.borrow_mut().queue = tracks;
        self
    }

    pub fn set_playback(&self, playback: PlaybackState) {
        self.inner.borrow_mut().playback = playback;
    }

    pub fn set_position(&self, secs: f64) {
        self.inner.borrow_mut().position_secs = secs;
    }

    /// Every write attempted, so a test can prove a poll is read-only.
    pub fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    pub fn clear_writes(&self) {
        self.writes.borrow_mut().clear();
    }

    fn record(&self, what: &str) {
        self.writes.borrow_mut().push(what.to_string());
    }

    /// Fail the way a scripted fake should, or return nothing.
    fn gate(&self) -> Result<(), PlayerError> {
        if self.quirks.denied {
            return Err(PlayerError::PermissionDenied);
        }
        if self.quirks.not_running {
            return Err(PlayerError::NotRunning);
        }
        if self.quirks.hang {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        Ok(())
    }

    /// Read the volume the way Spotify reports it, including the quantisation.
    fn read_volume(&self, stored: u8) -> u8 {
        if self.quirks.quantise_volume {
            stored.saturating_sub(1)
        } else {
            stored
        }
    }
}

impl Default for FakePlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Player for FakePlayer {
    fn state(&self) -> Result<PlayerState, PlayerError> {
        self.gate()?;
        let i = self.inner.borrow();
        Ok(PlayerState {
            playback: i.playback,
            position_secs: i.position_secs,
            volume: self.read_volume(i.volume),
            shuffling_enabled: i.shuffling,
            repeating_enabled: i.repeating,
            track: i.track.clone(),
        })
    }

    fn play(&self) -> Result<(), PlayerError> {
        self.gate()?;
        self.record("play");
        self.inner.borrow_mut().playback = PlaybackState::Playing;
        Ok(())
    }

    fn pause(&self) -> Result<(), PlayerError> {
        self.gate()?;
        self.record("pause");
        self.inner.borrow_mut().playback = PlaybackState::Paused;
        Ok(())
    }

    fn toggle(&self) -> Result<(), PlayerError> {
        self.gate()?;
        self.record("toggle");
        let mut i = self.inner.borrow_mut();
        i.playback = match i.playback {
            PlaybackState::Playing => PlaybackState::Paused,
            _ => PlaybackState::Playing,
        };
        Ok(())
    }

    fn next(&self) -> Result<(), PlayerError> {
        self.gate()?;
        self.record("next");
        let mut i = self.inner.borrow_mut();
        if !i.queue.is_empty() {
            i.track = i.queue.remove(0);
            i.position_secs = 0.0;
        }
        i.playback = PlaybackState::Playing;
        Ok(())
    }

    fn previous(&self) -> Result<(), PlayerError> {
        self.gate()?;
        self.record("previous");
        let mut i = self.inner.borrow_mut();
        i.position_secs = 0.0;
        i.playback = PlaybackState::Playing;
        Ok(())
    }

    fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        self.gate()?;
        self.record(&format!("seek {secs}"));
        let mut i = self.inner.borrow_mut();
        // A real player clamps a seek past the end to the end.
        let dur = i.track.duration_secs() as f64;
        i.position_secs = if dur > 0.0 {
            secs.clamp(0.0, dur)
        } else {
            secs.max(0.0)
        };
        Ok(())
    }

    fn volume(&self) -> Result<u8, PlayerError> {
        self.gate()?;
        let i = self.inner.borrow();
        Ok(self.read_volume(i.volume))
    }

    fn set_volume(&mut self, volume: u8) -> Result<(), PlayerError> {
        self.gate()?;
        let v = volume.min(100);
        self.record(&format!("volume {v}"));
        let mut i = self.inner.borrow_mut();
        if self.quirks.ignore_volume_writes {
            // Keep the old value, which is what "ignored" looks like.
            i.ignore_next_volume = Some(v);
            return Ok(());
        }
        i.volume = v;
        Ok(())
    }

    /// The fake records the write and applies the two it knows about, so the
    /// TUI's shuffle and repeat bindings are testable.
    fn command(&self, script: &str) -> Result<(), PlayerError> {
        self.gate()?;
        self.record(script);
        let mut i = self.inner.borrow_mut();
        if let Some(v) = script
            .strip_prefix("set shuffling to ")
            .and_then(|v| v.trim().parse::<bool>().ok())
        {
            i.shuffling = v;
        } else if let Some(v) = script
            .strip_prefix("set repeating to ")
            .and_then(|v| v.trim().parse::<bool>().ok())
        {
            i.repeating = v;
        }
        Ok(())
    }

    fn play_uri(&self, uri: &str) -> Result<(), PlayerError> {
        self.gate()?;
        // The same allow-list the real player applies, so a test that passes here
        // is a test that will pass against Spotify. A fake that accepts anything
        // is worse than no fake: it lets a broken URI through to production.
        crate::player::check_playable_uri(uri)?;
        self.record(&format!("play_uri {uri}"));
        let mut i = self.inner.borrow_mut();
        if let Some(track) = i.queue.iter().find(|t| t.uri.as_deref() == Some(uri)) {
            i.track = track.clone();
        } else {
            i.track.uri = Some(uri.to_string());
        }
        i.position_secs = 0.0;
        i.playback = PlaybackState::Playing;
        Ok(())
    }
}

/// A track with every field set, for tests and the TUI's own previews.
pub fn sample_track() -> TrackInfo {
    TrackInfo {
        title: "Census Designated".into(),
        artist: "Jane Remover".into(),
        album: "Census Designated".into(),
        album_artist: "Jane Remover".into(),
        duration_ms: 360_511,
        disc_number: 1,
        track_number: 8,
        popularity: Some(48),
        play_count: Some(0),
        artwork_url: Some(
            "https://i.scdn.co/image/ab67616d0000b2738a821784ac3e69e691d4945f".into(),
        ),
        uri: Some("spotify:track:6HacgXCExkzS552ILfJTXu".into()),
    }
}

/// The volume read-back check COMPAT rule 5 requires, tolerance included.
///
/// Spotify quantises, so a read-back of exactly one less than the value written is
/// a **success** and a read-back at the previous value is a **failure**. Getting
/// this wrong makes trak hide the volume meter after every single keypress
/// (`docs/KEYCHAIN`-style one-line notes aside, see `docs/APPLESCRIPT.md` §5).
pub fn volume_write_landed(want: u8, read: u8) -> bool {
    want.abs_diff(read) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_fake_is_stopped_with_nothing_loaded() {
        let p = FakePlayer::new();
        let s = p.state().unwrap();
        assert_eq!(s.playback, PlaybackState::Stopped);
        assert!(s.track.title.is_empty());
        assert!(s.track.is_ad());
    }

    #[test]
    fn the_playing_preset_has_a_real_track() {
        let s = FakePlayer::playing().state().unwrap();
        assert_eq!(s.playback, PlaybackState::Playing);
        assert_eq!(s.track.title, "Census Designated");
        assert!(!s.track.is_ad());
    }

    #[test]
    fn toggle_flips_play_and_pause() {
        let p = FakePlayer::playing();
        p.toggle().unwrap();
        assert_eq!(p.state().unwrap().playback, PlaybackState::Paused);
        p.toggle().unwrap();
        assert_eq!(p.state().unwrap().playback, PlaybackState::Playing);
    }

    #[test]
    fn next_moves_through_the_queue_and_resets_position() {
        let a = TrackInfo {
            title: "One".into(),
            uri: Some("spotify:track:1".into()),
            duration_ms: 100_000,
            ..sample_track()
        };
        let b = TrackInfo {
            title: "Two".into(),
            uri: Some("spotify:track:2".into()),
            duration_ms: 100_000,
            ..sample_track()
        };
        let p = FakePlayer::playing().with_queue(vec![a, b]);
        p.set_position(50.0);
        p.next().unwrap();
        assert_eq!(p.state().unwrap().track.title, "One");
        assert_eq!(p.state().unwrap().position_secs, 0.0);
        p.next().unwrap();
        assert_eq!(p.state().unwrap().track.title, "Two");
    }

    #[test]
    fn a_seek_past_the_end_clamps_like_a_real_player() {
        let mut p = FakePlayer::playing();
        p.seek(9999.0).unwrap();
        assert_eq!(p.state().unwrap().position_secs, 360.0);
        p.seek(-5.0).unwrap();
        assert_eq!(p.state().unwrap().position_secs, 0.0);
    }

    /// The bug this exists to prevent: reading a write back and calling it a
    /// failure because Spotify quantised it.
    #[test]
    fn volume_read_back_tolerates_the_quantisation() {
        assert!(volume_write_landed(70, 69), "one low is still landed");
        assert!(volume_write_landed(70, 70), "exact is landed");
        assert!(volume_write_landed(70, 71), "one high is landed");
        assert!(!volume_write_landed(70, 100), "unchanged means ignored");
        assert!(!volume_write_landed(70, 40), "a long way off means ignored");
    }

    #[test]
    fn a_quantising_fake_reports_one_lower() {
        let mut p = FakePlayer::with_quirks(Quirks {
            quantise_volume: true,
            ..Quirks::default()
        });
        p.set_volume(70).unwrap();
        let read = p.state().unwrap().volume;
        assert_eq!(read, 69);
        assert!(
            volume_write_landed(70, read),
            "the read-back must still count as landed"
        );
    }

    #[test]
    fn a_fake_that_ignores_volume_reports_the_old_value() {
        let mut p = FakePlayer::with_quirks(Quirks {
            ignore_volume_writes: true,
            ..Quirks::default()
        });
        let before = p.state().unwrap().volume;
        p.set_volume(10).unwrap();
        let after = p.state().unwrap().volume;
        assert_eq!(after, before, "an ignored write changes nothing");
        assert!(
            !volume_write_landed(10, after),
            "and the read-back must report it as not landed"
        );
    }

    /// COMPAT rule 3: a read must never write.
    #[test]
    fn reading_records_no_writes() {
        let p = FakePlayer::playing();
        for _ in 0..10 {
            p.state().unwrap();
        }
        assert!(p.writes().is_empty(), "reads wrote: {:?}", p.writes());
    }

    #[test]
    fn every_action_is_recorded_in_order() {
        let p = FakePlayer::playing();
        p.toggle().unwrap();
        p.next().unwrap();
        p.previous().unwrap();
        assert_eq!(p.writes(), vec!["toggle", "next", "previous"]);
        p.clear_writes();
        assert!(p.writes().is_empty());
    }

    #[test]
    fn the_not_running_quirk_fails_every_call() {
        let p = FakePlayer::with_quirks(Quirks {
            not_running: true,
            ..Quirks::default()
        });
        assert!(matches!(p.state(), Err(PlayerError::NotRunning)));
        assert!(matches!(p.toggle(), Err(PlayerError::NotRunning)));
        assert!(
            p.writes().is_empty(),
            "a not-running player records nothing"
        );
    }

    #[test]
    fn the_denied_quirk_reports_permission_denied() {
        let p = FakePlayer::with_quirks(Quirks {
            denied: true,
            ..Quirks::default()
        });
        assert!(matches!(p.state(), Err(PlayerError::PermissionDenied)));
    }

    #[test]
    fn play_uri_switches_track_and_starts_playing() {
        let p = FakePlayer::playing();
        p.play_uri("spotify:track:NEWONE").unwrap();
        let s = p.state().unwrap();
        assert_eq!(s.track.uri.as_deref(), Some("spotify:track:NEWONE"));
        assert_eq!(s.playback, PlaybackState::Playing);
        assert_eq!(s.position_secs, 0.0);
    }

    #[test]
    fn volume_is_clamped_to_the_valid_range() {
        let mut p = FakePlayer::new();
        p.set_volume(200).unwrap();
        assert_eq!(p.state().unwrap().volume, 100);
        p.set_volume(0).unwrap();
        assert_eq!(p.state().unwrap().volume, 0);
    }
}
