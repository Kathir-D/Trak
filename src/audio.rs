//! Real audio for the visualizer: a Core Audio **process tap on Spotify only**,
//! a lock-free ring buffer, and `cavacore` on a worker thread (TODO 8.3, 8.5).
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
//!   ring, runs one `cavacore` pass per tick and publishes band magnitudes the
//!   render thread can read without waiting for anything.
//!
//! Everything above that is [`AudioPipeline`], which decides between the tap and
//! [`SimulatedSource`]. It is created with [`AudioPipeline::new`] and never
//! blocks, because every Core Audio call happens on the worker.
//!
//! `docs/AUDIO-TAP.md` is the write-up of the experiments this is built on, and
//! every trap recorded there is load-bearing here. The four that cost the most:
//!
//! 1. **The rate is read, never assumed.** `cavacore` believes whatever sample
//!    rate it is given, so the tap's own `asbd.sample_rate` configures it — and
//!    [`max_input_samples`] is the other half of that, the half that is a panic
//!    rather than a wrong picture.
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
//! Fallback is the product decision, not an error path: a machine that denies the
//! tap still gets bars that move with the music, and the first failure raises
//! exactly one line saying what to grant ([`TapError::notice`]).

use std::cell::UnsafeCell;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, Instant};

use cavacore::{Cava, CavaBuilder, Channels, SampleRate as CavaSampleRate};
use cidre::{cat, cf, core_audio as ca, ns};

use super::{AudioSource, BARS, SimulatedSource};
use crate::config::VisualizerSource;

/// How long the worker waits between analyses. The frame tick reads the bars at
/// 30 fps (TODO 8.4), so much slower than this is wasted work and much faster is
/// a wake-up nobody asked for. 20 ms puts the analysis just ahead of the frame it
/// feeds, and is also the resolution [`RELEASE`] is measured in.
const TICK: Duration = Duration::from_millis(20);

/// How long before trying the tap again after a failure.
///
/// This is also how "reattach when Spotify restarts" (TODO 8.5) is delivered: a
/// tap cannot outlive the process it taps, so quitting Spotify has to take the tap
/// down and putting it back has to be somebody's job. Two seconds is short enough
/// that a restart is invisible, and long enough that a machine where the tap will
/// never work is not spinning.
const RETRY: Duration = Duration::from_secs(2);

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

    /// The tap's rate is not one `cavacore` accepts.
    #[error("the tap runs at {0} Hz, which is not a rate the analyser accepts")]
    BadSampleRate(String),

    /// `cavacore` refused the configuration the tap's format implies.
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
        let used = head.wrapping_sub(self.tail.0.load(Ordering::Acquire)).min(cap);
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

/// Turns `cavacore`'s output into the 0..=1 magnitudes the renderers expect.
///
/// `cavacore`'s bars are **not** normalised (TODO 1.6): with autosens they ramp from
/// near zero over about a second, and their absolute size depends on how loud the
/// master output happens to be. Without this, a bar's height would say how long the
/// tap had been listening rather than what was playing.
///
/// One recent peak: up instantly, down slowly. Up instantly because a loud frame
/// should look loud in this frame; down slowly because a peak that fell as fast as
/// it rose would make the whole spectrum twitch on every kick drum. At [`TICK`] and
/// [`RELEASE`] the fall is about a second and a quarter, longer than a bar's own
/// rise and fall.
struct BarScale {
    peak: f32,
}

/// How much of the tracked peak a frame leaves behind: a half-life of about eight
/// frames, so 160 ms. Long enough that a kick drum does not make the whole spectrum
/// twitch, short enough that a pause is silence in about two seconds.
const RELEASE: f32 = 0.92;

/// Below this the answer is silence whatever the input says.
///
/// **Measured**, not chosen: a 48 kHz `cavacore` answers a bass sine with about
/// 6e-5 and a 2 kHz one with about 5e-7, so real music lives above 1e-7 and the
/// floor goes below all of it. Above ~2 kHz the crate answers 1e-10 to 1e-11 for
/// *every* frequency — that is the numerical floor of its FFT, not music — and
/// normalising by a peak that small would draw that floor as a full-height
/// triangle. Putting the floor here is what makes the quiet top of the spectrum
/// read as silence instead of as noise (`docs/AUDIO-TAP.md` §4).
const SILENCE: f32 = 1e-9;

impl BarScale {
    fn new() -> Self {
        Self { peak: SILENCE }
    }

    /// Normalise `bars` in place.
    fn scale(&mut self, bars: &mut [f32]) {
        // A NaN or an infinity from the DSP must not become the peak: one poisoned
        // value in a `max` would then divide every other bar by nonsense.
        let loudest = bars
            .iter()
            .filter(|v| v.is_finite())
            .copied()
            .fold(0.0f32, f32::max)
            .max(0.0);
        self.peak = if loudest >= self.peak {
            loudest
        } else {
            (self.peak * RELEASE).max(loudest)
        };
        if self.peak <= SILENCE {
            self.peak = SILENCE;
            bars.fill(0.0);
            return;
        }
        for bar in bars.iter_mut() {
            *bar = if bar.is_finite() && *bar > 0.0 {
                (*bar / self.peak).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }
}

// ---------------------------------------------------------------------------
// The analyser
// ---------------------------------------------------------------------------

/// How many samples `cavacore` accepts in one `execute` at `sample_rate`.
///
/// `Cava::execute` copies its argument into an internal buffer of
/// `treble_buffer_size * 8` samples and shifts it with
/// `copy_within(..len - input.len(), input.len())`, so an input longer than that
/// **panics** — and `Cava` does not publish the length. The size is a function of the
/// sample rate alone (`cavacore`'s `compute_treble_buffer_size`, times eight for the
/// bass FFT), so it is reproduced here rather than guessed, and
/// `the_window_bound_is_cavacores_own` pins it against the crate.
///
/// Getting this wrong is not a wrong picture, it is a dead visualizer: a panic on the
/// worker, with `panic = "abort"` in the release profile, takes the TUI with it.
fn max_input_samples(sample_rate: u32) -> usize {
    let factor = if sample_rate <= 8_125 {
        1
    } else if sample_rate <= 16_250 {
        2
    } else if sample_rate <= 32_500 {
        4
    } else if sample_rate <= 75_000 {
        8
    } else if sample_rate <= 150_000 {
        16
    } else if sample_rate <= 300_000 {
        32
    } else {
        64
    };
    factor * 128 * 8
}

/// The frequency window `cavacore` is asked for: TODO 1.6's measured 60 Hz to 16
/// kHz, clamped so a tap slower than 32 kHz cannot ask for its own Nyquist.
/// `cavacore` rejects a range whose end is above half the sample rate, which is the
/// one way a perfectly good tap rate can still fail to configure.
fn frequency_top(sample_rate: u32) -> u32 {
    (16_000u32).min(sample_rate / 2).max(120)
}

/// One tap's worth of analysis: the `cavacore` instance, the samples waiting for it,
/// and the scaling between its output and a magnitude.
///
/// One instance per stream, which is not a style choice. `cavacore` carries peak and
/// autosens state inside the instance, and TODO 1.6's spike reported an *identical*
/// spectrum for an 80 Hz sine and a 440 Hz sine because it reused one across signals.
/// Here there is exactly one continuous stream, so the state is what smooths the
/// output rather than what corrupts it.
struct Analyser {
    cava: Cava,
    /// The rate the tap reported, verbatim. A `Cava` can only be built from a rounded
    /// one, and the number a status line shows should be the one Core Audio gave
    /// rather than a truncation of it.
    reported: f64,
    rate: u32,
    /// Samples read from the ring that have not been analysed yet.
    pending: Vec<f64>,
    /// A contiguous run of the front of `pending`, because `execute` borrows the
    /// analyser mutably and so cannot be handed a slice of its own field.
    chunk: Vec<f64>,
    raw: Vec<f64>,
    scaled: Vec<f32>,
    scale: BarScale,
}

impl Analyser {
    /// An analyser for a tap running at `sample_rate` Hz.
    ///
    /// The rate comes from the tap's own `AudioStreamBasicDesc` and never from a
    /// constant: `cavacore` trusts whatever it is told, and 44.1 kHz's maths over 48
    /// kHz's audio puts every band in the wrong place.
    fn new(sample_rate: f64) -> Result<Self, TapError> {
        // `asbd.sample_rate` is an `f64` and `cavacore` wants a `u32`, so the
        // truncation is where a rate of 47 999.5 would become 47 999 and a rate of
        // NaN would become 0. Both are checked rather than cast.
        let rate = if sample_rate.is_finite() && (1.0..=384_000.0).contains(&sample_rate) {
            sample_rate.round() as u32
        } else {
            return Err(TapError::BadSampleRate(format!("{sample_rate}")));
        };
        let top = frequency_top(rate);
        let cava = CavaBuilder::default()
            .bars_per_channel(NonZeroUsize::new(BARS).expect("BARS is not zero"))
            .sample_rate(CavaSampleRate::new(rate).expect("rate is in range"))
            .audio_channels(Channels::Mono)
            .enable_autosens(true)
            .noise_reduction(0.2)
            .frequency_range(
                NonZeroU32::new(60).expect("60 is not zero")
                    ..NonZeroU32::new(top).expect("the top is not zero"),
            )
            .build()
            .map_err(|errors| {
                TapError::Analyser(
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            })?;
        Ok(Self {
            cava,
            reported: sample_rate,
            rate,
            pending: Vec::new(),
            chunk: Vec::new(),
            raw: vec![0.0; BARS],
            scaled: vec![0.0; BARS],
            scale: BarScale::new(),
        })
    }

    /// The rate the tap reported.
    fn sample_rate(&self) -> f64 {
        self.reported
    }

    /// The most samples one `execute` may be given at this rate.
    fn limit(&self) -> usize {
        max_input_samples(self.rate)
    }

    /// Hand over samples from the ring. More than one window's worth is kept for the
    /// next tick rather than dropped, so a slow tick costs resolution rather than
    /// audio.
    fn offer(&mut self, samples: &[f32]) {
        self.pending.extend(samples.iter().map(|s| f64::from(*s)));
    }

    /// Analyse whatever has been offered, in window-sized pieces, and update the
    /// magnitudes in place.
    fn advance(&mut self) {
        let take = self.pending.len().min(self.limit());
        if take == 0 {
            return;
        }
        self.chunk.clear();
        self.chunk.extend_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        self.cava.execute(&self.chunk, &mut self.raw);
        for (bar, value) in self.scaled.iter_mut().zip(self.raw.iter()) {
            *bar = *value as f32;
        }
        self.scale.scale(&mut self.scaled);
    }

    /// The magnitudes as they stand. Always [`BARS`] of them, always in 0..=1.
    fn bars(&self) -> &[f32] {
        &self.scaled
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
    let mut downmix: Vec<f32> = Vec::new();
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
            io.ring.push(samples);
        } else {
            downmix.clear();
            downmix.reserve(frames);
            downmix.extend(
                samples
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
            io.ring.push(&downmix);
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
}

/// A started Core Audio device, kept alive. See `TapSession::_started`.
///
/// `allow(dead_code)` for the field: holding it is the entire job, and the tuple
/// field is otherwise "never read" to the compiler, which is a true thing to say
/// about a guard.
#[allow(dead_code)]
struct StartedDevice(Box<dyn std::any::Any>);

impl TapSession {
    /// Take up to `max` samples the callback has delivered.
    fn drain(&self, out: &mut Vec<f32>, max: usize) -> usize {
        self.io.ring.drain(out, max)
    }
}

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
fn spotify_process_objects() -> Result<Vec<u32>, TapError> {
    let pids: Vec<i32> = ns::RunningApp::with_bundle_id(&ns::String::with_str(SPOTIFY_BUNDLE_ID))
        .iter()
        .map(|app| app.pid())
        .filter(|pid| *pid > 0)
        .collect();
    if pids.is_empty() {
        return Err(TapError::SpotifyNotRunning);
    }
    let objects: Vec<u32> = pids
        .iter()
        .filter_map(|pid| process_object_for_pid(*pid).ok())
        .filter(|object| *object != 0)
        .collect();
    if objects.is_empty() {
        // Running, but not an audio client: it has just launched, or it has no output
        // open. Either way there is nothing to tap yet.
        return Err(TapError::NoAudioProcess);
    }
    Ok(objects)
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
    let objects = spotify_process_objects()?;
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
    let sub_device =
        cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output_uid.as_type_ref()]);
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
        },
        asbd.sample_rate,
    ))
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// What the TUI asks of the worker. Both are cheap and neither is worth a lock.
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
    /// The tap could not be started. Carried even for a state that is not worth a
    /// message, because the state is what the TUI draws.
    Failed(TapError),
    /// The tap was released. Not an error: it is what hiding the visualizer and
    /// quitting Spotify both look like from in here.
    Released,
}

/// The TUI's half of a live tap: the published bars, the command channel, and the
/// thread that owns the `TapSession`.
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
            // `TapSession` is dropped before this returns, and dropping it stops the
            // device, destroys the aggregate device and destroys the tap.
            let _ = worker.join();
        }
    }
}

/// The worker thread.
///
/// One loop, one tick long, holding at most one `TapSession` and one `Analyser`.
/// Every exit — `Stop`, a signal, a lost TUI — leaves through the same place, so
/// there is one teardown to get right rather than several.
fn run(commands: Receiver<Command>, outbox: Sender<TapEvent>, bars: Arc<BarCell>) {
    let _signals = SignalGuard::install();

    let mut wanted = false;
    let mut next_try: Option<Instant> = None;
    let mut session: Option<(TapSession, Analyser)> = None;
    let mut scratch: Vec<f32> = Vec::new();

    loop {
        match commands.try_recv() {
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
            match attach() {
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
                }
                Err(error) => {
                    next_try = Some(Instant::now() + RETRY);
                    let _ = outbox.send(TapEvent::Failed(error));
                }
            }
        }

        if let Some((tap, analyser)) = session.as_mut() {
            // One window per tick, which is a quarter of the ring, so the ring cannot
            // end up permanently ahead of the analysis.
            scratch.clear();
            if tap.drain(&mut scratch, analyser.limit()) > 0 {
                analyser.offer(&scratch);
                analyser.advance();
            }
            bars.store(analyser.bars());
        }

        sleep(TICK);
    }

    // The teardown, however this loop ended.
    drop(session);
}

/// A started tap and the analyser built from the rate that tap reported.
fn attach() -> Result<(TapSession, Analyser), TapError> {
    let (tap, sample_rate) = open_tap()?;
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

unsafe extern "C" {
    fn signal(signum: i32, handler: Handler) -> Handler;
}

/// Set by the handler, read by the worker. A signal handler may touch nothing but a
/// lock-free atomic, so it cannot run the teardown itself; it asks the worker, which
/// is at most one [`TICK`] away.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn note_signal(_signum: i32) {
    SHUTDOWN.store(true, Ordering::SeqCst);
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

    fn shutdown_requested() -> bool {
        SHUTDOWN.load(Ordering::SeqCst)
    }
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
    Simulated {
        reason: Option<TapError>,
    },
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
/// # use trak::visualizer::AudioPipeline;
/// let mut viz = AudioPipeline::new(trak::config::VisualizerSource::Auto);
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
    events: Receiver<TapEvent>,
    /// Whether a notice has already been raised this session.
    warned: bool,
    /// A notice produced but not yet collected, so one raised between two frames is
    /// not lost.
    pending_notice: Option<String>,
    simulated: SimulatedSource,
}

impl AudioPipeline {
    /// A pipeline for `want`.
    ///
    /// `SimulatedSource` returns immediately and never touches Core Audio. `Auto`
    /// returns immediately too, holding a simulated spectrum until
    /// [`set_wanted`](Self::set_wanted) says the visualizer is on screen.
    pub fn new(want: VisualizerSource) -> Self {
        Self {
            want,
            state: match want {
                VisualizerSource::Simulated => TapState::Forced,
                VisualizerSource::Auto => TapState::Idle,
            },
            tap: None,
            events: channel().1,
            warned: false,
            pending_notice: None,
            simulated: SimulatedSource::new(0),
        }
    }

    /// Whether the tap should exist right now.
    ///
    /// The TUI calls this once per frame, next to whatever decides the visualizer is
    /// visible: `true` on screen, `false` hidden. Going false releases the tap within
    /// one worker tick, which is what keeps a hidden visualizer from holding an audio
    /// device open.
    pub fn set_wanted(&mut self, wanted: bool) {
        if self.want == VisualizerSource::Simulated {
            return;
        }
        match (&mut self.tap, wanted) {
            (None, true) => {
                let (outbox, events) = channel();
                let (commands, inbox) = channel();
                let bars = Arc::new(BarCell::new());
                let published = Arc::clone(&bars);
                self.tap = Some(RealTap {
                    bars,
                    commands,
                    worker: Some(spawn(move || run(inbox, outbox, published))),
                });
                self.events = events;
                self.state = TapState::Idle;
            }
            (Some(tap), true) => tap.send(Command::Run),
            (Some(tap), false) => tap.send(Command::Idle),
            (None, false) => {}
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
        self.drain_events();
        match self.state {
            // A tap that has not started yet draws silence rather than simulated bars:
            // the two would disagree for the fraction of a second before the first real
            // frame, and a spectrum that jumps is the thing this whole file exists to
            // avoid.
            TapState::Tapping { .. } => self
                .tap
                .as_ref()
                .map(RealTap::spectrum)
                .unwrap_or_else(|| vec![0.0; BARS]),
            _ => self.simulated.spectrum(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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

    /// Feed `samples` through an analyser in window-sized pieces, as the worker does.
    fn run_analyser(rate: f64, samples: &[f32]) -> Analyser {
        let mut analyser = Analyser::new(rate).expect("a rate cavacore accepts");
        let limit = analyser.limit();
        let mut offset = 0;
        while offset < samples.len() {
            let end = (offset + limit).min(samples.len());
            analyser.offer(&samples[offset..end]);
            analyser.advance();
            offset = end;
        }
        analyser
    }

    fn loudest(bars: &[f32]) -> f32 {
        bars.iter().copied().fold(0.0, f32::max)
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

        let quiet = vec![0.0; BARS];
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
        // song that stopped. From a music-level peak to the floor is about two
        // seconds of ticks.
        let mut scale = BarScale::new();
        let mut loud = vec![0.0; BARS];
        loud[0] = 1e-4;
        scale.scale(&mut loud);

        let quiet = vec![0.0; BARS];
        let mut frames = 0;
        let mut bars = loud;
        while loudest(&bars) > 0.0 && frames < 2000 {
            scale.scale(&mut bars);
            frames += 1;
        }
        assert_eq!(bars, vec![0.0; BARS], "silence after the music stopped");
        assert!(
            frames <= 200,
            "and it took {frames} frames, which is {} s",
            f64::from(frames) * TICK.as_secs_f64()
        );
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
            let mut bars: Vec<f32> = (0..BARS)
                .map(|i| junk[(i + round) % junk.len()])
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
    fn a_nan_never_becomes_the_peak() {
        let mut scale = BarScale::new();
        let mut poisoned = vec![f32::NAN; BARS];
        poisoned[1] = 0.25;
        scale.scale(&mut poisoned);
        assert_eq!(poisoned[1], 1.0, "the real signal still scales: {poisoned:?}");
        assert_eq!(poisoned[0], 0.0, "and the NaN became silence");

        // And a frame that is *only* junk must not poison the peak either, because a
        // peak of infinity would divide every later frame by it.
        let mut junk = vec![f32::NAN; BARS];
        junk[0] = f32::INFINITY;
        scale.scale(&mut junk);
        assert_eq!(scale.peak, SILENCE, "no junk became the peak");
        assert_eq!(junk, vec![0.0; BARS]);
    }

    // -- the analyser ------------------------------------------------------

    /// A `Cava` built the way `Analyser` builds it, so the bound can be checked
    /// against the crate rather than against a copy of the crate.
    fn bare_cava(rate: u32) -> Cava {
        CavaBuilder::default()
            .bars_per_channel(NonZeroUsize::new(BARS).expect("BARS"))
            .sample_rate(CavaSampleRate::new(rate).expect("in range"))
            .audio_channels(Channels::Mono)
            .enable_autosens(true)
            .noise_reduction(0.2)
            .frequency_range(
                NonZeroU32::new(60).expect("60")..NonZeroU32::new(frequency_top(rate)).expect("top"),
            )
            .build()
            .unwrap_or_else(|_| panic!("a Cava at {rate} Hz"))
    }

    #[test]
    fn the_window_bound_is_cavacores_own() {
        // `max_input_samples` reproduces a `pub(crate)` function, and being wrong in
        // the direction of "too small" is a panic on the worker with `panic = "abort"`
        // in the release profile. So: exactly the bound must be accepted by the real
        // crate, and one more sample must not be.
        for rate in [8_000u32, 44_100, 48_000, 96_000, 192_000] {
            let bound = max_input_samples(rate);
            assert_eq!(
                Analyser::new(f64::from(rate)).expect("the rate is fine").limit(),
                bound
            );

            let mut out = vec![0.0f64; BARS];
            // SAFETY-free by way of `catch_unwind`: `Cava` holds raw pointers, so the
            // closure needs the assertion. Nothing survives it either way; the point
            // is only to see whether it panicked.
            let at_bound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                bare_cava(rate).execute(&vec![0.0; bound], &mut out);
            }));
            assert!(at_bound.is_ok(), "{rate} Hz: the bound itself must work");

            let over = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                bare_cava(rate).execute(&vec![0.0; bound + 1], &mut out);
            }));
            assert!(
                over.is_err(),
                "{rate} Hz: {bound} is not cavacore's bound, it accepted {bound} samples and \n\
                 {bound} and one more"
            );
        }
    }

    #[test]
    fn the_window_bound_grows_with_the_rate() {
        assert!(max_input_samples(96_000) > max_input_samples(48_000));
        assert!(max_input_samples(48_000) > max_input_samples(8_000));
        assert_eq!(max_input_samples(48_000), 8192);
    }

    #[test]
    fn an_analyser_takes_the_rate_it_is_given_and_not_a_constant() {
        assert_eq!(Analyser::new(48_000.0).expect("48k").sample_rate(), 48_000.0);
        assert_eq!(Analyser::new(44_100.0).expect("44.1k").sample_rate(), 44_100.0);
    }

    #[test]
    fn an_analyser_refuses_a_rate_it_cannot_use() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
            assert!(
                matches!(
                    Analyser::new(rate),
                    Err(TapError::BadSampleRate(_)) | Err(TapError::Analyser(_))
                ),
                "{rate} Hz"
            );
        }
    }

    #[test]
    fn the_window_is_clamped_to_nyquist_for_a_slow_tap() {
        // `cavacore` rejects a frequency range whose end is above half the sample rate,
        // so a 16 kHz window is a configuration error on a 22 kHz tap.
        for rate in [16_000u32, 22_050, 32_000, 44_100, 48_000, 96_000] {
            assert!(frequency_top(rate) <= rate / 2, "{rate}");
            assert!(
                Analyser::new(f64::from(rate)).is_ok(),
                "{rate} Hz should configure"
            );
        }
        assert_eq!(frequency_top(48_000), 16_000);
        assert_eq!(frequency_top(22_050), 11_025);
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
            highest < BARS / 2,
            "80 Hz landed in the upper half (band {highest}): {bars:?}"
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
            "4 kHz landed in the lower half (band {highest}): {bars:?}"
        );
    }

    #[test]
    fn an_analyser_discriminates_two_frequencies() {
        // TODO 1.6's fixture, through the real path rather than a bare `Cava`. If the
        // two came out the same shape, the tap would be feeding the display noise.
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
        // The overflow case: a slow tick leaves more pending than one `execute` takes,
        // and the leftovers have to be analysed rather than dropped or the spectrum
        // develops gaps.
        let samples = sine(48_000.0, 440.0, 1.0);
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.offer(&samples);
        let window = analyser.limit();
        analyser.advance();
        let after_one = analyser.bars().to_vec();
        while analyser.pending.len() > window {
            analyser.advance();
        }
        assert!(
            analyser.bars().iter().any(|b| *b > 0.05),
            "the rest of the audio still analysed: {:?}",
            analyser.bars()
        );
        assert_eq!(after_one.len(), BARS);
    }

    #[test]
    fn an_analyser_with_nothing_offered_is_a_no_op() {
        let mut analyser = Analyser::new(48_000.0).expect("48k");
        analyser.advance();
        analyser.advance();
        assert_eq!(analyser.bars(), &[0.0; BARS]);
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
        assert!(matches!(
            TapError::from_status(0),
            TapError::CoreAudio(_)
        ));
    }

    #[test]
    fn a_denial_says_where_to_go_and_nothing_else_does() {
        let line = TapError::PermissionDenied("'!hog'").notice().expect("a line");
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
        let (state, notice) = outcome(&TapEvent::Started { sample_rate: 48_000.0 }, false);
        assert_eq!(state, TapState::Tapping { sample_rate: 48_000.0 });
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
                assert!(bar.is_finite() && (0.0..=1.0).contains(bar), "band {i}: {bar}");
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
            TapState::Tapping { sample_rate: 48_000.0 },
            TapState::Simulated { reason: None },
        ] {
            let mut viz = AudioPipeline::new(VisualizerSource::Simulated);
            viz.state = state.clone();
            let bars = viz.spectrum();
            assert_eq!(bars.len(), BARS, "{state:?}");
        }
    }
}
