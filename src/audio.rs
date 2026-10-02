//! Real audio for the visualizer: a Core Audio **process tap on Spotify only**,
//! a lock-free ring buffer, and an FFT on a worker thread (TODO 8.3, 8.5).
//!
//! Three parts, in the order the audio flows.
//!
//! - The **capture** half: Spotify's pids become Core Audio process object ids,
//!   those become an include-list `CATapDescription`, and the tap arrives inside
//!   an aggregate device with an IO proc attached. It is *Spotify's process and
//!   nothing else*: a global tap would also carry Sonar's audio, which is what
//!   `docs/COMPAT.md` rule 3 is about.
//! - The **handover**: [`SampleRing`]. The IO proc runs on a realtime thread and
//!   does nothing but copy samples into a single-producer/single-consumer ring,
//!   so nothing there allocates, locks or panics.
//! - The **analysis** half: [`Analyser`] on a worker thread, which drains the
//!   ring, runs one transform per tick and publishes band magnitudes the
//!   render thread can read without waiting for anything.
//!
//! Everything above that is [`AudioPipeline`], which decides between the tap and
//! [`SimulatedSource`]. It is created with [`AudioPipeline::new`] and never
//! blocks, because every Core Audio call happens on the worker.
//!
//! `docs/AUDIO-TAP.md` is the write-up of the experiments this is built on, and
//! every trap recorded there is load-bearing here. The four that cost the most:
//!
//! 1. **The rate is read, never assumed.** The transform is sized from the tap's
//!    own `asbd.sample_rate` ([`window_len`]), because bin `k` means
//!    `k * rate / 2N` and a 44.1 kHz transform over 48 kHz audio puts every band
//!    in the wrong octave.
//! 2. **The `StartedDevice` is held for the whole capture.** Dropping it stops the
//!    device, and a stopped device delivers nothing, which looks exactly like a
//!    hang. [`TapSession`] exists so that is one field's problem.
//! 3. **The pids are not the ids `CATapDescription` wants.** Its header asks for
//!    the *process object* `AudioObjectID`, not the pid, and handing it the pid
//!    gives `!obj` (`kAudioHardwareBadObjectError`) for any non-empty list.
//! 4. **A tap with no process to tap is not created at all.** An empty include
//!    list is *accepted* by macOS and then delivers silence forever, which is a
//!    much harder thing to notice than an error.
//!
//! The analysis half is a plain radix-2 FFT and a log-spaced band map, both in
//! this file. That is not a preference for the hand-rolled version: it is because
//! TODO 1.6 chose `cavacore` for this job and 1.6's own measurement was wrong.
//! `cavacore` 2.0.2 builds each band's FFT range with
//! `relative_cut_off[n] as u32 * (len / 2)` where `relative_cut_off[n]` is
//! `cut_off / (rate / 2)` — a fraction, so the cast truncates it to **zero** and
//! every band is assigned the same first bin (`cavacore-2.0.2/src/lib.rs:339`).
//! The whole 60 Hz–16 kHz range then collapses into bins 0…31 of one transform:
//! measured, an 80 Hz sine reports band 9 and a 16 kHz sine reports the same
//! band 9, and everything above ~850 Hz is indistinguishable. There is no
//! configuration that avoids it — the cast is only non-zero above Nyquist, which
//! the builder rejects — and upstream still has it on `main` as of 2026-10.
//! `docs/AUDIO-TAP.md` §2f has the measurement and the fix.
//!
//! Fallback is the product decision, not an error path: a machine that denies the
//! tap still gets bars that move with the music, and the first failure raises
//! exactly one line saying what to grant ([`TapError::notice`]).

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, Instant};

use cidre::{cat, cf, core_audio as ca, ns};

use crate::config::VisualizerSource;
use crate::visualizer::{AudioSource, BARS, SimulatedSource};

/// How long the worker waits between analyses. The frame tick reads the bars at
/// 30 fps (TODO 8.4), so much slower than this is wasted work and much faster is
/// a wake-up nobody asked for. 20 ms puts the analysis just ahead of the frame it
/// feeds, and is also the resolution [`RELEASE`] is measured in.
const TICK: Duration = Duration::from_millis(20);

/// How many ticks the callback can be silent before the analyser is told the window is
/// silence. Three is 60 ms — far below one 30 fps frame, and comfortably longer than a
/// callback that arrives a tick late.
const SILENT_TICKS: u32 = 3;

/// How long before trying the tap again after a failure.
///
/// This is also how "reattach when Spotify restarts" (TODO 8.5) is delivered: a
/// tap cannot outlive the process it taps, so quitting Spotify has to take the tap
/// down and putting it back has to be somebody's job. Two seconds is short enough
/// that a restart is invisible, and long enough that a machine where the tap will
/// never work is not spinning.
const RETRY: Duration = Duration::from_secs(2);

/// How often a live tap checks that Spotify is still running. Once a second is
/// fast enough that a quit is noticed before anyone looks back at the pane, and
/// `kill(pid, 0)` once a second is nothing.
const LIVENESS: Duration = Duration::from_secs(1);

/// The longest an idle worker blocks before looking for a signal again. Only a
/// signal that arrives with no tap up waits this long, and then nothing needs
/// tearing down.
const IDLE_WAIT: Duration = Duration::from_millis(250);

/// The ring buffer's depth, in samples. 32 768 is a power of two — the ring masks
/// rather than divides, on the audio thread — and 0.68 s at the tap's 48 kHz:
/// enough that a descheduled worker cannot overrun it, small enough that an
/// overrun costs nothing but the oldest 20 ms.
const RING_SAMPLES: usize = 1 << 15;

/// The bundle id Trak talks to. Spotify's helper processes share it, so the tap
/// covers them too, which is what keeps the captured audio continuous across a
/// track change.
const SPOTIFY_BUNDLE_ID: &str = "com.spotify.client";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why the tap is not running.
///
/// Every variant is something the OS or Spotify did, not something Trak did, so
/// each reads as a fact and none of them is fatal: the visualizer falls back to
/// [`SimulatedSource`] and the TUI carries on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TapError {
    /// Spotify is not running. Not a failure — the visualizer has nothing to tap
    /// yet and says nothing about it.
    #[error("Spotify is not running")]
    SpotifyNotRunning,

    /// Spotify is running but Core Audio does not know it as an audio client, so
    /// there is no process object to put in the tap's include list. Also not a
    /// failure: it is what a Spotify that has just launched reports.
    #[error("Spotify is not an audio client yet")]
    NoAudioProcess,

    /// There is no output device to hang the aggregate device off, so there is
    /// nowhere for the tap's audio to come from.
    #[error("no output device")]
    NoOutputDevice,

    /// `kAudioHardwarePermissionsError`. The only variant that needs a human: the
    /// grant lives in System Settings and Trak cannot ask for it itself.
    #[error("macOS refused the tap ({0})")]
    PermissionDenied(&'static str),

    /// Any other `OSStatus` from Core Audio, named rather than numbered.
    #[error("Core Audio {0}")]
    CoreAudio(String),

    /// The tap came up in a format the analyser cannot read. The tap on the machine
    /// this was built on is 48 kHz mono 32-bit float; anything else is a surprise
    /// worth a line rather than a wrong-looking spectrum.
    #[error("the tap delivers {0}, which is not 32-bit float audio")]
    UnsupportedFormat(String),

    /// The tap's rate is not one the analyser accepts.
    #[error("the tap runs at {0} Hz, which is not a rate the analyser accepts")]
    BadSampleRate(String),

    /// The analyser refused the configuration the tap's format implies.
    #[error("the analyser rejected the tap's format: {0}")]
    Analyser(String),
}

impl TapError {
    /// The one line to show the owner, or `None` for a state that is not worth a
    /// message.
    ///
    /// Spotify not running gets no line: the TUI is already on its idle card, and a
    /// toast about it would be the second thing saying the same thing.
    pub fn notice(&self) -> Option<String> {
        match self {
            Self::SpotifyNotRunning | Self::NoAudioProcess => None,
            Self::PermissionDenied(_) => Some(
                "real audio off: macOS denied the audio tap — allow Trak under System Settings \
                 › Privacy & Security › Screen & System Audio Recording. Bars are simulated."
                    .into(),
            ),
            _ => Some(format!("real audio off ({}). Bars are simulated.", self)),
        }
    }

    /// Whether the failure is one the owner should be told about.
    ///
    /// Nothing here is treated as final, including a denied permission: the owner
    /// can grant it with Trak still open, and the worker is what notices. The
    /// distinction only decides whether a message is raised.
    fn is_quiet(&self) -> bool {
        matches!(self, Self::SpotifyNotRunning | Self::NoAudioProcess)
    }

    /// Classify an `OSStatus` from Core Audio.
    ///
    /// `!obj` is deliberately *not* read as a permission error: the spike spent a
    /// day being wrong about that (`docs/AUDIO-TAP.md` §3b), because `!obj` says the
    /// id did not map to an object, not that anybody refused.
    fn from_status(status: i32) -> Self {
        match status as u32 {
            // kAudioHardwarePermissionsError
            0x2168_6F67 => Self::PermissionDenied("'!hog'"),
            other => Self::CoreAudio(fourcc(other as i32)),
        }
    }
}

/// An `OSStatus` as its four-character code, because `560947818` is not a diagnosis
/// and `'!obj'` is.
fn fourcc(status: i32) -> String {
    let bytes = (status as u32).to_be_bytes();
    if bytes.iter().all(|c| (0x20..0x7f).contains(c)) {
        let name = String::from_utf8_lossy(&bytes).into_owned();
        format!("'{name}' 0x{:08X}", status as u32)
    } else {
        format!("0x{:08X}", status as u32)
    }
}

// ---------------------------------------------------------------------------
// The handover: a lock-free single-producer/single-consumer ring
// ---------------------------------------------------------------------------

/// Keeps the producer's and the consumer's cursors off each other's cache line.
/// Two atomics sharing a line are written about ninety times a second from two
/// threads, and the false sharing costs more than every allocation this avoids.
#[repr(align(64))]
struct Padded<T>(T);

/// A ring of `f32` between the realtime IO proc and the worker.
///
/// Written from Core Audio's thread, read from the worker's, and touched by nothing
/// else. Both cursors are free-running counters rather than masked positions,
/// which is what keeps "full" and "empty" distinguishable without wasting a slot:
/// the gap between them is the number of unread samples, and it can never exceed
/// the capacity, because only the consumer moves the read cursor.
///
/// Nothing in here allocates, locks or unwinds. It runs on the thread the audio
/// arrives on, where a wait or an allocation is a dropout.
struct SampleRing {
    /// `UnsafeCell` because the producer writes through `&self`: the ring *is* the
    /// shared mutable state, and the alternative — handing the callback a `&mut` it
    /// would have to keep across calls, from a C caller that does not — is exactly
    /// the aliasing this must not do. The safety argument is that each slot has one
    /// writer and one reader and they are different threads, so no two accesses ever
    /// overlap; the cursors are what prove it.
    samples: UnsafeCell<Box<[f32]>>,
    mask: usize,
    /// Moved by the producer only.
    head: Padded<AtomicUsize>,
    /// Moved by the consumer only.
    tail: Padded<AtomicUsize>,
}

impl SampleRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        Self {
            samples: UnsafeCell::new(vec![0.0; capacity].into_boxed_slice()),
            mask: capacity - 1,
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
        }
    }

    fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Samples waiting to be read.
    #[cfg(test)]
    fn available(&self) -> usize {
        self.head
            .0
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.0.load(Ordering::Acquire))
            .min(self.capacity())
    }

    /// Append `src`, dropping the oldest samples rather than the newest.
    ///
    /// The newest audio is the audio worth keeping: a buffer that filled because
    /// the consumer was descheduled should cost the oldest 20 ms, not the newest 20
    /// ms, because the newest is what is playing.
    ///
    /// A `src` longer than the whole ring keeps only its tail.
    fn push(&self, src: &[f32]) {
        let cap = self.capacity();
        let head = self.head.0.load(Ordering::Relaxed);
        let used = head
            .wrapping_sub(self.tail.0.load(Ordering::Acquire))
            .min(cap);
        let n = src.len().min(cap);
        // `n <= cap` and the free space is `cap - used`, so this moves the read
        // cursor only when the newest samples would not otherwise fit.
        let dropped = n.saturating_sub(cap - used);
        if dropped > 0 {
            self.tail.0.fetch_add(dropped, Ordering::Release);
        }
        // SAFETY: the acquire above ordered this against every read the consumer has
        // done, so the `dropped` slots being overwritten are ones it has finished
        // with, and `head - tail <= cap` bounds every index below. The borrow ends
        // before this function returns and no other thread holds one, which is the
        // SPSC invariant the whole type rests on.
        let samples = unsafe { &mut *self.samples.get() };
        // A `src` longer than the ring keeps its tail: the newest audio, for the same
        // reason the overflow case drops the oldest.
        for (i, &sample) in src[src.len() - n..].iter().take(n).enumerate() {
            // The mask bounds the index by construction, so this cannot fail; it is
            // written as a check anyway, because a reachable panic on the audio
            // thread is the one thing this function must not have.
            let Some(slot) = samples.get_mut((head.wrapping_add(i)) & self.mask) else {
                return;
            };
            *slot = sample;
        }
        self.head.0.store(head.wrapping_add(n), Ordering::Release);
    }

    /// Move up to `max` samples into `out` and return how many moved.
    fn drain(&self, out: &mut Vec<f32>, max: usize) -> usize {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let ready = self
            .head
            .0
            .load(Ordering::Acquire)
            .wrapping_sub(tail)
            .min(self.capacity());
        let n = ready.min(max);
        out.clear();
        out.reserve(n);
        // SAFETY: the acquire above ordered this after the producer's release, so
        // every sample up to `head` has been written; `ready` is bounded by the
        // capacity, so every index below is inside the buffer. Only the consumer
        // reads, and only the producer writes, so no borrow is live at the same time.
        let samples = unsafe { &*self.samples.get() };
        for i in 0..n {
            let Some(&sample) = samples.get((tail.wrapping_add(i)) & self.mask) else {
                break;
            };
            out.push(sample);
        }
        self.tail.0.store(tail.wrapping_add(n), Ordering::Release);
        n
    }
}

/// SAFETY: the buffer has exactly one writer and exactly one reader, and they are on
/// two different threads — Core Audio's IO proc and the worker — which is why this
/// type is shared at all. `push` is reachable only from the C callback and `drain`
/// only from the worker, so the two never run at the same time, and the acquire and
/// release pairs on `head` and `tail` order every sample against its reader. The
/// cells are disjoint, the cursors are atomic, and no third party can reach a sample
/// at all: those two methods are the only accessors.
unsafe impl Sync for SampleRing {}

// ---------------------------------------------------------------------------
// The handover: band magnitudes
// ---------------------------------------------------------------------------

/// The last set of band magnitudes, published by the worker and read by the render
/// thread.
///
/// One atomic per band, so a reader never waits and a writer never blocks. A reader
/// can catch a half-updated set — bands are 20 ms apart in time and each is a
/// magnitude, so a frame that mixes two moments differs by one tick of a spectrum
/// that is already moving. A mutex or a double buffer would buy nothing for 128
/// bytes read thirty times a second.
#[derive(Debug)]
struct BarCell {
    slots: [AtomicU32; BARS],
}

impl BarCell {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    fn store(&self, bars: &[f32]) {
        for (slot, value) in self.slots.iter().zip(bars) {
            slot.store(value.to_bits(), Ordering::Release);
        }
    }

    fn load(&self) -> Vec<f32> {
        self.slots
            .iter()
            .map(|slot| f32::from_bits(slot.load(Ordering::Acquire)))
            .collect()
    }
}

/// Turns the magnitudes into the 0..=1 values the renderers expect.
///
/// The magnitudes are **not** normalised (TODO 1.6): their absolute size depends
/// on how loud the master output happens to be, so without this a bar's height
/// would say how loud the Spotify window was rather than what was playing.
///
/// One recent peak: up instantly, down slowly. Up instantly because a loud frame
/// should look loud in this frame; down slowly because a peak that fell as fast as
/// it rose would make the whole spectrum twitch on every kick drum. At [`TICK`] and
/// [`RELEASE`] the fall is about a second and a quarter, longer than a bar's own
/// rise and fall.
///
/// The peak starts at nothing rather than at a floor, and reaching nothing is what
/// silence means: a frame of digital silence has zero magnitudes, so the reference
/// falls to zero and every bar is exactly zero within one tick. Nothing has to
/// guess at an absolute "this is quiet enough" level, which is the guess that made
/// `cavacore`'s own output unusable — its magnitudes span ten orders of magnitude
/// between a bass note and a 15 kHz one, so any fixed floor is either invisible or
/// cuts the top of the spectrum off.
struct BarScale {
    peak: f32,
}

/// How much of the tracked peak a frame leaves behind: a half-life of about eight
/// frames, so 160 ms. Long enough that a kick drum does not make the whole spectrum
/// twitch, short enough that a pause is silence in about two seconds.
const RELEASE: f32 = 0.92;

/// Below this fraction of the peak, a band is silence — about −60 dB.
///
/// **Measured, not chosen**: a Hann window's first sidelobe is −31 dB and its
/// leakage falls about 18 dB per octave after that, so a full-height band leaks
/// past −60 dB within about a octave and a half of itself. Everything the window
/// spreads from a real band is therefore under this, and a band under this is
/// either the window's own skirt or nothing at all. Neither is worth a row of
/// cells.
const RELATIVE_FLOOR: f32 = 1e-3;

impl BarScale {
    fn new() -> Self {
        Self { peak: 0.0 }
    }

    /// Normalise `bars` in place.
    fn scale(&mut self, bars: &mut [f32]) {
        // A NaN or an infinity from the transform must not become the peak: one
        // poisoned value in a `max` would then divide every other bar by nonsense.
        let loudest = bars
            .iter()
            .filter(|v| v.is_finite())
            .copied()
            .fold(0.0f32, f32::max)
            .max(0.0);
        self.peak = loudest.max(self.peak * RELEASE);
        if self.peak <= 0.0 {
            // Silence: no reference, so nothing to draw. Reached on the first frame
            // of a tap and again within a second of the music stopping.
            self.peak = 0.0;
            bars.fill(0.0);
            return;
        }
        let floor = self.peak * RELATIVE_FLOOR;
        for bar in bars.iter_mut() {
            *bar = if bar.is_finite() && *bar > floor {
                (*bar / self.peak).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }
}

// ---------------------------------------------------------------------------
// The transform
// ---------------------------------------------------------------------------

/// A radix-2 FFT, sized once and reused for every frame.
///
/// Hand-rolled because it is a hundred lines and no options, and because the crate
/// that used to do this job cannot: see the module docs and `docs/AUDIO-TAP.md` §2f.
/// A dependency would be no smaller than the tests this needs, and this is the part
/// of the pipeline where being able to read the answer matters more than not writing
/// the code.
mod fft {
    /// A complex number as the transform needs it.
    #[derive(Clone, Copy)]
    pub(super) struct Complex {
        re: f64,
        im: f64,
    }

    impl Complex {
        fn new(re: f64, im: f64) -> Self {
            Self { re, im }
        }

        /// The length of the vector, which is what a spectrum is made of.
        fn magnitude(self) -> f64 {
            self.re.hypot(self.im)
        }
    }

    impl std::ops::Add for Complex {
        type Output = Self;
        fn add(self, rhs: Self) -> Self {
            Self::new(self.re + rhs.re, self.im + rhs.im)
        }
    }

    impl std::ops::Sub for Complex {
        type Output = Self;
        fn sub(self, rhs: Self) -> Self {
            Self::new(self.re - rhs.re, self.im - rhs.im)
        }
    }

    impl std::ops::Mul for Complex {
        type Output = Self;
        fn mul(self, rhs: Self) -> Self {
            Self::new(
                self.re * rhs.re - self.im * rhs.im,
                self.re * rhs.im + self.im * rhs.re,
            )
        }
    }

    /// `exp(-2*pi*i*t/len)` for `t` in `0..len/2`, the twiddle factors of a
    /// decimation-in-time transform.
    fn twiddles(len: usize) -> Vec<Complex> {
        let step = -std::f64::consts::TAU / len as f64;
        (0..len / 2)
            .map(|t| {
                let angle = step * t as f64;
                Complex::new(angle.cos(), angle.sin())
            })
            .collect()
    }

    /// `bit_reverse(i)` for every `i`, so the permutation is table lookup rather
    /// than arithmetic on every frame.
    fn bit_reversal(len: usize) -> Vec<usize> {
        let bits = len.trailing_zeros();
        (0..len)
            .map(|i| (0..bits).fold(0usize, |acc, bit| (acc << 1) | ((i >> bit) & 1)))
            .collect()
    }

    /// A Hann window of `len` points, which is what makes one tone occupy a few
    /// bins instead of smearing across all of them.
    fn hann(len: usize) -> Vec<f64> {
        if len < 2 {
            return vec![1.0; len];
        }
        (0..len)
            .map(|i| 0.5 * (1.0 - (std::f64::consts::TAU * i as f64 / (len - 1) as f64).cos()))
            .collect()
    }

    /// A forward transform of a fixed power-of-two length.
    ///
    /// A real signal is transformed as if it were complex rather than by the usual
    /// real-FFT packing. That costs a factor of two in arithmetic and buys an
    /// implementation with no index bookkeeping in it at all. At one transform per
    /// [`TICK`] on 2048 points it is a few megaflops a second, which is not a number
    /// the CPU ever notices — `docs/AUDIO-TAP.md` §4 has the measurement on this
    /// machine.
    pub(super) struct Fft {
        len: usize,
        /// The Hann window, because a transform without one is not a spectrum
        /// estimate: it is a way of finding how much energy is *in* a band, which is
        /// what a bar means, rather than a rectangle in time smearing a tone across
        /// the whole pane.
        window: Vec<f64>,
        /// The twiddle factor for a butterfly, pre-indexed per stage: stage `s`
        /// uses every `len / 2^(s+1)`-th of them.
        twiddles: Vec<Complex>,
        reversal: Vec<usize>,
        signal: Vec<Complex>,
    }

    impl Fft {
        /// A transform of `len` points.
        ///
        /// # Panics
        ///
        /// If `len` is not a power of two of at least 2. `len` comes from
        /// [`window_len`], which only ever returns powers of two, and
        /// [`the_window_length_is_always_a_power_of_two`] pins that; the check is here
        /// because an FFT that silently computes the wrong thing for a
        /// non-power-of-two length is a much worse outcome than a panic on the worker
        /// with `panic = "abort"` in the release profile.
        pub(super) fn new(len: usize) -> Self {
            assert!(len.is_power_of_two() && len >= 2, "FFT length {len}");
            Self {
                len,
                window: hann(len),
                twiddles: twiddles(len),
                reversal: bit_reversal(len),
                signal: Vec::new(),
            }
        }

        /// The length of the transform, and so of the window.
        pub(super) fn len(&self) -> usize {
            self.len
        }

        /// The magnitudes of the transform of `samples`, newest `len` samples only.
        ///
        /// `out` is filled with `len / 2 + 1` magnitudes — DC through Nyquist. The
        /// mirror a real signal also produces is left alone: nothing between here and
        /// a bar height wants it, and reading it would double every frequency's
        /// apparent energy.
        ///
        /// `samples` shorter than `len` is treated as if the missing samples were
        /// silence, which is the truth at the start of a capture: there is no history
        /// yet, and inventing one would be a click.
        pub(super) fn magnitudes(&mut self, samples: &[f64], out: &mut [f64]) {
            let len = self.len;
            let tail = samples.len().saturating_sub(len);
            self.signal.clear();
            self.signal.reserve(len);
            for (i, sample) in samples[tail..].iter().enumerate() {
                self.signal.push(Complex::new(sample * self.window[i], 0.0));
            }
            // Fewer samples than the window is zero-padding, so a frame shorter than
            // the window is windowed against silence rather than shifted.
            self.signal.resize(len, Complex::new(0.0, 0.0));

            let data = &mut self.signal;
            for (i, &j) in self.reversal.iter().enumerate() {
                if j > i {
                    data.swap(i, j);
                }
            }
            let mut width = 2;
            while width <= len {
                let half = width / 2;
                let stride = len / width;
                let mut block = 0;
                while block < len {
                    for k in 0..half {
                        let w = self.twiddles[k * stride];
                        let even = data[block + k];
                        let odd = data[block + k + half] * w;
                        data[block + k] = even + odd;
                        data[block + k + half] = even - odd;
                    }
                    block += width;
                }
                width <<= 1;
            }

            for (bin, slot) in out.iter_mut().enumerate().take(len / 2 + 1) {
                *slot = data[bin].magnitude();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The band map
// ---------------------------------------------------------------------------

/// Where each bar's frequencies are, and how much of it to show.
///
/// **Log-spaced**, because music is: the bands of a display that are spaced
/// linearly put two thirds of the pane above 5 kHz, where a spectrum has nothing
/// to say, and give the bass — where the energy and the eye are — one bar. This is
/// the same 60 Hz to 16 kHz range TODO 1.6 chose for `cavacore`, over the same 32
/// bars, so the simulated and the real source still draw the same shape.
struct BandMap {
    /// The first bin of each band, inclusive.
    lower: [u16; BARS],
    /// The last bin of each band, inclusive.
    upper: [u16; BARS],
    /// How much of each band to show, relative to the lowest.
    gain: [f32; BARS],
}

/// The lowest frequency a bar covers: below this the display is all bass and no
/// shape, and 60 Hz is where a kick drum lives anyway.
const LOWEST_HZ: f64 = 60.0;

/// The highest. A tap at 22 050 Hz cannot show 16 kHz, so the top is clamped to
/// the transform rather than asked for; that clamp is the whole reason this is a
/// function of the rate.
const HIGHEST_HZ: f64 = 16_000.0;

/// How much of a spectrum's natural downward tilt to put back.
///
/// Music falls off with frequency at roughly 3 to 6 dB per octave, and the window
/// below wastes screen space on the quiet part of that. Raising each band by
/// `(f / f_lowest)^TILT` undoes three of those decibels, so a full band is a real
/// shape instead of a spike at the bottom left and a dark pane above it. Measured
/// against pink noise — which falls at exactly 3 dB per octave, so this is the
/// signal the exponent is defined on — a 32-band map comes out flat to within
/// about 6 dB from band 0 to band 31 (`pink_noise_stays_readable_across_the_pane`).
///
/// Not more: past this the display starts inventing energy that is not there.
const TILT: f32 = 0.5;

/// The highest frequency a map at `sample_rate` can show.
///
/// A transform of `window` points resolves `nyquist / (window / 2)` Hz per bin, so a
/// tap slower than 32 kHz cannot show [`HIGHEST_HZ`] at all: the top is pulled below
/// Nyquist by a whole bin, so the last band sits inside the spectrum rather than half
/// outside it. This is the clamp that makes the map a function of the rate.
fn top_hz(sample_rate: f64, window: usize) -> f64 {
    let nyquist = sample_rate / 2.0;
    HIGHEST_HZ
        .min(nyquist - nyquist / window as f64)
        .max(LOWEST_HZ * 1.5)
}

impl BandMap {
    /// The map for a transform of `window` points at `sample_rate` Hz.
    fn new(sample_rate: f64, window: usize) -> Self {
        let nyquist = sample_rate / 2.0;
        let top = top_hz(sample_rate, window);
        let per_band = (top / LOWEST_HZ).log10() / BARS as f64;

        let mut lower = [0u16; BARS];
        let mut upper = [0u16; BARS];
        let mut gain = [1.0f32; BARS];
        let bins = (window / 2) as f64;
        // A band narrower than one bin still gets a bin: its own, the nearest one,
        // rather than an empty range and a bar stuck at zero.
        let bin_of =
            |hz: f64| ((hz / nyquist * bins).round() as i64).clamp(0, window as i64 / 2 - 1);
        for (n, (lo, hi)) in lower.iter_mut().zip(upper.iter_mut()).enumerate() {
            let from = LOWEST_HZ * 10f64.powf(per_band * n as f64);
            let to = LOWEST_HZ * 10f64.powf(per_band * (n + 1) as f64);
            let first = bin_of(from);
            let last = bin_of(to).max(first);
            *lo = first as u16;
            *hi = last as u16;
            // The geometric centre, because the band is a ratio and not a span.
            gain[n] = ((from * to).sqrt() / LOWEST_HZ).powf(TILT as f64) as f32;
        }
        Self { lower, upper, gain }
    }

    /// Fold `bins` of magnitudes into the `BARS` bands.
    ///
    /// The **mean** over each band's bins, not the sum: a band's width in bins grows
    /// from one at the bottom to hundreds at the top, so summing would make the top
    /// of the spectrum tall for no reason other than arithmetic.
    fn bands(&self, bins: &[f64], out: &mut [f32]) {
        for (n, bar) in out.iter_mut().enumerate() {
            *bar = (self.mean(n, bins) as f32) * self.gain[n];
        }
    }

    /// The mean magnitude of band `n`, or zero if `bins` is empty.
    fn mean(&self, n: usize, bins: &[f64]) -> f64 {
        // The ranges come from `new` and are clamped again here, because this is the
        // one place an out-of-range index would be a panic on the worker.
        let last = bins.len().saturating_sub(1);
        let lo = (self.lower[n] as usize).min(last);
        let hi = (self.upper[n] as usize).min(last).max(lo);
        bins[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64
    }
}

// ---------------------------------------------------------------------------
// The analyser
// ---------------------------------------------------------------------------

/// The transform length for `sample_rate`: the power of two nearest `rate / 24`,
/// which is about 42 ms of audio at any rate.
///
/// Long enough that the lowest band's 12 Hz of span covers more than one bin at
/// 48 kHz — a shorter window puts 60 Hz and 72 Hz in the same bin and the bottom of
/// the display cannot separate a kick from a bass line — and short enough that a
/// quarter of a bar's height is still worth drawing at 20 ms a frame.
///
/// A power of two because the transform is radix-2, and derived from the rate
/// because bin `k` of an `N`-point transform means `k * rate / (2N)` Hz. Hard-coding
/// 2048 would put 48 kHz's bands a semitone from where they belong at 44.1 kHz and
/// by a factor of two at 96 kHz.
fn window_len(sample_rate: u32) -> usize {
    // `rate / 24` is 2000 at 48 kHz, and the clamp keeps a 8 kHz telephone tap from
    // asking for a 128-point transform and a 384 kHz one from asking for a 32 768.
    let target = (sample_rate / 24).clamp(512, 8192) as usize;
    target.next_power_of_two()
}

/// One tap's worth of analysis: the transform, the band map, the newest samples,
/// and the scaling between a magnitude and a bar.
///
/// One instance per stream, which is the same rule `cavacore` had and for the same
/// reason — its peak state lives inside the instance — except that here there is
/// exactly one continuous stream and so the state is what smooths the output rather
/// than what corrupts it.
struct Analyser {
    fft: fft::Fft,
    map: BandMap,
    /// The rate the tap reported, verbatim. A transform can only be sized from a
    /// rounded rate, and the number a status line shows should be the one Core Audio
    /// gave rather than a truncation of it.
    reported: f64,
    /// The newest `fft.len()` samples, oldest first.
    recent: Vec<f64>,
    /// `fft.len() / 2 + 1` magnitudes.
    magnitudes: Vec<f64>,
    bars: Vec<f32>,
    scale: BarScale,
}

impl Analyser {
    /// An analyser for a tap running at `sample_rate` Hz.
    ///
    /// The rate comes from the tap's own `AudioStreamBasicDesc` and never from a
    /// constant, and it is checked rather than cast: it is an `f64`, and a
    /// truncation is where a rate of 47 999.5 would quietly become 47 999 and a rate
    /// of NaN would become 0 — which sizes a 512-point transform and answers every
    /// band from the same dozen bins.
    fn new(sample_rate: f64) -> Result<Self, TapError> {
        let rate = if sample_rate.is_finite() && (4_000.0..=384_000.0).contains(&sample_rate) {
            sample_rate.round() as u32
        } else {
            return Err(TapError::BadSampleRate(format!("{sample_rate}")));
        };
        let fft = fft::Fft::new(window_len(rate));
        let bins = fft.len() / 2 + 1;
        Ok(Self {
            map: BandMap::new(sample_rate, fft.len()),
            fft,
            reported: sample_rate,
            recent: Vec::new(),
            magnitudes: vec![0.0; bins],
            bars: vec![0.0; BARS],
            scale: BarScale::new(),
        })
    }

    /// The rate the tap reported.
    fn sample_rate(&self) -> f64 {
        self.reported
    }

    /// The transform length in samples, which is also the depth of the history the
    /// bars are computed from.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.fft.len()
    }

    /// The bins band `n` reads, and the gain applied to it.
    #[cfg(test)]
    fn band(&self, n: usize) -> ((usize, usize), f32) {
        (
            (self.map.lower[n] as usize, self.map.upper[n] as usize),
            self.map.gain[n],
        )
    }

    /// Hand over the audio captured since the last call and update the bars in place.
    ///
    /// Anything older than one window is dropped rather than queued. That is the whole
    /// latency policy: the bars describe the last [`window_len`] samples of what the
    /// tap heard, and a slow tick costs resolution rather than delay.
    ///
    /// **An empty handover means silence**, not "no news". A tap whose client has
    /// paused is not called at all any more, so there is nothing to hand over and
    /// nothing to decay the bars with; keeping the last window would answer the same
    /// spectrum for ever, which is a paused song drawn as though it were playing. So an
    /// empty window is transformed as silence, and the bars reach zero on that tick.
    /// [`SILENT_TICKS`] is what stops one late callback from doing it.
    fn push(&mut self, samples: &[f32]) {
        let len = self.fft.len();
        if samples.is_empty() {
            self.recent.clear();
        } else if samples.len() >= len {
            self.recent.clear();
            self.recent
                .extend(samples[samples.len() - len..].iter().map(|s| f64::from(*s)));
        } else {
            self.recent.extend(samples.iter().map(|s| f64::from(*s)));
            let excess = self.recent.len().saturating_sub(len);
            if excess > 0 {
                self.recent.drain(..excess);
            }
        }
        self.fft.magnitudes(&self.recent, &mut self.magnitudes);
        self.map.bands(&self.magnitudes, &mut self.bars);
        self.scale.scale(&mut self.bars);
    }

    /// The bars as they stand. Always [`BARS`] of them, always in 0..=1.
    fn bars(&self) -> &[f32] {
        &self.bars
    }
}

// ---------------------------------------------------------------------------
// The capture half
// ---------------------------------------------------------------------------

/// What the realtime IO callback writes into.
///
/// The ring is behind atomics, so `&self` is enough and the callback never has to
/// hold the `&mut` it is given.
struct TapIo {
    ring: SampleRing,
    /// Interleaved frames per sample the tap delivers. 1 for the mono mixdown Trak
    /// asks for; carried anyway, so a stereo tap downmixes in one place instead of
    /// interleaving its channels into the spectrum, which is the one mistake a
    /// display must never be handed.
    channels: u32,
    /// Where a stereo tap is downmixed, held here rather than made per callback.
    ///
    /// Only the stereo path touches it, and Trak asks for a mono mixdown, so on the
    /// machine this was built on it stays empty. It is a field anyway: an allocation
    /// on this thread is a dropout, and a `Vec` declared inside the callback would
    /// allocate on every single callback.
    downmix: Vec<f32>,
}

/// The IO proc. It must not allocate, lock, log or panic, because it runs on the
/// thread the audio arrives on.
extern "C" fn on_audio(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input: &cat::AudioBufList<2>,
    _input_time: &cat::AudioTimeStamp,
    _output: &mut cat::AudioBufList<2>,
    _output_time: &cat::AudioTimeStamp,
    io: Option<&mut TapIo>,
) -> cidre::os::Status {
    let Some(io) = io else {
        return cidre::os::Status::default();
    };
    let channels = io.channels.max(1) as usize;
    let TapIo {
        ring,
        channels: _,
        downmix,
    } = &mut *io;
    for buffer in input.buffers.iter().take(input.number_buffers as usize) {
        if buffer.data.is_null() || buffer.data_bytes_size == 0 {
            continue;
        }
        let frames = (buffer.data_bytes_size as usize) / (4 * channels);
        if frames == 0 {
            continue;
        }
        // SAFETY: Core Audio guarantees `data` covers `data_bytes_size` valid bytes
        // for the duration of the callback, and the tap's own
        // `AudioStreamBasicDesc` says those bytes are `frames * channels`
        // interleaved 32-bit floats. `frames` comes from the buffer's own length, so
        // the slice is never longer than what the callback was handed.
        let samples =
            unsafe { std::slice::from_raw_parts(buffer.data as *const f32, frames * channels) };
        if channels == 1 {
            ring.push(samples);
        } else {
            downmix.clear();
            downmix.reserve(frames);
            downmix.extend(
                samples
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
            ring.push(downmix);
        }
    }
    cidre::os::Status::default()
}

/// A live tap, and the three objects that have to be destroyed in the right order
/// when it stops.
///
/// The field order *is* the teardown. Rust drops fields in declaration order, so this
/// reads as the sequence:
///
/// 1. `_started` stops the device and destroys the aggregate device it belonged to.
/// 2. `io` is the buffer the callback wrote into, which no callback runs into any
///    more.
/// 3. `_tap` destroys the tap.
///
/// Getting that order wrong is how an aggregate device is left behind: destroying the
/// tap first leaves the device referencing an object that no longer exists, and then
/// neither drop succeeds. TODO 8.5's done-when is that ten of these leave
/// `system_profiler SPAudioDataType` byte-identical to how it started.
struct TapSession {
    /// Held, never dropped early. See the module docs; the underscore is only because
    /// nothing reads it.
    ///
    /// cidre 0.29 does not re-export `StartedDevice`, so the handle is held as an
    /// erased `Box<dyn Any>`: `Any` has no methods, so nothing can reach through it,
    /// and dropping the box drops the device exactly as dropping the value would —
    /// which is the only thing about the device this needs.
    _started: StartedDevice,
    io: Box<TapIo>,
    _tap: ca::TapGuard,
    /// The Spotify processes the tap was built for, watched by [`TapSession::alive`].
    pids: Vec<i32>,
}

/// A started Core Audio device, kept alive. See `TapSession::_started`.
///
/// `allow(dead_code)` for the field: holding it is the entire job, and the tuple
/// field is otherwise "never read" to the compiler, which is a true thing to say
/// about a guard.
#[allow(dead_code)]
struct StartedDevice(Box<dyn std::any::Any>);

impl Capture for TapSession {
    fn drain(&mut self, out: &mut Vec<f32>) {
        self.io.ring.drain(out, self.io.ring.capacity());
    }

    /// Whether any process the tap was built for still exists.
    ///
    /// A tap whose processes have all exited is not an error to Core Audio: the
    /// device keeps running and the callback simply stops, which from here is
    /// indistinguishable from a pause. So quitting Spotify has to be *noticed*, and
    /// `kill(pid, 0)` is the cheapest question that tells the two apart — it sends
    /// nothing, and `EPERM` still means the process is there.
    fn alive(&self) -> bool {
        self.pids.iter().any(|pid| {
            // SAFETY: signal 0 is the documented existence check: no signal is sent.
            let found = unsafe { kill(*pid, 0) } == 0;
            found || std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
        })
    }
}

/// `EPERM`: the process exists, it just is not ours to signal.
const EPERM: i32 = 1;

/// Core Audio's property addressing, which cidre does not expose for the one
/// property the tap needs: the pid translation.
#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

/// `kAudioObjectSystemObject`.
const SYSTEM_OBJECT: u32 = 1;
/// `kAudioObjectPropertyScopeGlobal`.
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
/// `kAudioHardwarePropertyTranslatePIDToProcessObject`.
const TRANSLATE_PID_TO_PROCESS_OBJECT: u32 = u32::from_be_bytes(*b"id2p");

unsafe extern "C" {
    /// Declared rather than reached through cidre: cidre has no binding for the pid
    /// translation, which *is* the whole of TODO 1.5's finding, and
    /// `AudioObjectGetPropertyData` has had this shape since 10.6.
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const std::ffi::c_void,
        data_size: *mut u32,
        data: *mut std::ffi::c_void,
    ) -> i32;
}

/// `kAudioHardwarePropertyTranslatePIDToProcessObject`: the call the first attempts
/// were missing.
///
/// `CATapDescription.h` documents its include-list argument as "an AudioObjectID of
/// the process object", not a pid. Handing it the pid gives
/// `kAudioHardwareBadObjectError` for any non-empty list and *succeeds* for an empty
/// one, which is the shape of a bug that reads like a permissions problem.
///
/// Returns `0` (`kAudioObjectUnknown`) for a pid that names no Core Audio process.
/// That is a real answer and not an error: a Spotify that has just launched is one,
/// and it is why an empty include list is never built.
fn process_object_for_pid(pid: i32) -> Result<u32, TapError> {
    let qualifier = pid as u32;
    let address = PropertyAddress {
        selector: TRANSLATE_PID_TO_PROCESS_OBJECT,
        scope: SCOPE_GLOBAL,
        element: 0,
    };
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut object: u32 = 0;
    // SAFETY: the qualifier is a `u32` of exactly `size` bytes and the out pointer is
    // a `u32` of exactly `size` bytes, which is what the property asks for; both are
    // touched only within those bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &address,
            std::mem::size_of::<u32>() as u32,
            &qualifier as *const u32 as *const std::ffi::c_void,
            &mut size,
            &mut object as *mut u32 as *mut std::ffi::c_void,
        )
    };
    if status == 0 {
        Ok(object)
    } else {
        Err(TapError::from_status(status))
    }
}

/// Every Core Audio process object id for Spotify's running processes.
///
/// `NSRunningApplication` is a read-only workspace query: it launches nothing, which
/// is COMPAT rule 2, and it is the only reason this needs AppKit.
///
/// The `'prs#'` process-object-list property is deliberately **not** read to
/// enumerate. It returns `'nope'` on the machine this was built on
/// (`docs/AUDIO-TAP.md` §3b), so the pids Trak cares about are translated one at a
/// time and nothing depends on a listing that may never come.
///
/// Returns the object ids and the pids they came from: the ids go into the tap, and
/// the pids are what [`TapSession::alive`] watches.
fn spotify_process_objects() -> Result<(Vec<u32>, Vec<i32>), TapError> {
    let pids: Vec<i32> = ns::RunningApp::with_bundle_id(&ns::String::with_str(SPOTIFY_BUNDLE_ID))
        .iter()
        .map(|app| app.pid())
        .filter(|pid| *pid > 0)
        .collect();
    if pids.is_empty() {
        return Err(TapError::SpotifyNotRunning);
    }
    let (objects, tapped): (Vec<u32>, Vec<i32>) = pids
        .iter()
        .filter_map(|pid| Some((process_object_for_pid(*pid).ok()?, *pid)))
        .filter(|(object, _)| *object != 0)
        .unzip();
    if objects.is_empty() {
        // Running, but not an audio client: it has just launched, or it has no output
        // open. Either way there is nothing to tap yet.
        return Err(TapError::NoAudioProcess);
    }
    Ok((objects, tapped))
}

/// The object id of the output device the aggregate device hangs off.
///
/// The tap's audio arrives on the aggregate device, which has no output and no clock
/// of its own, so it is stacked onto the machine's default output the way any other
/// capture device is.
fn default_output_uid() -> Result<cidre::arc::R<cf::String>, TapError> {
    let device =
        ca::System::default_output_device().map_err(|e| TapError::from_status(e.0.get()))?;
    device.uid().map_err(|e| TapError::from_status(e.0.get()))
}

/// Open a tap on Spotify's processes and start it.
///
/// The order is the tap description, the tap, the aggregate device, the IO proc, the
/// device. Every step after the tap is undone automatically if the next one fails,
/// because each object is owned by a guard from the moment it exists, so a partial
/// start cannot leave a tap or an aggregate device behind.
///
/// Returns the tap and the rate it reported, in that order, because the rate is what
/// the analyser has to be built from.
fn open_tap() -> Result<(TapSession, f64), TapError> {
    let (objects, pids) = spotify_process_objects()?;
    let output_uid = default_output_uid()?;

    let numbers: Vec<_> = objects
        .iter()
        .map(|object| ns::Number::with_u32(*object))
        .collect();
    let processes = ns::Array::from_slice_retained(&numbers);
    // `initMonoMixdownOfProcesses:`, the include-list shape. Mono because the tap
    // mixes anyway and the pane is forty cells wide; include-list rather than
    // global-because-excluded because a global tap carries every other process's
    // audio, Sonar's included, which is what COMPAT rule 3 forbids.
    let description = ca::TapDesc::with_mono_mixdown_of_processes(&processes);
    let tap = description
        .create_process_tap()
        .map_err(|e| TapError::from_status(e.0.get()))?;
    let tap_uid = tap.uid().map_err(|e| TapError::from_status(e.0.get()))?;

    // The rate is read off the tap, before anything is built around it.
    let asbd = tap.asbd().map_err(|e| TapError::from_status(e.0.get()))?;
    if asbd.bits_per_channel != 32 || !asbd.format_flags.contains(cat::AudioFormatFlags::IS_FLOAT) {
        return Err(TapError::UnsupportedFormat(format!(
            "{} ch, {} bit, flags {:?}",
            asbd.channels_per_frame, asbd.bits_per_channel, asbd.format_flags
        )));
    }

    use cidre::core_audio::{aggregate_device_keys as agg, sub_device_keys as sub};
    let sub_device = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output_uid.as_type_ref()]);
    let sub_tap = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[tap_uid.as_type_ref()]);
    let aggregate_desc = cf::DictionaryOf::with_keys_values(
        &[
            agg::is_private(),
            agg::is_stacked(),
            agg::tap_auto_start(),
            agg::name(),
            agg::main_sub_device(),
            agg::uid(),
            agg::sub_device_list(),
            agg::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            // Named so a leftover from a crash is recognisable in
            // `system_profiler SPAudioDataType` rather than looking like someone's
            // hardware.
            cf::str!(c"trak visualizer tap").as_type_ref(),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );
    let aggregate = ca::AggregateDevice::with_desc(&aggregate_desc)
        .map_err(|e| TapError::from_status(e.0.get()))?;

    let mut io = Box::new(TapIo {
        ring: SampleRing::new(RING_SAMPLES),
        channels: asbd.channels_per_frame.max(1),
        downmix: Vec::new(),
    });
    // The IO proc holds `&mut TapIo` for as long as the device runs, so the buffer it
    // writes into is boxed and owned by the session that starts the device. Nothing
    // else ever touches it.
    let io_proc = aggregate
        .create_io_proc_id::<2, 2, TapIo>(on_audio, Some(io.as_mut()))
        .map_err(|e| TapError::from_status(e.0.get()))?;
    // `aggregate` moves in here. `started` is the handle that keeps the IO running,
    // and dropping it is the teardown — see `TapSession`.
    let started = ca::device_start(aggregate, Some(io_proc))
        .map_err(|e| TapError::from_status(e.0.get()))
        .map(|device| StartedDevice(Box::new(device)))?;

    Ok((
        TapSession {
            _started: started,
            io,
            _tap: tap,
            pids,
        },
        asbd.sample_rate,
    ))
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// A tap that is up, as the worker sees it.
///
/// The seam between the lifecycle and Core Audio, so the lifecycle — attach on
/// demand, release when hidden, notice Spotify quitting, reattach when it comes back
/// — runs in tests against a fake with no Spotify, no permission and no device.
/// Not `Send`: a capture is opened, read and dropped on the worker thread and never
/// leaves it, which is also why `TapSession` can hold cidre handles that are not
/// `Send` either.
trait Capture {
    /// Everything delivered since the last call, appended to `out`.
    fn drain(&mut self, out: &mut Vec<f32>);
    /// Whether the processes being tapped still exist.
    fn alive(&self) -> bool;
}

/// What opens a [`Capture`]: Core Audio in the binary, a fake in the tests.
///
/// `Sync` because one backend is shared by every worker a pipeline ever starts, and
/// `&self` because a fake needs to count what it was asked through a shared handle.
trait Backend: Send + Sync + std::fmt::Debug {
    /// Open a capture and say what rate it delivers at.
    fn open(&self) -> Result<(Box<dyn Capture>, f64), TapError>;
}

/// The real backend: a process tap on Spotify.
#[derive(Debug)]
struct CoreAudio;

impl Backend for CoreAudio {
    fn open(&self) -> Result<(Box<dyn Capture>, f64), TapError> {
        let (session, rate) = open_tap()?;
        Ok((Box::new(session), rate))
    }
}

/// The worker's clock. Its own type so a test can run the lifecycle in milliseconds
/// rather than at the real backoff.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// Between analyses while a tap is up.
    tick: Duration,
    /// Before trying again after a failure.
    retry: Duration,
    /// Between checks that Spotify is still there.
    liveness: Duration,
    /// The longest an idle worker sleeps before looking at signals again.
    idle_wait: Duration,
}

impl Timing {
    const REAL: Self = Self {
        tick: TICK,
        retry: RETRY,
        liveness: LIVENESS,
        idle_wait: IDLE_WAIT,
    };
}

/// What the TUI asks of the worker. All cheap, and none worth a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// The visualizer is on screen: attach.
    Run,
    /// The visualizer is hidden, or Spotify has quit: let go of the tap. The thread
    /// stays alive, so coming back is cheap.
    Idle,
    /// Leave. Sent only by `Drop`, and the only thing that ends the thread.
    Stop,
}

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
enum TapEvent {
    /// A tap is up and delivering samples, at this rate.
    Started { sample_rate: f64 },
    /// The tap could not be started, or the process it tapped has gone. Carried even
    /// for a state that is not worth a message, because the state is what decides
    /// the bars.
    Failed(TapError),
    /// The tap was released because it was no longer wanted. Not an error.
    Released,
}

/// The TUI's half of a live tap: the published bars, the command channel, and the
/// thread that owns the capture.
#[derive(Debug)]
struct RealTap {
    bars: Arc<BarCell>,
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl RealTap {
    fn send(&self, command: Command) {
        // The only failure is a worker that has already gone, and the answer is the
        // same either way: there is nothing listening, so the simulated source is what
        // the TUI is already drawing.
        let _ = self.commands.send(command);
    }

    fn spectrum(&self) -> Vec<f32> {
        self.bars.load()
    }
}

impl Drop for RealTap {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            // Joining is the whole of "never leave a tap behind": the worker's
            // capture is dropped before this returns, and dropping it stops the
            // device, destroys the aggregate device and destroys the tap.
            let _ = worker.join();
        }
    }
}

/// The worker thread.
///
/// One loop holding at most one capture and one `Analyser`. Every exit — `Stop`, a
/// signal, a lost TUI — leaves through the same place, so there is one teardown to
/// get right rather than several.
///
/// It only spins at [`TICK`] while a tap is up. Otherwise it blocks on the command
/// channel, waking at most every [`IDLE_WAIT`] to look for a signal or a retry that
/// is due, so a hidden visualizer costs nothing measurable.
fn run(
    backend: Arc<dyn Backend>,
    timing: Timing,
    commands: Receiver<Command>,
    outbox: Sender<TapEvent>,
    bars: Arc<BarCell>,
) {
    let _signals = SignalGuard::install();

    let mut wanted = false;
    let mut next_try: Option<Instant> = None;
    let mut session: Option<(Box<dyn Capture>, Analyser)> = None;
    let mut checked = Instant::now();
    let mut scratch: Vec<f32> = Vec::new();
    let mut quiet: u32 = 0;

    loop {
        let command = if session.is_some() {
            commands.try_recv()
        } else {
            let wait = match (wanted, next_try) {
                (true, Some(at)) => at.saturating_duration_since(Instant::now()),
                (true, None) => Duration::ZERO,
                (false, _) => timing.idle_wait,
            };
            match commands.recv_timeout(wait.min(timing.idle_wait)) {
                Ok(c) => Ok(c),
                Err(RecvTimeoutError::Timeout) => Err(TryRecvError::Empty),
                Err(RecvTimeoutError::Disconnected) => Err(TryRecvError::Disconnected),
            }
        };
        match command {
            Ok(Command::Run) => {
                wanted = true;
                // Coming back on screen is a fresh request: the owner should not wait
                // out a backoff that was counting down while the visualizer was hidden.
                next_try = None;
            }
            Ok(Command::Idle) => {
                wanted = false;
                // Released here rather than at the top of the loop, so the teardown
                // happens on the tick the visualizer was hidden rather than a tick
                // later than it could.
                if session.take().is_some() {
                    let _ = outbox.send(TapEvent::Released);
                }
            }
            Ok(Command::Stop) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        if SignalGuard::shutdown_requested() {
            break;
        }

        if wanted && session.is_none() && next_try.is_none_or(|at| Instant::now() >= at) {
            match attach(backend.as_ref()) {
                Ok((tap, analyser)) => {
                    // The bars start at silence rather than at whatever the last tap
                    // left behind: a reattach mid-song should not draw the previous
                    // track's spectrum for a frame.
                    bars.store(&[0.0; BARS]);
                    let _ = outbox.send(TapEvent::Started {
                        sample_rate: analyser.sample_rate(),
                    });
                    session = Some((tap, analyser));
                    next_try = None;
                    checked = Instant::now();
                    quiet = 0;
                }
                Err(error) => {
                    next_try = Some(Instant::now() + timing.retry);
                    let _ = outbox.send(TapEvent::Failed(error));
                }
            }
        }

        // Spotify quitting is the one way a tap goes away without anybody asking,
        // and it does not say so (see `TapSession::alive`). Checked once a second
        // rather than every tick, because the answer changes once a session at most.
        if session.is_some() && checked.elapsed() >= timing.liveness {
            checked = Instant::now();
            if session.as_ref().is_some_and(|(tap, _)| !tap.alive()) {
                session = None;
                bars.store(&[0.0; BARS]);
                // Waiting rather than released: this is "Spotify went", which is
                // what the retry is for, and the retry is what reattaches the tap
                // when Spotify is launched again.
                next_try = Some(Instant::now() + timing.retry);
                let _ = outbox.send(TapEvent::Failed(TapError::SpotifyNotRunning));
            }
        }

        if let Some((tap, analyser)) = session.as_mut() {
            // Everything the callback has delivered since the last tick, analysed as
            // one window. Anything older than the window is already gone, so the ring
            // cannot end up permanently ahead of the analysis.
            scratch.clear();
            tap.drain(&mut scratch);
            quiet = if scratch.is_empty() { quiet + 1 } else { 0 };
            if !scratch.is_empty() || quiet >= SILENT_TICKS {
                analyser.push(&scratch);
                bars.store(analyser.bars());
            }
            sleep(timing.tick);
        }
    }

    // The teardown, however this loop ended.
    drop(session);

    // And if a signal asked for it, pay it back now that the tap is down. Dropping the
    // session first is the whole ordering: `die_of` diverges, so this must not be left
    // to the drop at the end of the function.
    let signum = SHUTDOWN.load(Ordering::SeqCst);
    if signum != 0 {
        die_of(signum);
    }
}

/// A started tap and the analyser built from the rate that tap reported.
fn attach(backend: &dyn Backend) -> Result<(Box<dyn Capture>, Analyser), TapError> {
    let (tap, sample_rate) = backend.open()?;
    // A rate the analyser refuses must not leave the tap running: this is the one
    // place a started tap is handed back before it is stored, and the `?` here drops
    // it, which is the whole teardown.
    let analyser = Analyser::new(sample_rate)?;
    Ok((tap, analyser))
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// `SIGHUP` and `SIGTERM`: the two that mean "you are being replaced, or asked to
/// stop". `SIGINT` is deliberately left alone — in raw mode a terminal sends ctrl-c as
/// a key and Trak reads it as one, and a CLI that trapped it would stop dying the way
/// a CLI is expected to.
const SIGHUP: i32 = 1;
const SIGTERM: i32 = 15;

/// libc's `signal(2)` handler type. `sigaction` would be the stricter call, but this
/// is one handler that sets one atomic, and `signal` has had this shape on Darwin for
/// thirty years.
type Handler = usize;

/// libc's "default action" sentinel, which is `SIG_DFL` at this type.
const SIG_DFL: Handler = 0;

/// Exit code for a signal that was re-raised but somehow not fatal, which should not
/// happen. 128 + the signal number is what a shell expects for a signalled process.
const EXIT_SIGNALLED: i32 = 128;

unsafe extern "C" {
    fn signal(signum: i32, handler: Handler) -> Handler;
    fn raise(signum: i32) -> i32;
    fn kill(pid: i32, signum: i32) -> i32;
}

/// Set by the handler, read by the worker. A signal handler may touch nothing but a
/// lock-free atomic, so it cannot run the teardown itself; it asks the worker, which
/// is at most one [`TICK`] away.
static SHUTDOWN: AtomicI32 = AtomicI32::new(0);

extern "C" fn note_signal(signum: i32) {
    // `swap` rather than `store`: a second signal while the first is being acted on is
    // a user who is impatient, and the default disposition is put back by then, so
    // this is not even reached twice for the same signal.
    SHUTDOWN.swap(signum, Ordering::SeqCst);
}

/// Installs [`note_signal`] for [`SIGHUP`] and [`SIGTERM`].
///
/// This is the belt to RAII's braces, and the honest size of the belt: it covers a
/// `kill` that arrives while the worker is between ticks, and it cannot cover
/// `SIGKILL` or an abort. Trak's release profile is `panic = "abort"`, so a panic does
/// not unwind and `Drop` never runs — which makes process death a *normal* path rather
/// than a catastrophe, and is why "does a dead process leave a tap behind?" is answered
/// by measurement rather than by hope (`docs/AUDIO-TAP.md` §4).
struct SignalGuard;

impl SignalGuard {
    fn install() -> Self {
        // SAFETY: `note_signal` is `extern "C"` and touches nothing but one atomic,
        // which is all a signal handler is allowed to do. The previous disposition is
        // discarded because the guard is installed once per worker and Trak has no
        // other opinion about these two signals.
        for number in [SIGHUP, SIGTERM] {
            // SAFETY: a function pointer is a valid `signal` handler, and
            // `SignalGuard`'s own `Drop` puts the default back.
            unsafe {
                signal(number, note_signal as *const () as Handler);
            }
        }
        Self
    }

    /// The signal that was asked for, or `0` for none. Read once, so the worker acts
    /// on a signal that arrived rather than on one that arrives while it acts.
    fn shutdown_requested() -> bool {
        SHUTDOWN.load(Ordering::SeqCst) != 0
    }
}

/// Take the default disposition of `signum` back and raise it, so the process dies of
/// the signal it was sent.
///
/// [`note_signal`] swallows the default action, and that swallow is the whole point —
/// it is what buys the worker one tick to drop the tap. But a `kill` Trak does not die
/// of is a worse bug than a leftover aggregate device, so the worker pays the signal
/// back after the teardown is done. This is the ordinary shape of a signal handler
/// that has real work to do: notice, clean up, re-raise.
///
/// Nothing here returns in the normal case, so the `process::exit` is only reached if
/// `raise` was itself delivered and handled, which after putting `SIG_DFL` back is a
/// kernel that disagrees with this code.
fn die_of(signum: i32) -> ! {
    // SAFETY: `SIG_DFL` is libc's own sentinel for "default action", and `raise` is
    // the libc entry point for sending a signal to this process. Reaching `exit` means
    // the signal did not end the process, so stopping is the only honest answer left.
    unsafe {
        signal(signum, SIG_DFL);
        raise(signum);
    }
    std::process::exit(EXIT_SIGNALLED + signum);
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        // Give the signals back, so a Trak that has stopped tapping does not go on
        // swallowing a `kill` for the rest of the run.
        for number in [SIGHUP, SIGTERM] {
            // SAFETY: `SIG_DFL` is libc's own sentinel for "default action".
            unsafe {
                signal(number, SIG_DFL);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The seam the TUI holds
// ---------------------------------------------------------------------------

/// Where the visualizer's numbers are coming from right now.
///
/// Nothing draws this yet: it is here so a notice can say something true, and so a
/// test can assert what the pipeline decided without a tap.
#[derive(Debug, Clone, PartialEq)]
pub enum TapState {
    /// Nobody has asked for the tap, or asked and it has been released.
    Idle,
    /// Asked for, but Spotify is not there yet. The retry is TODO 8.5's "reattach
    /// when Spotify restarts", so this resolves itself.
    WaitingForSpotify,
    /// A tap is up. The rate is kept because it is the number that proves the rate was
    /// read rather than assumed.
    Tapping { sample_rate: f64 },
    /// The tap failed and the simulated source is standing in. The error is kept for
    /// the notice.
    Simulated { reason: Option<TapError> },
    /// `source = "simulated"`. The tap is never attempted, so nothing here can be a
    /// fallback from one.
    Forced,
}

/// One notice per session, decided in one pure function.
///
/// The rules are that a failure the owner can do nothing about is silent, a failure
/// they can act on is said **once**, and a later failure never talks over an earlier
/// one. A visualizer that re-explains itself every two seconds while it retries is a
/// visualizer nobody can look at.
fn outcome(event: &TapEvent, warned: bool) -> (TapState, Option<String>) {
    match event {
        TapEvent::Started { sample_rate } => (
            TapState::Tapping {
                sample_rate: *sample_rate,
            },
            None,
        ),
        TapEvent::Failed(error) => {
            let state = match error {
                TapError::SpotifyNotRunning | TapError::NoAudioProcess => {
                    TapState::WaitingForSpotify
                }
                other => TapState::Simulated {
                    reason: Some(other.clone()),
                },
            };
            let notice = if warned || error.is_quiet() {
                None
            } else {
                error.notice()
            };
            (state, notice)
        }
        TapEvent::Released => (TapState::Idle, None),
    }
}

/// The visualizer's audio source: a Core Audio tap on Spotify when it can be had, and
/// [`SimulatedSource`] when it cannot.
///
/// This is the type the TUI holds in place of a bare `SimulatedSource`.
///
/// - [`new`](Self::new) never blocks, whatever the config says. Every Core Audio call
///   is on a worker, so a machine where the tap takes 200 ms to refuse it costs the
///   first frame nothing.
/// - [`set_wanted`](Self::set_wanted) is the lifecycle TODO 8.5 is about: `true` when
///   the visualizer is on screen, `false` when it is hidden or Spotify has quit.
///   Idempotent and cheap, so the render loop can call it every frame.
/// - [`take_notice`](Self::take_notice) yields the one-line explanation of a fallback,
///   once.
///
/// ```no_run
/// # use trak::audio::AudioPipeline;
/// # use trak::config::VisualizerSource;
/// # use trak::visualizer::AudioSource;
/// let mut viz = AudioPipeline::new(VisualizerSource::Auto);
/// viz.set_track("spotify:track:abc");
/// viz.set_wanted(true); // the visualizer came on screen
/// viz.set_playing(true);
/// viz.set_position(12.0);
/// let _bars = viz.spectrum(); // real audio, or bars that move
/// if let Some(line) = viz.take_notice() { /* one status line */ }
/// viz.set_wanted(false); // and that stops the tap
/// ```
#[derive(Debug)]
pub struct AudioPipeline {
    want: VisualizerSource,
    state: TapState,
    tap: Option<RealTap>,
    /// What the last [`set_wanted`](Self::set_wanted) said, so a call every frame is
    /// a comparison and not a channel send.
    wanted: bool,
    events: Receiver<TapEvent>,
    /// Whether a notice has already been raised this session.
    warned: bool,
    /// A notice produced but not yet collected, so one raised between two frames is
    /// not lost.
    pending_notice: Option<String>,
    simulated: SimulatedSource,
    backend: Arc<dyn Backend>,
    timing: Timing,
}

impl AudioPipeline {
    /// A pipeline for `want`.
    ///
    /// `SimulatedSource` returns immediately and never touches Core Audio. `Auto`
    /// returns immediately too, holding a simulated spectrum until
    /// [`set_wanted`](Self::set_wanted) says the visualizer is on screen.
    pub fn new(want: VisualizerSource) -> Self {
        Self::with_backend(want, Arc::new(CoreAudio), Timing::REAL)
    }

    fn with_backend(want: VisualizerSource, backend: Arc<dyn Backend>, timing: Timing) -> Self {
        Self {
            want,
            state: match want {
                VisualizerSource::Simulated => TapState::Forced,
                VisualizerSource::Auto => TapState::Idle,
            },
            tap: None,
            wanted: false,
            events: channel().1,
            warned: false,
            pending_notice: None,
            simulated: SimulatedSource::new(0),
            backend,
            timing,
        }
    }

    /// Follow `[visualizer] source`, which the settings screen can change live.
    ///
    /// Going to `simulated` drops the worker, and with it any tap, before this
    /// returns: the setting is the owner saying "do not tap", and a tap that outlives
    /// it by a tick is a tap they did not ask for. Going back to `auto` attaches again
    /// on the next [`set_wanted`](Self::set_wanted) that says the pane is visible.
    pub fn set_source(&mut self, want: VisualizerSource) {
        if want == self.want {
            return;
        }
        self.want = want;
        self.tap = None;
        self.wanted = false;
        self.events = channel().1;
        self.state = match want {
            VisualizerSource::Simulated => TapState::Forced,
            VisualizerSource::Auto => TapState::Idle,
        };
    }

    /// Whether the tap should exist right now.
    ///
    /// The TUI calls this once per frame with whatever decides the visualizer is
    /// visible: `true` on screen, `false` hidden or Spotify gone. Going false releases
    /// the tap within one worker tick, which is what keeps a hidden visualizer from
    /// holding an audio device open. Calling it again with the same answer does
    /// nothing at all.
    pub fn set_wanted(&mut self, wanted: bool) {
        if self.want == VisualizerSource::Simulated || wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        match (&self.tap, wanted) {
            (None, true) => {
                let (outbox, events) = channel();
                let (commands, inbox) = channel();
                let bars = Arc::new(BarCell::new());
                let published = Arc::clone(&bars);
                let backend = Arc::clone(&self.backend);
                let timing = self.timing;
                let tap = RealTap {
                    bars,
                    commands,
                    worker: Some(spawn(move || {
                        run(backend, timing, inbox, outbox, published)
                    })),
                };
                // The first `Run` has to be sent here rather than left to the next
                // call: the worker starts with nothing wanted, and a caller that asks
                // once and then leaves it alone would get a live worker and a tap that
                // never attaches.
                tap.send(Command::Run);
                self.tap = Some(tap);
                self.events = events;
                self.state = TapState::Idle;
            }
            (Some(tap), true) => tap.send(Command::Run),
            (Some(tap), false) => tap.send(Command::Idle),
            (None, false) => {}
        }
    }

    /// The real bars, when a tap is up; `None` when the caller should draw its own
    /// simulated ones.
    ///
    /// This is what the TUI reads: it keeps its own [`SimulatedSource`] on the app
    /// state, so all it needs from here is whether there is something better.
    pub fn live_spectrum(&mut self) -> Option<Vec<f32>> {
        self.drain_events();
        match self.state {
            TapState::Tapping { .. } => self.tap.as_ref().map(RealTap::spectrum),
            _ => None,
        }
    }

    /// The one-line explanation of the last fallback, once.
    ///
    /// `None` for every call after the first that had something to say: a notice that
    /// reappears is a notice nobody reads.
    pub fn take_notice(&mut self) -> Option<String> {
        self.pending_notice.take()
    }

    /// Where the bars are coming from.
    pub fn state(&self) -> &TapState {
        &self.state
    }

    /// The tap's rate, when one is up. `None` otherwise, including while it is still
    /// being attached.
    pub fn sample_rate(&self) -> Option<f64> {
        match self.state {
            TapState::Tapping { sample_rate } => Some(sample_rate),
            _ => None,
        }
    }

    /// Point the simulated fallback at a different track, so a switch from real audio
    /// to simulated is a fade rather than a jump.
    pub fn set_track(&mut self, key: &str) {
        self.simulated.set_track(key);
    }

    /// Whether music is playing. Only the simulated source reads it: a tap delivers
    /// silence when Spotify is paused, which is the same answer.
    pub fn set_playing(&mut self, playing: bool) {
        self.simulated.set_playing(playing);
    }

    /// The playback position, as `App::interpolated_position` reports it. Only the
    /// simulated source reads it, for the same reason as the other two: the tap knows
    /// when the music stops on its own.
    pub fn set_position(&mut self, position_secs: f64) {
        self.simulated.set_position(position_secs);
    }

    /// Apply whatever the worker has said since the last frame.
    ///
    /// Non-blocking by construction — `try_iter` and nothing else — because this runs on
    /// the thread that draws the frame. A silent worker costs one atomic load.
    fn drain_events(&mut self) {
        for event in self.events.try_iter() {
            let (state, notice) = outcome(&event, self.warned);
            self.state = state;
            if notice.is_some() {
                self.warned = true;
            }
            if self.pending_notice.is_none() {
                self.pending_notice = notice;
            }
        }
    }
}

impl AudioSource for AudioPipeline {
    fn spectrum(&mut self) -> Vec<f32> {
        self.live_spectrum()
            .unwrap_or_else(|| self.simulated.spectrum())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// One second of a sine at `freq`, as the tap would deliver it.
    fn sine(rate: f64, freq: f64, secs: f64) -> Vec<f32> {
        let n = (rate * secs) as usize;
        (0..n)
            .map(|i| (std::f64::consts::TAU * freq * i as f64 / rate).sin() as f32)
            .collect()
    }

    /// `n` samples of `v`.
    fn constant(n: usize, v: f32) -> Vec<f32> {
        vec![v; n]
    }

    /// Feed `samples` through an analyser in the pieces the worker uses: one
    /// callback's worth, which at 48 kHz and the 20 ms tick is about 960 samples
    /// rather than a whole window.
    fn run_analyser(rate: f64, samples: &[f32]) -> Analyser {
        let mut analyser = Analyser::new(rate).expect("a rate the analyser accepts");
        let tick = (rate / 50.0) as usize;
        for chunk in samples.chunks(tick.max(1)) {
            analyser.push(chunk);
        }
        analyser
    }

    fn loudest(bars: &[f32]) -> f32 {
        bars.iter().copied().fold(0.0, f32::max)
    }

    /// A deterministic noise source with a flat spectrum, for the tests that need a
    /// signal with energy everywhere at once.
    fn white_noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
            })
            .map(|v| v as f32)
            .collect()
    }

    /// A deterministic noise source with a −3 dB/octave slope, which is the signal
    /// the tilt in [`TILT`] is defined against: three summed one-pole filters over
    /// white noise, the usual pink-noise construction.
    fn pink_noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        let white = |s: &mut u64| {
            *s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        };
        let mut b0 = 0.0;
        let mut b1 = 0.0;
        let mut b2 = 0.0;
        (0..n)
            .map(|_| {
                let w = white(&mut s);
                b0 = 0.99765 * b0 + w * 0.0990460;
                b1 = 0.96300 * b1 + w * 0.2965164;
                b2 = 0.57000 * b2 + w * 1.0526913;
                ((b0 + b1 + b2 + w * 0.1848) * 0.15) as f32
            })
            .collect()
    }

    // -- the ring ----------------------------------------------------------

    #[test]
    fn the_ring_returns_what_went_in_and_in_order() {
        let ring = SampleRing::new(16);
        let mut out = Vec::new();
        assert_eq!(ring.drain(&mut out, 16), 0, "an empty ring drains nothing");
        assert!(out.is_empty());

        ring.push(&[1.0, 2.0, 3.0]);
        assert_eq!(ring.available(), 3);
        assert_eq!(ring.drain(&mut out, 16), 3);
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
        assert_eq!(ring.drain(&mut out, 16), 0, "and is empty again");
    }

    #[test]
    fn the_ring_wraps_around_without_losing_the_order() {
        let ring = SampleRing::new(8);
        let mut out = Vec::new();
        // Four samples into an eight-slot ring, four rounds: the write cursor laps
        // the read cursor twice, which is where a masking bug shows up.
        for round in 0..5 {
            let base = round as f32 * 10.0;
            ring.push(&[base, base + 1.0, base + 2.0, base + 3.0]);
            ring.drain(&mut out, 4);
            assert_eq!(
                out,
                vec![base, base + 1.0, base + 2.0, base + 3.0],
                "round {round}"
            );
        }
    }

    #[test]
    fn the_ring_drops_the_oldest_audio_when_it_overflows() {
        let ring = SampleRing::new(4);
        ring.push(&[1.0, 2.0, 3.0]);
        // Three more, with only one slot free.
        ring.push(&[4.0, 5.0, 6.0]);
        let mut out = Vec::new();
        assert_eq!(ring.drain(&mut out, 4), 4);
        assert_eq!(
            out,
            vec![3.0, 4.0, 5.0, 6.0],
            "the newest audio is the audio worth keeping"
        );
    }

    #[test]
    fn the_ring_keeps_only_the_tail_of_a_push_longer_than_itself() {
        let ring = SampleRing::new(4);
        ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        let mut out = Vec::new();
        ring.drain(&mut out, 64);
        assert_eq!(out, vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn a_ring_never_reports_more_than_it_can_hold() {
        // The invariant the two cursors exist to keep. If `push` or `drain` ever let
        // `head - tail` exceed the capacity, the producer starts overwriting samples
        // the consumer has not read and the audio comes out as a buzz.
        let ring = SampleRing::new(8);
        let mut out = Vec::new();
        for round in 0..200 {
            ring.push(&constant(5, round as f32));
            ring.drain(&mut out, 3);
            assert!(ring.available() <= ring.capacity(), "round {round}");
            assert!(out.len() <= 3);
        }
    }

    #[test]
    fn draining_caps_at_the_limit_and_keeps_the_rest() {
        let ring = SampleRing::new(16);
        ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut out = Vec::new();
        assert_eq!(ring.drain(&mut out, 2), 2);
        assert_eq!(out, vec![1.0, 2.0]);
        assert_eq!(ring.available(), 3, "the rest is still waiting");
        assert_eq!(ring.drain(&mut out, 16), 3);
        assert_eq!(out, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn a_ring_tiny_or_odd_sized_is_still_a_ring() {
        // The capacity is rounded up to a power of two because the wrap is a mask. If
        // that ever stopped happening, a size of 3 would divide by three on the audio
        // thread instead.
        for asked in [0, 1, 2, 3, 5, 100, 1_000] {
            let ring = SampleRing::new(asked);
            assert!(ring.capacity().is_power_of_two(), "{asked}");
            ring.push(&constant(asked * 3, 0.5));
            let mut out = Vec::new();
            ring.drain(&mut out, ring.capacity());
            assert!(out.len() <= ring.capacity());
        }
    }

    #[test]
    fn the_ring_can_be_shared_across_threads() {
        // The whole design is that the audio thread and the worker touch disjoint
        // halves; `Sync` is what lets them be the two halves of one object.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SampleRing>();
        assert_send_sync::<BarCell>();
    }

    // -- the published bars ------------------------------------------------

    #[test]
    fn the_bar_cell_round_trips_every_value_a_dsp_can_produce() {
        let cell = BarCell::new();
        let bars: Vec<f32> = (0..BARS)
            .map(|i| match i % 5 {
                0 => 0.0,
                1 => 1.0,
                2 => -0.5,
                3 => f32::NAN,
                _ => f32::INFINITY,
            })
            .collect();
        cell.store(&bars);
        let got = cell.load();
        assert_eq!(got.len(), BARS);
        for (i, (a, b)) in bars.iter().zip(&got).enumerate() {
            // Compared bit for bit: a NaN must come back a NaN and an infinity an
            // infinity, not "some value that is not finite".
            assert_eq!(a.to_bits(), b.to_bits(), "band {i}");
        }
    }

    #[test]
    fn a_fresh_bar_cell_is_silence_and_a_short_store_leaves_the_rest_alone() {
        let cell = BarCell::new();
        assert_eq!(cell.load(), vec![0.0; BARS]);
        cell.store(&[1.0, 1.0]);
        let got = cell.load();
        assert_eq!(&got[..2], &[1.0, 1.0]);
        assert_eq!(&got[2..], &[0.0; BARS - 2]);
    }

    // -- the scaling -------------------------------------------------------

    #[test]
    fn scaling_puts_the_loudest_band_at_full_height() {
        let mut scale = BarScale::new();
        let mut bars = vec![0.0; BARS];
        bars[7] = 0.8;
        scale.scale(&mut bars);
        assert_eq!(bars[7], 1.0, "the peak is full height: {bars:?}");
        assert!(bars[0..7].iter().all(|b| *b == 0.0));
    }

    #[test]
    fn every_scaled_bar_is_a_magnitude() {
        let mut scale = BarScale::new();
        for round in 0..40 {
            let mut bars: Vec<f32> = (0..BARS)
                .map(|i| ((i as f32 + round as f32) * 0.37).sin() * (1.0 + round as f32 * 0.11))
                .collect();
            scale.scale(&mut bars);
            for (i, bar) in bars.iter().enumerate() {
                assert!(
                    bar.is_finite() && (0.0..=1.0).contains(bar),
                    "round {round}, band {i}: {bar}"
                );
            }
        }
    }

    #[test]
    fn scaling_silence_gives_silence() {
        let mut scale = BarScale::new();
        for _ in 0..50 {
            let mut bars = vec![0.0; BARS];
            scale.scale(&mut bars);
            assert_eq!(bars, vec![0.0; BARS]);
        }
    }

    #[test]
    fn a_quiet_frame_falls_back_slowly_rather_than_dropping() {
        // A peak that fell as fast as it rose would make the whole spectrum twitch on
        // every kick drum, which is the thing this scale exists to stop. What matters
        // is the half-life: 8 frames at `TICK` is 160 ms, longer than the band itself.
        let mut scale = BarScale::new();
        let mut loud = vec![0.0; BARS];
        loud[0] = 1.0;
        scale.scale(&mut loud);
        assert_eq!(scale.peak, 1.0);

        let quiet = [0.0; BARS];
        let mut frames = 0;
        while scale.peak > 0.5 && frames < 500 {
            scale.scale(&mut quiet.to_vec());
            frames += 1;
        }
        assert!(
            frames >= 5,
            "the peak halved in {frames} frames, which is a twitch, not a fall"
        );
        assert!(
            frames <= 40,
            "and took {frames} frames, which is too slow to reach silence"
        );
    }

    #[test]
    fn a_loud_frame_eventually_becomes_silence() {
        // A pause has to reach zero, or the visualizer shows a frozen spectrum of a
        // song that stopped.
        let mut scale = BarScale::new();
        let mut loud = vec![0.0; BARS];
        loud[0] = 1.0;
        scale.scale(&mut loud);
        assert_eq!(loud[0], 1.0, "the loud frame is full height to begin with");

        // Every tick gets the same **raw** quiet frame — silence, every band zero, in
        // the units the scaler is given magnitudes in. Handing it back its own output
        // instead would be a state the worker can never be in: `Analyser::push`
        // overwrites every band from the transform before scaling, so a scaled bar is
        // never fed in again. It would also pin the reference for ever, because a
        // full-height bar is louder than the reference it was divided by.
        let quiet = vec![0.0; BARS];
        let mut bars = vec![0.0; BARS];
        let mut frames = 0;
        while scale.peak > 0.01 && frames < 2_000 {
            bars.copy_from_slice(&quiet);
            scale.scale(&mut bars);
            assert_eq!(
                bars,
                vec![0.0; BARS],
                "frame {frames} of silence drew something"
            );
            frames += 1;
        }
        assert!(
            frames <= 200,
            "the reference fell a hundredfold in {frames} frames, which is {} s",
            f64::from(frames) * TICK.as_secs_f64()
        );

        // And the point of letting it fall: a passage quieter than the loud one is
        // visible again rather than a flat line. A reference that hovered at a floor
        // would pass the first half of this and fail here.
        let mut faint = vec![0.0; BARS];
        faint[0] = 0.01;
        scale.scale(&mut faint);
        assert!(faint[0] > 0.9, "and a quiet passage came back: {faint:?}");
    }

    #[test]
    fn a_loud_frame_is_loud_immediately() {
        let mut scale = BarScale::new();
        let mut quiet = vec![0.01; BARS];
        scale.scale(&mut quiet);
        let mut loud = vec![0.0; BARS];
        loud[3] = 0.5;
        scale.scale(&mut loud);
        assert_eq!(loud[3], 1.0, "the attack is instant: {loud:?}");
    }

    #[test]
    fn scaling_survives_the_values_a_dsp_can_hand_over() {
        let junk = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0,
            -0.0,
            1e30,
            0.0,
            0.5,
        ];
        let mut scale = BarScale::new();
        for round in 0..30 {
            let mut bars: Vec<f32> = (0..BARS).map(|i| junk[(i + round) % junk.len()]).collect();
            scale.scale(&mut bars);
            for (i, bar) in bars.iter().enumerate() {
                assert!(
                    bar.is_finite() && (0.0..=1.0).contains(bar),
                    "round {round}, band {i}: {bar}"
                );
            }
        }
    }

    #[test]
    fn a_nan_never_becomes_the_peak() {
        let mut scale = BarScale::new();
        let mut poisoned = vec![f32::NAN; BARS];
        poisoned[1] = 0.25;
        scale.scale(&mut poisoned);
        assert_eq!(
            poisoned[1], 1.0,
            "the real signal still scales: {poisoned:?}"
        );
        assert_eq!(poisoned[0], 0.0, "and the NaN became silence");

        // And a frame that is *only* junk must not poison the peak either, because a
        // peak of infinity would divide every later frame by it. On its own scale the
        // junk leaves nothing behind at all...
        let mut scale = BarScale::new();
        let mut junk = vec![f32::NAN; BARS];
        junk[0] = f32::INFINITY;
        scale.scale(&mut junk);
        assert_eq!(scale.peak, 0.0, "no junk became the peak");
        assert_eq!(junk, vec![0.0; BARS]);

        // ...and it must not overwrite a peak that is already tracking, or every
        // frame after one bad frame would be measured against nothing.
        let mut scale = BarScale::new();
        let mut real = vec![0.0; BARS];
        real[0] = 0.5;
        scale.scale(&mut real);
        scale.scale(&mut junk);
        assert!(
            scale.peak.is_finite() && scale.peak > 0.0 && scale.peak <= 0.5,
            "the real peak survived the junk frame: {}",
            scale.peak
        );
    }

    // -- the transform -----------------------------------------------------

    #[test]
    fn the_window_length_is_always_a_power_of_two() {
        // The transform is radix-2, so this is not a style question: a non-power-of-two
        // length would make `Fft::new` panic on the worker, and `panic = "abort"` in
        // the release profile takes the TUI with it.
        for rate in (1..=384_000).step_by(37) {
            let len = window_len(rate);
            assert!(len.is_power_of_two(), "{rate} Hz -> {len}");
            assert!((512..=8192).contains(&len), "{rate} Hz -> {len}");
        }
    }

    #[test]
    fn the_window_length_is_about_forty_two_milliseconds_at_any_rate() {
        for rate in [8_000u32, 16_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
            let ms = window_len(rate) as f64 / f64::from(rate) * 1000.0;
            assert!(
                (21.0..=85.0).contains(&ms),
                "{rate} Hz -> a {ms:.1} ms window, which resolves neither the low bands \
                 nor a 20 ms frame"
            );
        }
        assert_eq!(window_len(48_000), 2048, "and the tap's own rate gets 2048");
        assert!(
            window_len(96_000) > window_len(48_000),
            "a faster tap needs a longer window for the same span of time"
        );
        assert!(
            window_len(48_000) > window_len(8_000),
            "and a slower one needs a shorter one"
        );
    }

    #[test]
    fn the_transform_finds_a_tone_in_the_bin_its_frequency_names() {
        // The claim every frequency decision rests on: bin `k` of an `N`-point
        // transform means `k * rate / N` Hz. If this is wrong the band map is wrong.
        for rate in [8_000.0f64, 44_100.0, 48_000.0, 96_000.0] {
            let len = window_len(rate as u32);
            let mut fft = fft::Fft::new(len);
            let mut mags = vec![0.0; len / 2 + 1];
            for freq in [200.0f64, 1_000.0, 4_000.0] {
                if freq >= rate / 2.0 {
                    continue;
                }
                let samples: Vec<f64> = (0..len)
                    .map(|i| (std::f64::consts::TAU * freq * i as f64 / rate).sin())
                    .collect();
                fft.magnitudes(&samples, &mut mags);
                let peak = mags.iter().copied().fold(0.0, f64::max);
                let loudest = mags
                    .iter()
                    .position(|m| *m == peak)
                    .expect("the transform filled the magnitudes");
                let bin = (freq / rate * len as f64).round() as usize;
                assert!(
                    loudest.abs_diff(bin) <= 1,
                    "{rate} Hz, {freq} Hz: bin {loudest}, expected {bin}"
                );
                assert!(
                    peak > 1.0,
                    "{rate} Hz, {freq} Hz: peak {peak} is not a tone"
                );
            }
        }
    }

    #[test]
    fn the_transform_keeps_a_constant_signal_in_the_first_two_bins() {
        // DC in bin 0 and Nyquist in bin `len/2`, which is the one property of a
        // real transform that the bar map leans on: it stops at Nyquist and never
        // reads the mirrored half.
        let len = 64;
        let mut fft = fft::Fft::new(len);
        let mut mags = vec![0.0; len / 2 + 1];
        fft.magnitudes(&vec![1.0; len], &mut mags);
        assert_eq!(mags[0].round(), len as f64 / 2.0, "DC: {:?}", mags);

        let alternating: Vec<f64> = (0..len)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        fft.magnitudes(&alternating, &mut mags);
        assert!(
            mags[len / 2] > mags.iter().take(len / 2).copied().fold(0.0, f64::max),
            "Nyquist: {:?}",
            mags
        );
    }

    #[test]
    fn a_frame_shorter_than_the_window_is_zero_padded_and_not_shifted() {
        // The first tick of a capture is shorter than the window, and the first few
        // after it are too. What must not happen is the frame being read from the wrong
        // offset — a window that starts halfway through the audio answers a different
        // spectrum, and no test of the band map would notice. DC settles it: a frame of
        // `n` ones has a DC magnitude of `n * sum(hann) / 2`, so half a window is half
        // of the whole and a shifted read is neither.
        let len = 256;
        let mut fft = fft::Fft::new(len);
        let mut short = vec![0.0; len / 2 + 1];
        let mut full = vec![0.0; len / 2 + 1];
        fft.magnitudes(&vec![1.0; len / 2], &mut short);
        fft.magnitudes(&vec![1.0; len], &mut full);
        assert!(
            (short[0] - full[0] / 2.0).abs() < full[0] * 1e-6,
            "DC is not halved: {} against {}",
            short[0],
            full[0]
        );
        assert!(short.iter().all(|m| m.is_finite() && *m >= 0.0));

        // And one sample is one sample, not a window of them.
        let mut one = vec![0.0; len / 2 + 1];
        fft.magnitudes(&[1.0], &mut one);
        assert!(one[0] < full[0] * 0.01, "DC of a single sample: {}", one[0]);
    }

    #[test]
    fn a_transform_of_an_odd_length_is_refused_rather_than_wrong() {
        // Both directions: the panic is the point, and it has to be reachable only by
        // a caller that ignored `window_len`.
        let refused = std::panic::catch_unwind(|| fft::Fft::new(1000));
        assert!(refused.is_err(), "1000 is not a power of two");
        let refused = std::panic::catch_unwind(|| fft::Fft::new(1));
        assert!(
            refused.is_err(),
            "and a 1-point transform is not a transform"
        );
    }

    // -- the band map ------------------------------------------------------

    #[test]
    fn the_bands_go_up_in_frequency_and_never_skip_a_bin() {
        for rate in [16_000.0f64, 22_050.0, 44_100.0, 48_000.0, 96_000.0] {
            let map = BandMap::new(rate, window_len(rate as u32));
            let mut last = 0usize;
            for n in 0..BARS {
                let (lo, hi) = (map.lower[n] as usize, map.upper[n] as usize);
                assert!(hi >= lo, "{rate} Hz band {n}: {lo}..{hi}");
                assert!(lo >= last, "{rate} Hz band {n}: {lo} < {last}");
                assert!(
                    hi < rate as usize,
                    "{rate} Hz band {n}: {hi} is past Nyquist"
                );
                last = lo;
            }
            assert!(
                map.lower[BARS - 1] > map.lower[0],
                "{rate} Hz: the map does not span anything"
            );
        }
    }

    #[test]
    fn the_top_band_stays_below_nyquist_however_slow_the_tap_is() {
        // The reason the top is a function of the rate: a 22 050 Hz tap cannot show
        // 16 kHz, and asking it to is how a spectrum gets bands that answer nothing.
        for rate in [8_000.0f64, 16_000.0, 22_050.0, 32_000.0, 44_100.0, 48_000.0] {
            let map = BandMap::new(rate, window_len(rate as u32));
            let _ = &map;
            let window = window_len(rate as u32);
            let top = top_hz(rate, window);
            let nyquist = rate / 2.0;
            let bin = nyquist / window as f64;
            assert!(top <= HIGHEST_HZ, "{rate} Hz: top {top} is above the range");
            if HIGHEST_HZ >= nyquist {
                // Slow tap: Nyquist is the binding limit, and it binds within a bin so
                // the last band is inside the spectrum rather than half outside it.
                assert!(top < nyquist, "{rate} Hz: top {top}");
                assert!(
                    top > nyquist - bin * 2.0,
                    "{rate} Hz: top {top} is more than a bin under Nyquist, which \\
                     throws away range the transform has"
                );
            } else {
                assert_eq!(top, HIGHEST_HZ, "{rate} Hz has room for the whole range");
            }
            assert!((map.upper[BARS - 1] as usize) < window / 2, "{rate} Hz");
        }
        // 48 kHz shows the whole range, and a slower tap shows everything it has.
        assert_eq!(top_hz(48_000.0, 2048).round(), 16_000.0);
        assert_eq!(top_hz(192_000.0, 8192).round(), 16_000.0);
        assert!((top_hz(22_050.0, 1024) - 11_014.0).abs() < 1.0);
        assert!((top_hz(8_000.0, 512) - 3_992.0).abs() < 1.0);
    }

    #[test]
    fn the_tilt_rises_with_frequency_and_leaves_the_bottom_alone() {
        let map = BandMap::new(48_000.0, 2048);
        for n in 1..BARS {
            assert!(
                map.gain[n] > map.gain[n - 1],
                "band {n} is quieter than band {} and should be lifted further",
                n - 1
            );
        }
        assert!(
            (0.5..2.0).contains(&map.gain[0]),
            "and the bottom band is not itself boosted much: {}",
            map.gain[0]
        );
    }

    #[test]
    fn pink_noise_stays_readable_across_the_pane() {
        // The claim `TILT` is defined on: a −3 dB/octave source, which the tilt turns
        // into a flat-ish spectrum. Without it the top two thirds of the pane would be
        // dark for every track; with too much of it the top would be brighter than the
        // bottom, which is not what music looks like.
        let analyser = run_analyser(48_000.0, &pink_noise(48_000, 20_260_112));
        let bars = analyser.bars();
        let loud = bars[BARS / 4..BARS * 3 / 4]
            .iter()
            .copied()
            .filter(|b| *b > 0.0)
            .count();
        assert!(
            loud > BARS / 4,
            "the middle of the pane is empty for pink noise: {bars:?}"
        );
        let top_quarter = bars[BARS * 3 / 4..].iter().copied().fold(0.0f32, f32::max);
        assert!(top_quarter > 0.05, "and the top quarter is dark: {bars:?}");
    }

    // -- the analyser ------------------------------------------------------

    #[test]
    fn an_analyser_takes_the_rate_it_is_given_and_not_a_constant() {
        assert_eq!(
            Analyser::new(48_000.0).expect("48k").sample_rate(),
            48_000.0
        );
        assert_eq!(
            Analyser::new(44_100.0).expect("44.1k").sample_rate(),
            44_100.0
        );
        // And the window follows it, which is the half of that which decides where a
        // frequency lands.
        assert_eq!(Analyser::new(48_000.0).expect("48k").len(), 2048);
        assert_eq!(Analyser::new(44_100.0).expect("44.1k").len(), 2048);
        assert_eq!(Analyser::new(96_000.0).expect("96k").len(), 4096);
    }

    #[test]
    fn an_analyser_refuses_a_rate_it_cannot_use() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9, 3_999.0] {
            assert!(
                matches!(Analyser::new(rate), Err(TapError::BadSampleRate(_))),
                "{rate} Hz"
            );
        }
    }

    #[test]
    fn an_analyser_turns_a_silence_into_silence() {
        let analyser = run_analyser(48_000.0, &constant(48_000, 0.0));
        assert_eq!(analyser.bars(), &[0.0; BARS]);
    }

    #[test]
    fn an_analyser_puts_a_bass_sine_at_the_bottom_of_the_spectrum() {
        let analyser = run_analyser(48_000.0, &sine(48_000.0, 80.0, 1.0));
        let bars = analyser.bars();
        assert_eq!(bars.len(), BARS);
        assert!(loudest(bars) > 0.5, "and it drew something: {bars:?}");
        let highest = bars
            .iter()
            .rposition(|b| *b > 0.02)
            .unwrap_or_else(|| panic!("no energy at all: {bars:?}"));
        assert!(
            highest < BARS / 4,
            "80 Hz landed at band {highest} of {BARS}: {bars:?}"
        );
    }

    #[test]
    fn an_analyser_puts_a_treble_sine_at_the_top_of_the_spectrum() {
        let analyser = run_analyser(48_000.0, &sine(48_000.0, 4_000.0, 1.0));
        let bars = analyser.bars();
        let highest = bars
            .iter()
            .rposition(|b| *b > 0.02)
            .unwrap_or_else(|| panic!("no energy at all: {bars:?}"));
        assert!(
            highest > BARS / 2,
            "4 kHz landed at band {highest} of {BARS}: {bars:?}"
        );
    }

    #[test]
    fn every_frequency_between_the_bands_lands_where_it_belongs() {
        // The property the whole band map exists for, and the one TODO 1.6's fixture
        // claimed without checking: sweeping the range, the loudest band only ever
        // moves up. A map that collapsed every frequency onto one bin — which is what
        // `cavacore` 2.0.2 does — fails this at the second frequency.
        let mut previous = 0usize;
        let mut freq = LOWEST_HZ;
        while freq <= 12_000.0 {
            let analyser = run_analyser(48_000.0, &sine(48_000.0, freq, 0.5));
            let bars = analyser.bars();
            let loudest = bars
                .iter()
                .enumerate()
                .fold(
                    (0usize, 0.0f32),
                    |acc, (n, b)| {
                        if *b > acc.1 { (n, *b) } else { acc }
                    },
                )
                .0;
            assert!(
                loudest >= previous,
                "{freq:.0} Hz went backwards, to band {loudest} from {previous}: {bars:?}"
            );
            assert!(bars[loudest] > 0.2, "{freq:.0} Hz drew nothing: {bars:?}");
            previous = loudest;
            freq *= 1.26;
        }
        assert!(
            previous > BARS * 3 / 4,
            "12 kHz still landed in the bottom quarter, at band {previous}"
        );
    }

    #[test]
    fn a_tone_at_the_bottom_of_the_band_range_is_in_the_first_band() {
        // The edges, not just the middle of the range: the first band's upper edge is
        // where a log map is most likely to be off by a bin.
        let analyser = Analyser::new(48_000.0).expect("48k");
        for n in [0usize, 1, 2] {
            let ((lo, hi), _) = analyser.band(n);
            let centre = ((lo + hi) / 2) as f64 * 48_000.0 / analyser.len() as f64;
            assert!(
                centre < 200.0,
                "band {n} centres at {centre} Hz, not the bass"
            );
        }
    }

    #[test]
    fn an_analyser_discriminates_two_frequencies() {
        // TODO 1.6's fixture, through the real path. If the two came out the same
        // shape, the tap would be feeding the display noise.
        let bass = run_analyser(48_000.0, &sine(48_000.0, 80.0, 1.0));
        let mid = run_analyser(48_000.0, &sine(48_000.0, 440.0, 1.0));
        let mean_difference = bass
            .bars()
            .iter()
            .zip(mid.bars())
            .map(|(a, b)| f64::from((a - b).abs()))
            .sum::<f64>()
            / BARS as f64;
        assert!(
            mean_difference > 0.02,
            "80 Hz and 440 Hz look the same: {mean_difference}"
        );
    }

    #[test]
    fn an_analyser_every_band_is_a_magnitude_after_a_second_of_audio() {
        for freq in [60.0, 220.0, 1_000.0, 8_000.0] {
            let analyser = run_analyser(48_000.0, &sine(48_000.0, freq, 1.0));
            for (i, bar) in analyser.bars().iter().enumerate() {
                assert!(
                    bar.is_finite() && (0.0..=1.0).contains(bar),
                    "{freq} Hz, band {i}: {bar}"
                );
            }
        }
    }

    #[test]
    fn an_analyser_survives_more_audio_than_one_window() {
        // A slow tick, or a worker that was descheduled, hands over far more than one
        // window at once. The extra is dropped rather than queued — the bars describe
        // the most recent window — but the frame must still be a whole one, so this
        // asserts the bars describe the *end* of the audio rather than its start.
        let window = window_len(48_000);
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.push(&sine(48_000.0, 80.0, 1.0));
        let low = analyser.bars().iter().rposition(|b| *b > 0.02);

        let mut later = Analyser::new(48_000.0).expect("48k");
        later.push(&sine(48_000.0, 4_000.0, 1.0));
        let high = later.bars().iter().rposition(|b| *b > 0.02);

        let mut both = Analyser::new(48_000.0).expect("48k");
        let mut audio = sine(48_000.0, 80.0, 1.0);
        audio.extend(sine(48_000.0, 4_000.0, 1.0));
        both.push(&audio);
        assert!(window < audio.len(), "and this really is several windows");
        assert!(
            low.unwrap_or(BARS) < BARS / 4,
            "80 Hz alone is in the bass: {low:?}"
        );
        assert!(
            high.unwrap_or(0) > BARS / 2,
            "4 kHz alone is in the treble: {high:?}"
        );
        // One call, both sines, four windows' worth: the treble is last, so the bars
        // must describe the treble and not the bass that came first.
        let seen = both.bars().iter().rposition(|b| *b > 0.02);
        assert_eq!(
            seen, high,
            "the last window is what counts, not the first: {seen:?} against {high:?}"
        );
    }

    #[test]
    fn an_analyser_with_nothing_offered_is_a_no_op() {
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.push(&[]);
        analyser.push(&[]);
        assert_eq!(analyser.bars(), &[0.0; BARS]);
    }

    #[test]
    fn a_tap_that_stops_delivering_becomes_silence_rather_than_freezing() {
        // Measured on this machine: with Spotify **paused** the aggregate device is
        // still running but Core Audio stops invoking the IO proc altogether, so there
        // is no silence to hand over — there is no handover at all. Re-transforming the
        // last window would keep drawing the spectrum of a song that stopped, which is
        // the one thing the release ballistics exist to stop.
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.push(&sine(48_000.0, 80.0, 1.0));
        assert!(loudest(analyser.bars()) > 0.5, "there is something to lose");
        analyser.push(&[]);
        assert_eq!(
            analyser.bars(),
            &[0.0; BARS],
            "one empty handover and the spectrum is gone"
        );

        // And silence is not a transient: it holds, however long the tap stays quiet.
        let held = analyser.scale.peak;
        for _ in 0..200 {
            analyser.push(&[]);
        }
        assert_eq!(analyser.bars(), &[0.0; BARS], "still nothing, 4 s later");
        // The reference decays geometrically rather than reaching zero, which is what
        // lets a quiet passage after a loud one come back into view; what matters is
        // that it is far below where it was, not that it is exactly nothing.
        assert!(
            analyser.scale.peak < held * 0.01,
            "the reference fell {} -> {}",
            held,
            analyser.scale.peak
        );
    }

    #[test]
    fn audio_coming_back_after_a_gap_is_heard_not_lost_to_the_silence() {
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.push(&sine(48_000.0, 80.0, 1.0));
        analyser.push(&[]);
        assert_eq!(analyser.bars(), &[0.0; BARS]);
        analyser.push(&sine(48_000.0, 4_000.0, 1.0));
        let highest = analyser
            .bars()
            .iter()
            .rposition(|b| *b > 0.02)
            .unwrap_or_else(|| panic!("nothing after the gap: {:?}", analyser.bars()));
        assert!(highest > BARS / 2, "resuming put 4 kHz at band {highest}");
    }

    #[test]
    fn an_analyser_keeps_no_longer_history_than_one_window() {
        // The latency policy, as a bound on memory rather than on a comment: a tap
        // that ran for an hour must not have accumulated an hour of samples.
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        for _ in 0..200 {
            analyser.push(&white_noise(960, 7));
        }
        assert!(
            analyser.recent.len() <= analyser.len(),
            "{} samples held for a {} window",
            analyser.recent.len(),
            analyser.len()
        );
        assert!(analyser.bars().iter().any(|b| *b > 0.05));
    }

    // -- errors ------------------------------------------------------------

    #[test]
    fn an_osstatus_is_read_as_its_four_character_code() {
        assert_eq!(fourcc(560_947_818), "'!obj' 0x216F626A");
        assert_eq!(fourcc(-1), "0xFFFFFFFF");
        assert!(!fourcc(0).contains('\''));
    }

    #[test]
    fn a_refusal_is_read_as_a_refusal_and_a_bad_object_is_not() {
        // The trap the spike lost a day to: `!obj` means the id did not map to an
        // object, which is nothing to do with anybody saying no.
        assert!(matches!(
            TapError::from_status(0x2168_6F67_u32 as i32),
            TapError::PermissionDenied(_)
        ));
        assert!(matches!(
            TapError::from_status(560_947_818),
            TapError::CoreAudio(_)
        ));
        assert!(matches!(TapError::from_status(0), TapError::CoreAudio(_)));
    }

    #[test]
    fn a_denial_says_where_to_go_and_nothing_else_does() {
        let line = TapError::PermissionDenied("'!hog'")
            .notice()
            .expect("a line");
        assert!(
            line.contains("Screen & System Audio Recording"),
            "the line has to name the setting: {line}"
        );
        assert!(!line.contains('\n'), "one line: {line}");
        assert!(
            line.contains("simulated"),
            "and it has to say what happens instead: {line}"
        );
    }

    #[test]
    fn the_states_with_nothing_to_do_about_it_are_silent() {
        for error in [TapError::SpotifyNotRunning, TapError::NoAudioProcess] {
            assert_eq!(error.notice(), None, "{error} should say nothing");
            assert!(error.is_quiet(), "{error}");
        }
    }

    #[test]
    fn every_failing_state_raises_exactly_one_line() {
        for error in [
            TapError::NoOutputDevice,
            TapError::PermissionDenied("'!hog'"),
            TapError::CoreAudio("'!obj'".into()),
            TapError::UnsupportedFormat("2 ch, 16 bit".into()),
            TapError::BadSampleRate("nan".into()),
            TapError::Analyser("bars".into()),
        ] {
            let line = error.notice().expect("a line");
            assert!(!line.contains('\n'), "{error}: {line}");
            assert!(line.contains("simulated"), "{error}: {line}");
        }
    }

    // -- the decision the pipeline makes ------------------------------------

    #[test]
    fn a_started_tap_reports_the_rate_it_reported() {
        let (state, notice) = outcome(
            &TapEvent::Started {
                sample_rate: 48_000.0,
            },
            false,
        );
        assert_eq!(
            state,
            TapState::Tapping {
                sample_rate: 48_000.0
            }
        );
        assert_eq!(notice, None, "starting is not news");
    }

    #[test]
    fn spotify_missing_is_a_waiting_state_and_not_a_message() {
        for error in [TapError::SpotifyNotRunning, TapError::NoAudioProcess] {
            let (state, notice) = outcome(&TapEvent::Failed(error.clone()), false);
            assert_eq!(state, TapState::WaitingForSpotify, "{error}");
            assert_eq!(notice, None, "{error}");
        }
    }

    #[test]
    fn a_refusal_is_simulated_bars_and_one_line() {
        let (state, notice) = outcome(
            &TapEvent::Failed(TapError::PermissionDenied("'!hog'")),
            false,
        );
        assert!(matches!(state, TapState::Simulated { .. }));
        assert!(notice.is_some());
    }

    #[test]
    fn a_failure_is_only_ever_explained_once() {
        let event = TapEvent::Failed(TapError::CoreAudio("'!obj'".into()));
        let (_, first) = outcome(&event, false);
        assert!(first.is_some(), "the first one is said");
        let (_, second) = outcome(&event, true);
        assert_eq!(second, None, "and then it is never said again");
    }

    #[test]
    fn releasing_the_tap_goes_back_to_idle_and_says_nothing() {
        let (state, notice) = outcome(&TapEvent::Released, false);
        assert_eq!(state, TapState::Idle);
        assert_eq!(notice, None);
    }

    // -- the pipeline ------------------------------------------------------

    #[test]
    fn a_forced_simulated_source_never_starts_a_thread_or_a_tap() {
        // CI has no Spotify, no audio device and no permission, so this is the only
        // configuration a test suite can exercise end to end. It has to be the one
        // that provably touches nothing.
        let mut viz = AudioPipeline::new(VisualizerSource::Simulated);
        viz.set_wanted(true);
        viz.set_wanted(true);
        viz.set_track("spotify:track:abc");
        viz.set_playing(true);
        viz.set_position(30.0);
        assert_eq!(*viz.state(), TapState::Forced);
        for _ in 0..30 {
            let bars = viz.spectrum();
            assert_eq!(bars.len(), BARS);
            for (i, bar) in bars.iter().enumerate() {
                assert!(
                    bar.is_finite() && (0.0..=1.0).contains(bar),
                    "band {i}: {bar}"
                );
            }
        }
        assert_eq!(viz.take_notice(), None, "nothing failed, so nothing to say");
        viz.set_wanted(false);
    }

    #[test]
    fn a_pipeline_starts_idle_and_holds_nothing_until_it_is_wanted() {
        // Constructing it must not touch Core Audio, so this is safe in CI.
        let viz = AudioPipeline::new(VisualizerSource::Auto);
        assert_eq!(*viz.state(), TapState::Idle);
        assert_eq!(viz.sample_rate(), None);
    }

    // -- the lifecycle, against a fake Core Audio ----------------------------

    /// The machine as the fake backend sees it: whether Spotify is running, whether
    /// macOS says no, and how many captures exist right now. `live` is the fake's
    /// `system_profiler`: it goes up on open and down on drop, so "nothing left
    /// behind" is a number that has to come back to zero.
    #[derive(Debug, Default)]
    struct World {
        spotify: AtomicBool,
        deny: AtomicBool,
        opened: AtomicUsize,
        live: AtomicUsize,
    }

    #[derive(Debug)]
    struct Fake(Arc<World>);

    struct FakeCapture(Arc<World>);

    impl Capture for FakeCapture {
        fn drain(&mut self, out: &mut Vec<f32>) {
            // A loud low tone, a tick's worth at a time, while Spotify is there.
            if self.0.spotify.load(Ordering::SeqCst) {
                out.extend(sine(48_000.0, 80.0, 0.02));
            }
        }

        fn alive(&self) -> bool {
            self.0.spotify.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeCapture {
        fn drop(&mut self) {
            self.0.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Backend for Fake {
        fn open(&self) -> Result<(Box<dyn Capture>, f64), TapError> {
            if !self.0.spotify.load(Ordering::SeqCst) {
                return Err(TapError::SpotifyNotRunning);
            }
            if self.0.deny.load(Ordering::SeqCst) {
                return Err(TapError::PermissionDenied("'!hog'"));
            }
            self.0.opened.fetch_add(1, Ordering::SeqCst);
            self.0.live.fetch_add(1, Ordering::SeqCst);
            Ok((Box::new(FakeCapture(Arc::clone(&self.0))), 48_000.0))
        }
    }

    const FAST: Timing = Timing {
        tick: Duration::from_millis(2),
        retry: Duration::from_millis(10),
        liveness: Duration::from_millis(5),
        idle_wait: Duration::from_millis(5),
    };

    fn fake(want: VisualizerSource) -> (AudioPipeline, Arc<World>) {
        let world = Arc::new(World::default());
        world.spotify.store(true, Ordering::SeqCst);
        let viz = AudioPipeline::with_backend(want, Arc::new(Fake(Arc::clone(&world))), FAST);
        (viz, world)
    }

    /// Read frames the way the TUI does until `done` holds, or fail after 2 s. Every
    /// notice that came up on the way is returned, so a test can count them.
    fn frames_until(viz: &mut AudioPipeline, done: impl Fn(&AudioPipeline) -> bool) -> Vec<String> {
        let mut notices = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let _ = viz.live_spectrum();
            notices.extend(viz.take_notice());
            if done(viz) {
                return notices;
            }
            assert!(Instant::now() < deadline, "stuck in {:?}", viz.state());
            sleep(Duration::from_millis(1));
        }
    }

    fn tapping(viz: &AudioPipeline) -> bool {
        matches!(viz.state(), TapState::Tapping { .. })
    }

    #[test]
    fn the_tap_starts_only_when_the_visualizer_is_on_screen() {
        let (mut viz, world) = fake(VisualizerSource::Auto);
        sleep(Duration::from_millis(30));
        let _ = viz.live_spectrum();
        assert_eq!(world.opened.load(Ordering::SeqCst), 0, "nobody asked yet");
        assert_eq!(viz.live_spectrum(), None);

        viz.set_wanted(true);
        frames_until(&mut viz, tapping);
        assert_eq!(
            viz.sample_rate(),
            Some(48_000.0),
            "the rate the tap reported"
        );
        assert_eq!(world.live.load(Ordering::SeqCst), 1);
        frames_until(&mut viz, |v| {
            v.tap.as_ref().is_some_and(|t| loudest(&t.spectrum()) > 0.5)
        });
    }

    #[test]
    fn asking_every_frame_opens_one_tap() {
        // The TUI says "wanted" thirty times a second. Only the first of those is news.
        let (mut viz, world) = fake(VisualizerSource::Auto);
        for _ in 0..100 {
            viz.set_wanted(true);
            let _ = viz.live_spectrum();
        }
        frames_until(&mut viz, tapping);
        sleep(Duration::from_millis(30));
        assert_eq!(world.opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn hiding_the_visualizer_releases_the_tap_and_showing_it_reattaches() {
        let (mut viz, world) = fake(VisualizerSource::Auto);
        viz.set_wanted(true);
        frames_until(&mut viz, tapping);

        viz.set_wanted(false);
        frames_until(&mut viz, |v| *v.state() == TapState::Idle);
        assert_eq!(
            world.live.load(Ordering::SeqCst),
            0,
            "a hidden pane holds no tap"
        );
        assert_eq!(viz.live_spectrum(), None, "and draws the simulated bars");

        viz.set_wanted(true);
        frames_until(&mut viz, tapping);
        assert_eq!(world.opened.load(Ordering::SeqCst), 2);
        assert_eq!(world.live.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spotify_quitting_drops_the_tap_and_relaunching_it_reattaches() {
        let (mut viz, world) = fake(VisualizerSource::Auto);
        viz.set_wanted(true);
        frames_until(&mut viz, tapping);

        world.spotify.store(false, Ordering::SeqCst);
        let notices = frames_until(&mut viz, |v| *v.state() == TapState::WaitingForSpotify);
        assert_eq!(
            world.live.load(Ordering::SeqCst),
            0,
            "a tap on nothing is let go"
        );
        assert_eq!(viz.live_spectrum(), None);
        // Several retries happen while it is gone, and none of them is news.
        sleep(Duration::from_millis(50));
        let _ = viz.live_spectrum();
        assert!(notices.is_empty() && viz.take_notice().is_none());
        assert_eq!(
            world.opened.load(Ordering::SeqCst),
            1,
            "nothing to tap, nothing opened"
        );

        world.spotify.store(true, Ordering::SeqCst);
        frames_until(&mut viz, tapping);
        assert_eq!(
            world.opened.load(Ordering::SeqCst),
            2,
            "and it came back on its own"
        );
    }

    #[test]
    fn a_denied_tap_falls_back_to_simulated_bars_and_says_so_once() {
        let (mut viz, world) = fake(VisualizerSource::Auto);
        world.deny.store(true, Ordering::SeqCst);
        viz.set_track("spotify:track:abc");
        viz.set_playing(true);
        viz.set_position(30.0);
        viz.set_wanted(true);

        let mut notices = frames_until(&mut viz, |v| {
            matches!(v.state(), TapState::Simulated { .. })
        });
        // The worker keeps retrying every `retry` — that is how a grant made with trak
        // open is picked up — and every retry fails the same way.
        for _ in 0..3 {
            sleep(Duration::from_millis(40));
            notices.extend(viz.take_notice());
            let _ = viz.live_spectrum();
            notices.extend(viz.take_notice());
            viz.set_wanted(false);
            viz.set_wanted(true);
        }
        assert_eq!(
            notices.len(),
            1,
            "one toast, not one per retry: {notices:?}"
        );
        assert!(
            notices[0].contains("Screen & System Audio Recording"),
            "{}",
            notices[0]
        );
        assert_eq!(world.live.load(Ordering::SeqCst), 0);

        // And the bars are the simulated ones: moving, never the tap's.
        assert_eq!(viz.live_spectrum(), None);
        let frames: Vec<Vec<f32>> = (0..10)
            .map(|i| {
                viz.set_position(30.0 + f64::from(i) * 0.1);
                viz.spectrum()
            })
            .collect();
        assert!(
            frames.iter().any(|f| loudest(f) > 0.0),
            "simulated bars move"
        );

        // Granting it with trak still open is picked up by the retry.
        world.deny.store(false, Ordering::SeqCst);
        viz.set_wanted(true);
        frames_until(&mut viz, tapping);
    }

    #[test]
    fn the_simulated_setting_never_taps_and_switching_to_it_lets_go_at_once() {
        let (mut viz, world) = fake(VisualizerSource::Simulated);
        viz.set_wanted(true);
        sleep(Duration::from_millis(30));
        assert_eq!(*viz.state(), TapState::Forced);
        assert_eq!(viz.live_spectrum(), None);
        assert_eq!(world.opened.load(Ordering::SeqCst), 0);

        viz.set_source(VisualizerSource::Auto);
        viz.set_wanted(true);
        frames_until(&mut viz, tapping);

        // Changed in the settings screen with the tap up: down before the call returns.
        viz.set_source(VisualizerSource::Simulated);
        assert_eq!(world.live.load(Ordering::SeqCst), 0);
        assert_eq!(*viz.state(), TapState::Forced);
        viz.set_wanted(true);
        sleep(Duration::from_millis(30));
        assert_eq!(world.opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_the_pipeline_takes_the_tap_down_before_it_returns() {
        let (mut viz, world) = fake(VisualizerSource::Auto);
        viz.set_wanted(true);
        frames_until(&mut viz, tapping);
        drop(viz);
        assert_eq!(world.live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_pipeline_can_be_moved_to_another_thread() {
        // `AudioSource: Send` is the whole reason the tap can live on a worker.
        fn assert_send<T: Send>() {}
        assert_send::<AudioPipeline>();
        assert_send::<Box<dyn AudioSource>>();
    }

    #[test]
    fn a_pipeline_never_hands_the_renderers_anything_unusable() {
        // Whatever the state, every frame is `BARS` magnitudes. A tap that is up but
        // has not delivered yet is silence, not a short slice.
        for state in [
            TapState::Idle,
            TapState::WaitingForSpotify,
            TapState::Forced,
            TapState::Tapping {
                sample_rate: 48_000.0,
            },
            TapState::Simulated { reason: None },
        ] {
            let mut viz = AudioPipeline::new(VisualizerSource::Simulated);
            viz.state = state.clone();
            let bars = viz.spectrum();
            assert_eq!(bars.len(), BARS, "{state:?}");
        }
    }
}
