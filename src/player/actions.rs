//! User-initiated writes to Spotify, each with a read-back.
//!
//! Two rules shape everything here.
//!
//! **Only ever in response to a user action** (COMPAT rule 3). Nothing in this
//! module may be called from a poll or a retry; the callers are the key handler
//! and the CLI, and nothing else. That is why every function takes the action
//! explicitly rather than sitting behind a queue.
//!
//! **Read back and compare with a tolerance of one** (COMPAT rule 5). Spotify
//! quantises, so a read-back of exactly one less than the value written is a
//! *success*. Without the tolerance trak hides the volume meter after every
//! keypress, which is the bug this module exists to make impossible.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use crate::player::{Player, PlayerError, volume_write_landed};

/// A write that has been issued, and what reading it back said.
///
/// `landed: false` means Spotify did not take the write. That is a state trak
/// reports, never a crash.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOutcome {
    pub what: &'static str,
    pub wanted: i64,
    pub read: i64,
    pub landed: bool,
}

impl WriteOutcome {
    /// The one line shown when a write did not take (TODO 4.8).
    pub fn notice(&self) -> Option<String> {
        if self.landed {
            return None;
        }
        Some(match self.what {
            "volume" => format!(
                "Spotify ignored the volume change (set {}, it reports {}). \
                 The volume meter is hidden; see COMPAT rule 5.",
                self.wanted, self.read
            ),
            other => format!(
                "Spotify ignored the {other} change ({:?}).",
                (self.wanted, self.read)
            ),
        })
    }
}

/// Seek, then read back. A seek is a write, so it gets the same treatment.
pub fn seek_checked<P: Player + ?Sized>(p: &mut P, secs: f64) -> Result<WriteOutcome, PlayerError> {
    p.seek(secs)?;
    let after = p.state()?;

    // A real player clamps a seek past either end, so the value to compare
    // against is the clamped target, not what the caller asked for. Comparing
    // against the raw request would report every over-seek as a failure.
    let dur = after.track.duration_secs() as f64;
    let expected = if dur > 0.0 {
        secs.clamp(0.0, dur)
    } else {
        secs.max(0.0)
    };
    // Within a second either way: a position read is noisy and the bar is
    // interpolated anyway (ARCHITECTURE).
    let landed = (after.position_secs - expected).abs() < 1.0;

    Ok(WriteOutcome {
        what: "seek",
        wanted: expected as i64,
        read: after.position_secs as i64,
        landed,
    })
}

/// Set the volume and report whether Spotify took it.
///
/// This is the one place the ±1 tolerance is applied, so the rule has exactly one
/// implementation and one set of tests.
pub fn set_volume_checked<P: Player + ?Sized>(
    p: &mut P,
    volume: u8,
) -> Result<WriteOutcome, PlayerError> {
    let want = volume.min(100);
    p.set_volume(want)?;
    let read = p.state()?.volume;
    Ok(WriteOutcome {
        what: "volume",
        wanted: i64::from(want),
        read: i64::from(read),
        landed: volume_write_landed(want, read),
    })
}

/// Step the volume, staying inside 0–100 rather than wrapping.
pub fn step_volume<P: Player + ?Sized>(p: &mut P, step: i16) -> Result<WriteOutcome, PlayerError> {
    let current = i16::from(p.state()?.volume);
    let target = (current + step).clamp(0, 100) as u8;
    set_volume_checked(p, target)
}

/// A user-initiated action. The event loop runs these; `update` never does.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    Toggle,
    Next,
    Prev,
    Replay,
    /// `Some(secs)` to seek, `None` to step by the configured amount.
    Seek(f64),
    VolumeStep(i16),
    SetVolume(u8),
    ToggleShuffle,
    CycleRepeat,
    /// Play a URI. `enter` on a history row (TODO 3.6) and the CLI both need it.
    #[allow(dead_code, reason = "wired up with the history list, TODO 3.6")]
    PlayUri(String),
    /// Put a link on the pasteboard. Not a Spotify write.
    CopyLink(String),
    /// The idle card's enter. The only launch trak ever performs (COMPAT rule 2).
    Launch,
    /// Put a macOS notification on screen (TODO 4.5). Not a Spotify write at all:
    /// it is `display notification` through osascript, and it is a command rather
    /// than something the loop does inline because osascript is a process spawn
    /// and must never happen on the render thread.
    Notify(String, String),
}

/// What a finished command reports back.
///
/// Three outcomes, not two: a command can fail, it can succeed, and it can
/// *succeed at being ignored* — Spotify takes the call and does nothing with it.
/// Folding those last two together is what made the volume meter flap, because
/// the app had no way to hear about a write that quietly did not land
/// (COMPAT rule 5).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutcome {
    pub cmd: PlayerCommand,
    /// `Err` is a failure to make the call at all. `Ok(None)` is a command with
    /// nothing to read back. `Ok(Some(outcome))` is one that was read back, and
    /// `outcome.landed` says whether Spotify took it.
    pub result: Result<Option<WriteOutcome>, PlayerError>,
}

impl CommandOutcome {
    pub fn ok(cmd: PlayerCommand) -> Self {
        Self {
            cmd,
            result: Ok(None),
        }
    }

    pub fn failed(cmd: PlayerCommand, e: PlayerError) -> Self {
        Self {
            cmd,
            result: Err(e),
        }
    }

    pub fn read_back(cmd: PlayerCommand, outcome: WriteOutcome) -> Self {
        Self {
            cmd,
            result: Ok(Some(outcome)),
        }
    }
}

/// A fetched and decoded cover.
///
/// Both halves come back from the worker: the path because that is what is
/// cached and what identifies the image, and the decoded pixels so the render
/// thread does no decoding (TODO 4.1, "never block the UI thread").
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedArt {
    pub path: std::path::PathBuf,
    pub image: image::DynamicImage,
}

/// What a finished job reports back.
///
/// A poll and a write are different shapes: a poll's whole point is the state it
/// read, while a write only reports whether it landed. Folding them into one
/// `Result<(), _>` is what made the TUI poll without ever displaying anything.
#[derive(Debug)]
pub enum WorkerResult {
    /// A read succeeded.
    State(Box<crate::player::PlayerState>),
    /// A read failed. `NotRunning` is the common one and gets its own handling in
    /// the TUI, because it means "show the idle card", not "show an error".
    ReadFailed(PlayerError),
    /// A write finished.
    Command(CommandOutcome),
    /// An album-art download finished. Not a player job at all, but it runs on
    /// the same worker so the render loop never waits on the network
    /// (TODO 4.1).
    Art {
        url: String,
        result: Result<LoadedArt, crate::art::ArtError>,
    },
    /// Sonar's state file was re-read. Not a Spotify job; it runs here so the
    /// render loop never waits on the filesystem (TODO 4.6).
    Sonar(crate::sonar::SonarState),
    /// headless-spotify answered (TODO 4.7).
    Headless(crate::headless::Headless),
    /// A lyrics lookup finished. Not a Spotify job either; it runs here so the
    /// render loop never waits on a network round trip (TODO 6.1).
    Lyrics {
        /// The track the lyrics were fetched *for*. A result for a track the user
        /// has skipped past is dropped rather than shown under the wrong title.
        uri: Option<String>,
        result: Result<crate::lyrics::Lyrics, crate::lyrics::LyricsError>,
    },
}

/// Runs player writes on a worker thread so the render loop never blocks.
///
/// A read is ~430 ms and a write similar (docs/APPLESCRIPT.md §4), so doing either
/// on the UI thread would freeze the TUI for the best part of a second.
///
/// Used by the TUI event loop (TODO 3.2); the CLI is a one-shot command and does
/// not need it.
pub struct Worker {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<WorkerResult>,
    handle: Option<JoinHandle<()>>,
    /// Set while a command is in flight, so a held-down key does not queue ten
    /// of them.
    busy: Arc<AtomicBool>,
}

/// Put text on the macOS pasteboard.
///
/// `pbcopy` rather than a pasteboard API: it needs no entitlement, no framework
/// binding, and it is what every other tool on the machine already uses. A
/// failure is not worth surfacing as an error -- the link has already been
/// printed -- so it is reported as a bool and the caller carries on.
pub fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        let _ = child.kill();
        return false;
    }
    // Dropping stdin closes the pipe, which is what makes pbcopy commit.
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// A unit of work for the worker, boxed so one channel carries every command.
type Job = Box<dyn FnOnce(&mut dyn Player) -> WorkerResult + Send>;

impl Drop for Worker {
    /// Shutting down on drop is what guarantees the thread is joined. Without it a
    /// thread could still be inside an AppleScript call while the terminal is
    /// being restored, which is how a TUI ends up leaving the terminal broken.
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    pub fn new<P: Player + Send + 'static>(player: P) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (result_tx, result_rx) = channel::<WorkerResult>();
        let busy = Arc::new(AtomicBool::new(false));
        let thread_busy = Arc::clone(&busy);

        let handle = std::thread::Builder::new()
            .name("trak-player".into())
            .spawn(move || {
                // One player for the life of the thread, mutably borrowed per job.
                let mut player = player;
                while let Ok(job) = job_rx.recv() {
                    let out = job(&mut player);
                    // Clear before publishing, so a caller that sees a result
                    // always sees an idle worker.
                    thread_busy.store(false, Ordering::Release);
                    if result_tx.send(out).is_err() {
                        // The app is gone; nothing left to report to.
                        return;
                    }
                }
            })
            .expect("spawning the player worker");

        Self {
            job_tx: Some(job_tx),
            result_rx,
            handle: Some(handle),
            busy,
        }
    }

    /// Queue a command, and say whether it was accepted.
    ///
    /// Does nothing if one is already running, which is what keeps a repeated
    /// keypress from building a backlog. **The bool matters**: a caller that has
    /// already recorded "a fetch is in flight" needs to know whether it really
    /// is, or the state is stuck at Loading forever with nothing on its way.
    /// That is not hypothetical -- it is how the lyrics tab ended up saying
    /// "looking for lyrics…" for every track.
    pub fn submit<F>(&self, f: F) -> bool
    where
        F: FnOnce(&mut dyn Player) -> WorkerResult + Send + 'static,
    {
        let Some(tx) = &self.job_tx else { return false };
        if self.busy.load(Ordering::Acquire) {
            return false;
        }
        self.busy.store(true, Ordering::Release);
        if tx.send(Box::new(f)).is_err() {
            self.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Take a finished job, if there is one. Never blocks.
    pub fn poll(&self) -> Option<WorkerResult> {
        self.result_rx.try_recv().ok()
    }

    /// Stop the worker and wait for it, so no thread outlives the process.
    ///
    /// Dropping the job sender is what ends the worker's `recv`; joining is what
    /// proves it actually stopped. Called on quit and by the panic hook, because a
    /// detached thread holding an AppleScript call would outlive the terminal.
    pub fn shutdown(&mut self) {
        self.job_tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::{FakePlayer, Quirks};

    #[test]
    fn a_volume_write_lands_on_a_normal_player() {
        let mut p = FakePlayer::playing();
        let o = set_volume_checked(&mut p, 70).unwrap();
        assert!(o.landed);
        assert_eq!(o.read, 70);
        assert!(o.notice().is_none());
    }

    /// The regression this module exists for.
    #[test]
    fn a_quantised_read_back_still_counts_as_landed() {
        let mut p = FakePlayer::with_quirks(Quirks {
            quantise_volume: true,
            ..Quirks::default()
        });
        let o = set_volume_checked(&mut p, 70).unwrap();
        assert_eq!(o.read, 69);
        assert!(o.landed, "one low must still count as landed");
        assert!(o.notice().is_none());
    }

    #[test]
    fn an_ignored_write_is_reported_with_a_one_line_notice() {
        let mut p = FakePlayer::with_quirks(Quirks {
            ignore_volume_writes: true,
            ..Quirks::default()
        });
        let o = set_volume_checked(&mut p, 10).unwrap();
        assert!(!o.landed);
        let n = o.notice().expect("an ignored write must explain itself");
        assert!(!n.contains('\n'), "one line only: {n:?}");
        assert!(n.contains("volume"), "{n}");
    }

    #[test]
    fn stepping_the_volume_clamps_at_both_ends() {
        let mut p = FakePlayer::playing();
        p.set_volume(95).unwrap();
        assert_eq!(
            step_volume(&mut p, 10).unwrap().wanted,
            100,
            "no wrap to 105"
        );
        p.set_volume(5).unwrap();
        assert_eq!(step_volume(&mut p, -10).unwrap().wanted, 0, "no wrap to -5");
    }

    #[test]
    fn a_seek_is_read_back_against_the_clamped_target() {
        let mut p = FakePlayer::playing();
        let o = seek_checked(&mut p, 60.0).unwrap();
        assert!(o.landed);
        assert_eq!(o.read, 60);

        let o = seek_checked(&mut p, 9999.0).unwrap();
        assert!(o.landed, "clamping to the end is landing, not a failure");
        assert_eq!(o.wanted, 360, "the reported target is the clamped one");

        let o = seek_checked(&mut p, -10.0).unwrap();
        assert!(o.landed);
        assert_eq!(o.read, 0);
    }

    #[test]
    fn writes_propagate_a_player_error() {
        let mut p = FakePlayer::with_quirks(Quirks {
            not_running: true,
            ..Quirks::default()
        });
        assert!(matches!(
            set_volume_checked(&mut p, 50),
            Err(PlayerError::NotRunning)
        ));
        assert!(matches!(
            seek_checked(&mut p, 10.0),
            Err(PlayerError::NotRunning)
        ));
        assert!(matches!(
            step_volume(&mut p, 10),
            Err(PlayerError::NotRunning)
        ));
    }

    /// A poll comes back as the state it read, which is the only way the TUI has
    /// anything to draw.
    #[test]
    fn a_poll_job_returns_the_state_it_read() {
        let w = Worker::new(FakePlayer::playing());
        w.submit(|p| match p.state() {
            Ok(s) => WorkerResult::State(Box::new(s)),
            Err(e) => WorkerResult::ReadFailed(e),
        });
        let mut got = None;
        for _ in 0..200 {
            if let Some(r) = w.poll() {
                got = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match got.expect("a result") {
            WorkerResult::State(s) => {
                assert_eq!(s.track.title, "Census Designated");
            }
            other => panic!("expected a state, got {other:?}"),
        }
    }

    #[test]
    fn a_poll_on_a_missing_player_reports_read_failed() {
        let w = Worker::new(FakePlayer::with_quirks(Quirks {
            not_running: true,
            ..Quirks::default()
        }));
        w.submit(|p| match p.state() {
            Ok(s) => WorkerResult::State(Box::new(s)),
            Err(e) => WorkerResult::ReadFailed(e),
        });
        let mut got = None;
        for _ in 0..200 {
            if let Some(r) = w.poll() {
                got = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(
            got.expect("a result"),
            WorkerResult::ReadFailed(PlayerError::NotRunning)
        ));
    }

    #[test]
    fn a_failing_job_still_reports_and_clears_the_busy_flag() {
        let w = Worker::new(FakePlayer::with_quirks(Quirks {
            not_running: true,
            ..Quirks::default()
        }));
        w.submit(|p| match p.state() {
            Ok(s) => WorkerResult::State(Box::new(s)),
            Err(e) => WorkerResult::ReadFailed(e),
        });
        let mut got = None;
        for _ in 0..200 {
            if let Some(r) = w.poll() {
                got = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got.is_some());
        assert!(!w.is_busy(), "the worker must not stay stuck busy");
    }

    /// A held-down key must not queue a dozen writes.
    #[test]
    fn the_worker_drops_commands_while_one_is_in_flight() {
        use std::sync::atomic::AtomicUsize;
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let w = Worker::new(FakePlayer::playing());
        for _ in 0..50 {
            let c = Arc::clone(&counter);
            w.submit(move |p| {
                c.fetch_add(1, Ordering::AcqRel);
                std::thread::sleep(std::time::Duration::from_millis(30));
                let _ = set_volume_checked(p, 10);
                WorkerResult::Command(CommandOutcome::ok(PlayerCommand::Toggle))
            });
        }
        assert!(w.is_busy());
        for _ in 0..400 {
            let _ = w.poll();
            if !w.is_busy() && ran.load(Ordering::Acquire) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let n = ran.load(Ordering::Acquire);
        assert!(n >= 1, "at least one command must run");
        assert!(n < 50, "commands must be coalesced, but {n} of 50 ran");
    }

    #[test]
    fn the_clipboard_helper_actually_copies() {
        // pbcopy is present on every macOS, so this is safe in CI.
        let marker = format!("trak-clipboard-test-{}", std::process::id());
        assert!(
            copy_to_clipboard(&marker),
            "pbcopy should have accepted the text"
        );
        // Read it back to be sure, rather than trusting the exit status.
        let out = std::process::Command::new("pbpaste")
            .output()
            .expect("pbpaste");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), marker);
    }

    #[test]
    fn shutting_down_joins_the_thread() {
        let mut w = Worker::new(FakePlayer::playing());
        w.shutdown();
        w.shutdown();
        assert!(w.poll().is_none());
    }
}
