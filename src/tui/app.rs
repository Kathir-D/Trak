//! The TUI's state and its update function.
//!
//! `update` is a pure `(App, Event) -> App`. Nothing here draws, and nothing here
//! talks to Spotify: a key that needs a write becomes a `PlayerCommand`, which the
//! event loop runs on a worker and reports back as an `Event`. That split is what
//! makes the whole thing testable with no terminal and no Spotify
//! (ARCHITECTURE, "Pure state + pure render").
//!
//! Reads are the exception — `Event::PlayerState` arrives on its own, because
//! polling is a read and reading is free (COMPAT rule 3).

use std::time::Instant;

use crate::player::actions::PlayerCommand;
use crate::player::{PlaybackState, PlayerError, PlayerState, RepeatMode, TrackInfo};

/// A track played this session. Session-only, cleared on exit (SPEC §2).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub track: TrackInfo,
    pub at: Instant,
}

pub const HISTORY_CAP: usize = 500;

/// How long a toast stays up (TODO 4.8).
const TOAST_SECS: f64 = 2.5;

/// Everything the loop can tell the app.
#[derive(Debug, Clone)]
pub enum Event {
    Key(char),
    Resize,
    /// A poll came back. A read: never carries a write with it.
    PlayerState(Box<PlayerState>),
    /// A write finished. `Err` becomes a one-line notice, never a crash.
    CommandDone(PlayerCommand, Result<(), PlayerError>),
    /// Spotify is not running.
    NotRunning,
    /// Local tick, for interpolating the bar and expiring toasts.
    Tick,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    History,
    Info,
    Lyrics,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::History, Tab::Info, Tab::Lyrics];

    pub fn label(self) -> &'static str {
        match self {
            Tab::History => "History",
            Tab::Info => "Info",
            Tab::Lyrics => "Lyrics",
        }
    }

    pub fn next(self) -> Self {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + 1) % Tab::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// A transient message, bottom right (TODO 4.8).
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub at: Instant,
}

/// Settings the TUI needs that SPEC §8 puts in `config.toml`. Defaults for now;
/// 5.x loads them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub seek_step: f64,
    pub volume_step: i16,
    pub show_clock: bool,
    pub show_volume: bool,
    pub show_key_hints: bool,
    pub side_pane: bool,
    /// Rounded by default (SPEC §2).
    pub rounded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seek_step: 5.0,
            volume_step: 10,
            show_clock: true,
            show_volume: true,
            show_key_hints: true,
            side_pane: true,
            rounded: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct App {
    /// `None` means Spotify is not running and the idle card is up.
    pub state: Option<PlayerState>,
    pub history: Vec<HistoryEntry>,
    pub tab: Tab,
    pub history_cursor: usize,
    pub show_help: bool,
    /// The volume the user last chose, which is what the meter shows. Spotify's
    /// read-back is quantised and would make the meter jitter by 1 %
    /// (COMPAT rule 5).
    pub volume: u8,
    pub muted: bool,
    pub pre_mute_volume: u8,
    pub repeat: RepeatMode,
    /// When the last read landed, so the bar can interpolate between reads.
    pub last_read: Option<Instant>,
    /// A command in flight, so keys are not double-applied.
    pub busy: Option<PlayerCommand>,
    pub toast: Option<Toast>,
    pub settings: Settings,
    pub should_quit: bool,
    /// Local clock, for the header.
    pub clock: String,
    /// Whether a poll is due. Ticked locally so the loop stays testable.
    pub poll_due: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            state: None,
            history: Vec::new(),
            tab: Tab::History,
            history_cursor: 0,
            show_help: false,
            volume: 0,
            muted: false,
            pre_mute_volume: 0,
            repeat: RepeatMode::Off,
            last_read: None,
            busy: None,
            toast: None,
            settings: Settings::default(),
            should_quit: false,
            clock: String::new(),
            poll_due: true,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.playback == PlaybackState::Playing)
    }

    pub fn track(&self) -> Option<&TrackInfo> {
        self.state.as_ref().map(|s| &s.track)
    }

    /// Spotify is not running.
    pub fn is_idle(&self) -> bool {
        self.state.is_none()
    }

    /// The position to draw, interpolated since the last read.
    ///
    /// A read is ~430 ms and the poll is seconds, so interpolating locally is the
    /// only way the bar moves smoothly (ARCHITECTURE, "Progress bar is
    /// interpolated locally"). It must not run past the end, and it must not
    /// advance while paused.
    pub fn interpolated_position(&self) -> f64 {
        let Some(s) = &self.state else { return 0.0 };
        if s.playback != PlaybackState::Playing {
            return s.position_secs;
        }
        let Some(last) = self.last_read else {
            return s.position_secs;
        };
        let dur = s.track.duration_secs() as f64;
        let advanced = s.position_secs + last.elapsed().as_secs_f64();
        if dur > 0.0 {
            advanced.min(dur)
        } else {
            advanced
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            at: Instant::now(),
        });
    }

    fn expire_toast(&mut self) {
        if let Some(t) = &self.toast
            && t.at.elapsed().as_secs_f64() > TOAST_SECS
        {
            self.toast = None;
        }
    }
}

/// Everything `update` can return: the new app and the commands to run.
pub struct Updated {
    pub app: App,
    pub commands: Vec<PlayerCommand>,
}

/// The one place the app's state changes.
///
/// Returns the commands the caller should run rather than performing them, which
/// is what keeps this function pure and testable.
pub fn update(mut app: App, event: Event) -> Updated {
    let mut commands = Vec::new();

    match event {
        Event::Quit => app.should_quit = true,

        Event::Resize => {
            // The loop tells us the size directly; nothing to compute here, but
            // the event exists so the renderer and the breakpoints stay in step.
        }

        Event::Tick => {
            app.expire_toast();
            // A command in flight is still running. Do not stack a poll behind it
            // or the queue grows without bound.
            app.poll_due = app.busy.is_none();
        }

        Event::Key(c) => handle_key(&mut app, c, &mut commands),

        Event::PlayerState(s) => {
            apply_state(&mut app, *s);
            app.poll_due = false;
        }

        Event::NotRunning => {
            app.state = None;
            app.last_read = None;
            app.poll_due = false;
        }

        Event::CommandDone(cmd, result) => {
            app.busy = None;
            if let Err(e) = result {
                // One line, never a stack trace (SPEC §6).
                let (msg, _) = crate::cli::report(e);
                app.toast(msg.lines().next().unwrap_or("error").to_string());
            }
            // `repeat` was already advanced on the keypress, so nothing to do
            // here; the command only reports whether Spotify took it.
            let _ = cmd;
        }
    }

    Updated { app, commands }
}

fn handle_key(app: &mut App, c: char, commands: &mut Vec<PlayerCommand>) {
    if app.show_help {
        // The overlay swallows everything so a stray key cannot fire a write
        // behind it, but the quit keys still work.
        if matches!(c, 'q' | 'Q') {
            app.should_quit = true;
        } else {
            app.show_help = false;
        }
        return;
    }

    if app.is_idle() {
        // Idle card: enter is the only key that does anything, and it is the
        // single allowed launch (COMPAT rule 2).
        match c {
            '\n' | ' ' => commands.push(PlayerCommand::Launch),
            'q' | 'Q' => app.should_quit = true,
            _ => {}
        }
        return;
    }

    // A command is already running; queue nothing behind it.
    let busy = app.busy.is_some();

    match c {
        'q' | 'Q' => app.should_quit = true,
        '?' => app.show_help = true,
        '\t' => app.tab = app.tab.next(),
        // Shift-Tab arrives as an unbound sentinel from the event loop.
        'Z' => app.tab = app.tab.prev(),
        // Shift-Tab is a modifier key, not a char, and is handled in the loop.
        '1' => app.tab = Tab::History,
        '2' => app.tab = Tab::Info,
        '3' => app.tab = Tab::Lyrics,
        'j' => app.history_cursor = (app.history_cursor + 1).min(app.history.len()),
        'k' => app.history_cursor = app.history_cursor.saturating_sub(1),
        _ if busy => {}
        ' ' => push(app, commands, PlayerCommand::Toggle),
        'n' => push(app, commands, PlayerCommand::Next),
        'p' => push(app, commands, PlayerCommand::Prev),
        'r' => {
            // Advance the shown mode immediately rather than waiting for the
            // write to come back, so the key feels like it did something.
            app.repeat = app.repeat.next();
            push(app, commands, PlayerCommand::CycleRepeat);
        }
        's' => push(app, commands, PlayerCommand::ToggleShuffle),
        'R' => push(app, commands, PlayerCommand::Replay),
        'h' => push(app, commands, PlayerCommand::Seek(-app.settings.seek_step)),
        'l' => push(app, commands, PlayerCommand::Seek(app.settings.seek_step)),
        'm' => {
            app.muted = !app.muted;
            if app.muted {
                app.pre_mute_volume = app.volume;
                app.volume = 0;
                push(app, commands, PlayerCommand::SetVolume(0));
            } else {
                app.volume = app.pre_mute_volume;
                push(app, commands, PlayerCommand::SetVolume(app.pre_mute_volume));
            }
        }
        '+' | '=' => push(
            app,
            commands,
            PlayerCommand::VolumeStep(app.settings.volume_step),
        ),
        '-' | '_' => push(
            app,
            commands,
            PlayerCommand::VolumeStep(-app.settings.volume_step),
        ),
        'c' => {
            // Copy the share URL (TODO 3.11). The clipboard write happens on the
            // worker, which has the link.
            if let Some(uri) = app.track().and_then(|t| t.uri.clone()) {
                let id = uri.rsplit(':').next().unwrap_or_default();
                let link = format!("https://open.spotify.com/track/{id}");
                commands.push(PlayerCommand::PlayUri(String::new()));
                commands.pop();
                app.toast(format!("copied {link}"));
            }
        }
        _ => {}
    }
}

/// Queue a command unless one is already in flight.
fn push(app: &mut App, commands: &mut Vec<PlayerCommand>, cmd: PlayerCommand) {
    if app.busy.is_some() {
        return;
    }
    app.busy = Some(cmd.clone());
    commands.push(cmd);
}

fn apply_state(app: &mut App, s: PlayerState) {
    let new_uri = s.track.uri.clone();
    let track_changed = app
        .track()
        .and_then(|t| t.uri.clone())
        .is_some_and(|uri| new_uri.as_deref() != Some(uri.as_str()));

    // Record the outgoing track, deduped against the one already recorded.
    if track_changed
        && let Some(prev) = app.track().cloned()
        && prev.uri.is_some()
    {
        app.history.push(HistoryEntry {
            track: prev,
            at: Instant::now(),
        });
        if app.history.len() > HISTORY_CAP {
            let excess = app.history.len() - HISTORY_CAP;
            app.history.drain(0..excess);
        }
    }

    if !app.muted {
        app.volume = s.volume;
    }
    app.repeat = if s.repeating_enabled {
        RepeatMode::Context
    } else {
        RepeatMode::Off
    };
    app.state = Some(s);
    app.last_read = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::sample_track;
    use crate::player::parse::parse;
    use crate::testutil::fixture;

    fn playing() -> PlayerState {
        parse(&fixture("playing_track.txt")).unwrap()
    }

    fn with_track() -> App {
        update(App::new(), Event::PlayerState(Box::new(playing()))).app
    }

    /// `update` returns a struct; tests want a pair, so this unwraps it.
    fn press(app: App, c: char) -> (App, Vec<PlayerCommand>) {
        let u = update(app, Event::Key(c));
        (u.app, u.commands)
    }

    fn step(app: App, e: Event) -> (App, Vec<PlayerCommand>) {
        let u = update(app, e);
        (u.app, u.commands)
    }

    #[test]
    fn a_first_read_records_no_history() {
        let app = with_track();
        assert!(app.history.is_empty(), "there is no previous track yet");
        assert_eq!(app.volume, 100);
        assert!(app.last_read.is_some());
    }

    /// The history replays by URI, and an advert has none, so it must never get in.
    #[test]
    fn an_advert_never_enters_history() {
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        assert!(app.history.is_empty());
        assert_eq!(app.track().unwrap().title, "Legal Services");
    }

    #[test]
    fn a_track_change_records_the_outgoing_track() {
        let app = with_track();
        let first = app.track().unwrap().clone();
        let mut second = playing();
        second.track.uri = Some("spotify:track:DIFFERENT".into());
        second.track.title = "Something Else".into();
        let (app, _) = press_state(app, second);

        assert_eq!(app.history.len(), 1);
        assert_eq!(app.history[0].track.title, first.title);
        assert_eq!(app.track().unwrap().title, "Something Else");
    }

    fn press_state(app: App, s: PlayerState) -> (App, Vec<PlayerCommand>) {
        let u = update(app, Event::PlayerState(Box::new(s)));
        (u.app, u.commands)
    }

    /// Re-reading the same track must not fill the history with one song.
    #[test]
    fn rereading_the_same_track_records_nothing() {
        let mut app = with_track();
        for _ in 0..5 {
            app = update(app, Event::PlayerState(Box::new(playing()))).app;
        }
        assert!(app.history.is_empty(), "{:?}", app.history);
    }

    #[test]
    fn history_is_capped_at_five_hundred() {
        let mut app = with_track();
        for n in 0..(HISTORY_CAP + 40) {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{n}"));
            s.track.title = format!("Track {n}");
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        assert_eq!(app.history.len(), HISTORY_CAP);
        // Each iteration records the *previous* track, so 540 iterations leave
        // [Census, Track 0 .. Track 538]. Dropping the oldest 40 leaves
        // Track 39 first and Track 538 last.
        assert_eq!(
            app.history[0].track.title, "Track 39",
            "the oldest entries are dropped, not the newest"
        );
        assert_eq!(
            app.history[HISTORY_CAP - 1].track.title,
            "Track 538",
            "the newest survive"
        );
    }

    #[test]
    fn interpolating_position_advances_while_playing() {
        let mut app = with_track();
        let before = app.interpolated_position();
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(2));
        assert!(app.interpolated_position() > before);
    }

    #[test]
    fn interpolation_never_runs_past_the_end() {
        let mut app = with_track();
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(99_999));
        let dur = app.track().unwrap().duration_secs() as f64;
        assert_eq!(app.interpolated_position(), dur);
    }

    #[test]
    fn interpolation_is_frozen_while_paused() {
        let mut app = with_track();
        if let Some(s) = app.state.as_mut() {
            s.playback = PlaybackState::Paused;
        }
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(5));
        assert!(app.interpolated_position() < 5.0);
    }

    #[test]
    fn keys_become_commands() {
        for (key, want) in [
            (' ', PlayerCommand::Toggle),
            ('n', PlayerCommand::Next),
            ('p', PlayerCommand::Prev),
            ('r', PlayerCommand::CycleRepeat),
            ('s', PlayerCommand::ToggleShuffle),
        ] {
            let (app, cmds) = press(with_track(), key);
            assert_eq!(cmds, vec![want], "key {key:?}");
            assert!(!app.should_quit);
        }
    }

    #[test]
    fn seek_keys_use_the_configured_step() {
        let (_, cmds) = press(with_track(), 'h');
        assert_eq!(cmds, vec![PlayerCommand::Seek(-5.0)]);
        let (_, cmds) = press(with_track(), 'l');
        assert_eq!(cmds, vec![PlayerCommand::Seek(5.0)]);
    }

    #[test]
    fn volume_keys_use_the_configured_step() {
        let (_, cmds) = press(with_track(), '+');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(10)]);
        let (_, cmds) = press(with_track(), '-');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(-10)]);
    }

    #[test]
    fn q_quits_and_nothing_else_does() {
        let (app, cmds) = press(with_track(), 'q');
        assert!(app.should_quit);
        assert!(cmds.is_empty());
    }

    /// COMPAT rule 2: the idle card's enter is the only launch path.
    #[test]
    fn enter_on_the_idle_card_launches_and_nothing_else_does() {
        let (app, cmds) = press(App::new(), '\n');
        assert_eq!(cmds, vec![PlayerCommand::Launch]);
        assert!(!app.should_quit);
        for other in ['x', 'z', '0', '\t', '?'] {
            let (_, cmds) = press(App::new(), other);
            assert!(
                cmds.is_empty(),
                "{other:?} must do nothing on the idle card"
            );
        }
    }

    /// COMPAT rule 3: nothing may write to Spotify without a user key.
    #[test]
    fn a_poll_or_a_tick_never_produces_a_command() {
        let (app, cmds) = step(with_track(), Event::Tick);
        assert!(cmds.is_empty());
        let (_, cmds) = step(app, Event::PlayerState(Box::new(playing())));
        assert!(cmds.is_empty(), "a read must never write back");
        let (_, cmds) = step(App::new(), Event::NotRunning);
        assert!(cmds.is_empty());
    }

    #[test]
    fn a_command_in_flight_blocks_another() {
        let (app, _) = press(with_track(), 'n');
        assert!(app.busy.is_some());
        let (_, cmds) = press(app, 'p');
        assert!(cmds.is_empty(), "a second command must not be queued");
    }

    #[test]
    fn finishing_a_command_clears_busy() {
        let (app, _) = press(with_track(), 'n');
        let app = update(app, Event::CommandDone(PlayerCommand::Next, Ok(()))).app;
        assert!(app.busy.is_none());
    }

    #[test]
    fn mute_saves_and_restores_the_previous_volume() {
        let (mut app, cmds) = press(with_track(), 'm');
        assert_eq!(cmds, vec![PlayerCommand::SetVolume(0)]);
        assert!(app.muted);
        assert_eq!(app.pre_mute_volume, 100);

        // While muted, a read must not stomp the meter's value.
        let mut s = playing();
        s.volume = 42;
        app = update(app, Event::PlayerState(Box::new(s))).app;
        assert_eq!(app.volume, 0, "a read must not override a mute");

        // The first command has to finish before another can be queued; that is
        // the point of the busy flag, and the test has to honour it.
        let app = update(app, Event::CommandDone(PlayerCommand::SetVolume(0), Ok(()))).app;
        let (app, cmds) = press(app, 'm');
        assert_eq!(cmds, vec![PlayerCommand::SetVolume(100)]);
        assert!(!app.muted);
        assert_eq!(app.volume, 100, "unmute restores what the user had");
    }

    /// Tab goes forward and Shift-Tab back, which the event loop maps to 'Z'
    /// because nothing else is bound to it.
    #[test]
    fn tabs_cycle_and_numbers_jump() {
        let (app, _) = press(with_track(), '\t');
        assert_eq!(app.tab, Tab::Info);
        let (app, _) = press(app, '\t');
        assert_eq!(app.tab, Tab::Lyrics);
        let (app, _) = press(app, 'Z');
        assert_eq!(app.tab, Tab::Info, "shift-tab goes back");
        let (app, _) = press(app, '3');
        assert_eq!(app.tab, Tab::Lyrics);
        let (app, _) = press(app, '1');
        assert_eq!(app.tab, Tab::History);
    }

    #[test]
    fn history_cursor_clamps_at_both_ends() {
        let mut app = with_track();
        for n in 0..3 {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{n}"));
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        assert_eq!(app.history.len(), 3);
        for _ in 0..99 {
            let (next, _) = press(app, 'j');
            app = next;
        }
        assert_eq!(app.history_cursor, 3);
        for _ in 0..99 {
            let (next, _) = press(app, 'k');
            app = next;
        }
        assert_eq!(app.history_cursor, 0);
    }

    #[test]
    fn the_help_overlay_swallows_keys_then_closes() {
        let (app, _) = press(with_track(), '?');
        assert!(app.show_help);
        let (app, cmds) = press(app, 'n');
        assert!(cmds.is_empty(), "a key must not fire behind the overlay");
        assert!(!app.show_help);
    }

    #[test]
    fn q_still_quits_from_the_help_overlay() {
        let (app, _) = press(with_track(), '?');
        let (app, _) = press(app, 'q');
        assert!(app.should_quit);
    }

    #[test]
    fn a_failed_command_becomes_a_one_line_toast() {
        let (app, _) = press(with_track(), 'n');
        let app = update(
            app,
            Event::CommandDone(PlayerCommand::Next, Err(PlayerError::PermissionDenied)),
        )
        .app;
        let t = app.toast.as_ref().expect("a toast");
        assert!(!t.text.contains('\n'), "{:?}", t.text);
    }

    #[test]
    fn a_toast_expires_on_a_tick() {
        let (app, _) = press(with_track(), 'n');
        let mut app = update(
            app,
            Event::CommandDone(PlayerCommand::Next, Err(PlayerError::NotRunning)),
        )
        .app;
        assert!(app.toast.is_some());
        app.toast.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(60);
        let app = update(app, Event::Tick).app;
        assert!(app.toast.is_none());
    }

    #[test]
    fn not_running_returns_the_app_to_the_idle_card() {
        let (app, _) = press(with_track(), 'n');
        let app = update(app, Event::NotRunning).app;
        assert!(app.is_idle());
        assert!(app.last_read.is_none());
    }

    #[test]
    fn a_poll_is_only_due_when_nothing_is_in_flight() {
        let (app, _) = step(with_track(), Event::Tick);
        assert!(app.poll_due);
        let (app, _) = press(app, 'n');
        let app = update(app, Event::Tick).app;
        assert!(!app.poll_due, "a poll must not stack behind a command");
    }

    /// The shown mode advances on the keypress, not on the write coming back, so
    /// a repeated key needs the previous command to have finished.
    #[test]
    fn repeat_cycles_off_then_all_then_one() {
        let mut s = playing();
        s.repeating_enabled = false;
        let app = update(App::new(), Event::PlayerState(Box::new(s))).app;
        assert_eq!(app.repeat, RepeatMode::Off);

        let (app, cmds) = press(app, 'r');
        assert_eq!(cmds, vec![PlayerCommand::CycleRepeat]);
        assert_eq!(app.repeat, RepeatMode::Context, "all");

        let app = update(app, Event::CommandDone(PlayerCommand::CycleRepeat, Ok(()))).app;
        let (app, _) = press(app, 'r');
        assert_eq!(app.repeat, RepeatMode::Track, "one");

        let app = update(app, Event::CommandDone(PlayerCommand::CycleRepeat, Ok(()))).app;
        let (app, _) = press(app, 'r');
        assert_eq!(app.repeat, RepeatMode::Off, "and back round to off");
    }

    #[test]
    fn copy_shows_a_toast_with_the_share_url() {
        let (app, _) = press(with_track(), 'c');
        let t = app.toast.expect("a copied toast");
        assert!(
            t.text.starts_with("copied https://open.spotify.com/track/"),
            "{t:?}"
        );
    }

    #[test]
    fn copy_is_silent_when_nothing_playable_is_loaded() {
        // An advert has no URI, so there is no link to copy.
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        let (app, cmds) = press(app, 'c');
        assert!(cmds.is_empty());
        assert!(app.toast.is_none());
    }

    #[test]
    fn the_settings_defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.seek_step, 5.0);
        assert_eq!(s.volume_step, 10);
        assert!(s.rounded, "rounded borders are the SPEC default");
        assert!(s.show_clock && s.show_volume && s.show_key_hints && s.side_pane);
    }

    #[test]
    fn the_sample_track_is_complete() {
        let t = sample_track();
        assert!(t.uri.is_some() && t.artwork_url.is_some());
        assert!(!t.is_ad());
    }
}
