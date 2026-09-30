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

/// The result of a background command, reported back as an event.
#[allow(dead_code, reason = "used by the TUI event loop, TODO 3.2")]
pub type CommandResult = Result<WriteOutcome, PlayerError>;

/// Runs player writes on a worker thread so the render loop never blocks.
///
/// A read is ~430 ms and a write similar (docs/APPLESCRIPT.md §4), so doing either
/// on the UI thread would freeze the TUI for the best part of a second.
///
/// Used by the TUI event loop (TODO 3.2); the CLI is a one-shot command and does
/// not need it.
#[allow(dead_code, reason = "used by the TUI event loop, TODO 3.2")]
pub struct Worker {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<CommandResult>,
    handle: Option<JoinHandle<()>>,
    /// Set while a command is in flight, so a held-down key does not queue ten
    /// of them.
    busy: Arc<AtomicBool>,
}

/// A unit of work for the worker, boxed so one channel carries every command.
#[allow(dead_code, reason = "used by the TUI event loop, TODO 3.2")]
type Job = Box<dyn FnOnce(&mut dyn Player) -> CommandResult + Send>;

// The worker is the TUI's background thread; the CLI never needs one. It gains a
// caller in TODO 3.2, which is the very next task.
#[allow(dead_code, reason = "used by the TUI event loop, TODO 3.2")]
impl Worker {
    pub fn new<P: Player + Send + 'static>(player: P) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (result_tx, result_rx) = channel::<CommandResult>();
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

    /// Queue a command. Does nothing if one is already running, which is what
    /// keeps a repeated keypress from building a backlog.
    pub fn submit<F>(&self, f: F)
    where
        F: FnOnce(&mut dyn Player) -> CommandResult + Send + 'static,
    {
        let Some(tx) = &self.job_tx else { return };
        if self.busy.load(Ordering::Acquire) {
            return;
        }
        self.busy.store(true, Ordering::Release);
        if tx.send(Box::new(f)).is_err() {
            self.busy.store(false, Ordering::Release);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Take a finished command, if there is one. Never blocks.
    pub fn poll(&self) -> Option<CommandResult> {
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
    use crate::player::PlaybackState;
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
        let o = step_volume(&mut p, 10).unwrap();
        assert_eq!(o.wanted, 100, "must not wrap to 105");

        p.set_volume(5).unwrap();
        let o = step_volume(&mut p, -10).unwrap();
        assert_eq!(o.wanted, 0, "must not wrap to -5");
    }

    #[test]
    fn stepping_down_then_up_returns_to_the_start() {
        let mut p = FakePlayer::playing();
        p.set_volume(80).unwrap();
        step_volume(&mut p, -10).unwrap();
        let back = step_volume(&mut p, 10).unwrap();
        assert_eq!(back.wanted, 80);
        assert!(back.landed);
    }

    #[test]
    fn a_seek_is_read_back_and_lands() {
        let mut p = FakePlayer::playing();
        let o = seek_checked(&mut p, 60.0).unwrap();
        assert!(o.landed);
        assert_eq!(o.read, 60);
    }

    #[test]
    fn a_seek_past_the_end_lands_at_the_end() {
        let mut p = FakePlayer::playing();
        let o = seek_checked(&mut p, 9999.0).unwrap();
        assert!(o.landed, "clamping to the end is landing, not a failure");
        assert_eq!(o.wanted, 360, "the reported target is the clamped one");
        assert_eq!(o.read, 360);
    }

    #[test]
    fn a_seek_before_the_start_lands_at_the_start() {
        let mut p = FakePlayer::playing();
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

    #[test]
    fn the_worker_runs_a_command_off_the_calling_thread() {
        let w = Worker::new(FakePlayer::playing());
        w.submit(|p| {
            let o = set_volume_checked(p, 42)?;
            Ok(o)
        });
        // Give the worker a moment, then collect.
        let mut out = None;
        for _ in 0..200 {
            if let Some(r) = w.poll() {
                out = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let o = out.expect("the worker should have reported").unwrap();
        assert_eq!(o.wanted, 42);
        assert!(o.landed);
    }

    /// A held-down key must not queue a dozen writes: while one job runs, further
    /// submissions are dropped rather than piled up behind it.
    #[test]
    fn the_worker_drops_commands_while_one_is_in_flight() {
        use std::sync::atomic::AtomicUsize;
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let w = Worker::new(FakePlayer::playing());

        // The 50 submissions happen far faster than the job can finish, so they
        // all collide with the first.
        for _ in 0..50 {
            let c = Arc::clone(&counter);
            w.submit(move |p| {
                c.fetch_add(1, Ordering::AcqRel);
                std::thread::sleep(std::time::Duration::from_millis(30));
                set_volume_checked(p, 10)
            });
        }
        assert!(w.is_busy(), "the first submit should be in flight");

        // Wait for it to drain, then count what actually ran.
        let mut done = 0;
        for _ in 0..400 {
            if w.poll().is_some() {
                done += 1;
            }
            if !w.is_busy() && ran.load(Ordering::Acquire) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let n = ran.load(Ordering::Acquire);
        assert!(n >= 1, "at least one command must run");
        assert!(n < 50, "commands must be coalesced, but {n} of 50 ran");
        assert!(done >= 1, "the result must come back");
    }

    #[test]
    fn a_failing_command_still_reports_and_clears_the_busy_flag() {
        let w = Worker::new(FakePlayer::with_quirks(Quirks {
            not_running: true,
            ..Quirks::default()
        }));
        w.submit(|p| set_volume_checked(p, 10));
        let mut got = None;
        for _ in 0..200 {
            if let Some(r) = w.poll() {
                got = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(got, Some(Err(PlayerError::NotRunning))));
        assert!(!w.is_busy(), "the worker must not stay stuck busy");
    }

    #[test]
    fn shutting_down_joins_the_thread() {
        let mut w = Worker::new(FakePlayer::playing());
        w.shutdown();
        // A second shutdown is harmless, and polling a dead worker is fine.
        w.shutdown();
        assert!(w.poll().is_none());
    }

    #[test]
    fn a_paused_player_still_accepts_a_seek() {
        let mut p = FakePlayer::playing();
        p.set_playback(PlaybackState::Paused);
        let o = seek_checked(&mut p, 30.0).unwrap();
        assert!(o.landed);
    }
}
