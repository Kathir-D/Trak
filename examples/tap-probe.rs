// TODO 8.3 and 8.5: does the real tap actually track the music, and does it leave
// anything behind?
//
// Two questions that cannot be asked in a unit test, because both need a running
// Spotify, a permission grant and an audio device — none of which CI has:
//
//   cargo run --release --example tap-probe run [seconds]
//       attach the tap, hold it, and print the state, the rate Core Audio
//       reported, how much the bars moved, and what the CPU cost.
//
//   cargo run --release --example tap-probe cycles [count]
//       attach and release `count` times, which is 8.5's done-when: run this
//       between two `system_profiler SPAudioDataType` dumps and diff them.
//
//   cargo run --release --example tap-probe idle [seconds]
//       draw for the same time with the tap never asked for, which is the baseline the
//       `run` number is worth anything against.
//
//   cargo run --release --example tap-probe signal
//       attach, then raise SIGTERM at ourselves, so "a kill still kills, and the
//       tap is down first" is a thing that was measured rather than intended.
//
//   cargo run --release --example tap-probe census
//       count the audio devices and process taps Core Audio shows *this* process.
//       Measured: another process's tap and private aggregate device are invisible
//       here, so on its own this only gives the baseline; `cycles` runs the same
//       count from inside, where its own tap does show (3/0 -> 4/1 -> 3/0).
//
// Nothing here launches Spotify or touches it: it reads the processes that are
// already running, which is what COMPAT rule 2 forbids going beyond.
use std::time::{Duration, Instant};

use trak::audio::AudioPipeline;
use trak::config::VisualizerSource;
use trak::visualizer::AudioSource;

/// One line per bar, so a spectrum that is not moving is obvious on one screen.
fn draw(bars: &[f32]) -> String {
    const ROWS: usize = 12;
    let mut grid = vec![vec![' '; bars.len()]; ROWS];
    for (x, bar) in bars.iter().enumerate() {
        let height = (f64::from(*bar) * ROWS as f64).round() as usize;
        for row in grid.iter_mut().skip(ROWS.saturating_sub(height)) {
            row[x] = '#';
        }
    }
    let mut lines: Vec<String> = (0..ROWS)
        .map(|row| {
            format!(
                "    {:>5.1} |{}|",
                (ROWS - 1 - row) as f64 / ROWS as f64,
                grid[row].iter().collect::<String>()
            )
        })
        .collect();
    lines.push("                +----------------------------+".into());
    lines.join("\n")
}

fn usage() -> ! {
    eprintln!("usage: tap-probe <run [secs] | idle [secs] | cycles [count] | signal | census>");
    std::process::exit(2)
}

/// How far the bars travelled over the whole run, and how full they got. Both are
/// needed: a spectrum that is all zero is a dead tap, and one that is all full is a
/// tap full of noise.
struct Motion {
    loudest: f32,
    moved: f64,
    frames: usize,
}

/// One second of real frames, summed.
#[derive(Default)]
struct Window {
    frames: usize,
    mean: f64,
    peak: f32,
    thirds: [f64; 3],
}

impl Window {
    fn add(&mut self, bars: &[f32]) {
        let n = bars.len().max(1);
        self.frames += 1;
        self.mean += bars.iter().map(|b| f64::from(*b)).sum::<f64>() / n as f64;
        self.peak = bars.iter().copied().fold(self.peak, f32::max);
        for (i, third) in bars.chunks(n.div_ceil(3)).enumerate().take(3) {
            self.thirds[i] += third.iter().map(|b| f64::from(*b)).sum::<f64>() / third.len() as f64;
        }
    }

    fn line(&self) -> String {
        if self.frames == 0 {
            return "  (no real frames: simulated or waiting)".into();
        }
        let f = self.frames as f64;
        format!(
            "{:.3}  {:.3}  {:.3}  {:.3}  {:.3}",
            self.mean / f,
            self.peak,
            self.thirds[0] / f,
            self.thirds[1] / f,
            self.thirds[2] / f
        )
    }
}

fn track(motion: &mut Motion, bars: &[f32]) {
    let peak = bars.iter().copied().fold(0.0f32, f32::max);
    motion.loudest = motion.loudest.max(peak);
    motion.frames += 1;
    motion.moved += f64::from(peak);
}

fn report(motion: &Motion, seconds: f64) {
    println!("  frames drawn    : {}", motion.frames);
    println!("  loudest bar     : {:.3}", motion.loudest);
    println!(
        "  mean bar height : {:.4}",
        motion.moved / motion.frames.max(1) as f64
    );
    println!(
        "  verdict         : {}",
        match motion.loudest {
            0.0 => "SILENCE — nothing arrived. Is Spotify playing?",
            v if v < 0.02 => "almost nothing arrived",
            v if v > 0.999 && motion.moved / motion.frames.max(1) as f64 > 0.99 => {
                "every bar pinned at full height — that is noise, not music"
            }
            _ => "bars are moving",
        }
    );
    println!("  wall clock      : {seconds:.1} s");
}

fn run(secs: f64) {
    let mut viz = AudioPipeline::new(VisualizerSource::Auto);
    viz.set_wanted(true);
    let started = Instant::now();
    let mut motion = Motion {
        loudest: 0.0,
        moved: 0.0,
        frames: 0,
    };
    let mut frames = 0usize;
    let mut first_shape = String::new();

    // One line per second of what the real bars did, read the way the TUI reads
    // them (`live_spectrum`, which is `None` unless a tap is up): the mean height,
    // the loudest bar, and the energy in the bottom, middle and top thirds. Music
    // moves all of it from second to second; a paused Spotify is all zeros.
    let mut second = Window::default();
    println!("     t  state                      mean   peak    low    mid   high");

    // The render loop's own cadence, so what is measured is what the TUI does.
    while started.elapsed() < Duration::from_secs_f64(secs) {
        let live = viz.live_spectrum();
        let bars = live
            .clone()
            .unwrap_or_else(|| vec![0.0; trak::visualizer::BARS]);
        track(&mut motion, &bars);
        if live.is_some() {
            second.add(&bars);
        }
        if frames == 0 {
            first_shape = draw(&bars);
        }
        frames += 1;
        if frames.is_multiple_of(30) {
            println!(
                "  {:>4.1}s  {:<24} {}",
                started.elapsed().as_secs_f64(),
                format!("{:?}", viz.state()),
                second.line()
            );
            second = Window::default();
        }
        if frames.is_multiple_of(150) {
            println!("{}", draw(&bars));
        }
        if let Some(line) = viz.take_notice() {
            println!("  notice: {line}");
        }
        std::thread::sleep(Duration::from_millis(33));
    }

    println!("\nfirst frame:\n{first_shape}\n");
    println!("final state : {:?}", viz.state());
    println!("sample rate  : {:?}", viz.sample_rate());
    report(&motion, started.elapsed().as_secs_f64());
    viz.set_wanted(false);
}

/// The same loop as `run` with the tap never asked for: the baseline for the CPU
/// figure, and the shape of a `source = "simulated"` machine.
fn idle(secs: f64) {
    let mut viz = AudioPipeline::new(VisualizerSource::Auto);
    let started = Instant::now();
    let mut motion = Motion {
        loudest: 0.0,
        moved: 0.0,
        frames: 0,
    };
    while started.elapsed() < Duration::from_secs_f64(secs) {
        viz.set_playing(true);
        viz.set_position(started.elapsed().as_secs_f64());
        let bars = viz.spectrum();
        track(&mut motion, &bars);
        std::thread::sleep(Duration::from_millis(33));
    }
    println!("  never asked for the tap; state {:?}", viz.state());
    report(&motion, started.elapsed().as_secs_f64());
}

fn cycles(count: usize) {
    // Deliberately one pipeline for the whole run: the point is that the *worker*
    // takes the tap down and puts it back up, not that a new process does.
    let mut viz = AudioPipeline::new(VisualizerSource::Auto);
    println!("  before any tap:");
    census();
    for n in 1..=count {
        viz.set_wanted(true);
        // Long enough for the worker to have attached and published a state, and short
        // enough that ten of them is not a minute of waiting.
        let deadline = Instant::now() + Duration::from_millis(1_500);
        while Instant::now() < deadline {
            let _ = viz.spectrum();
            if *viz.state() != trak::audio::TapState::Idle {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let up = viz.state().clone();
        std::thread::sleep(Duration::from_millis(250));
        if n == 1 {
            // What this process sees of its own tap while it is up, so the "after"
            // census below is known to be able to see one at all.
            println!("  with the first tap up:");
            census();
        }
        viz.set_wanted(false);
        // Wait for the release to come back, so the next cycle starts from Idle.
        let deadline = Instant::now() + Duration::from_millis(1_500);
        while Instant::now() < deadline {
            let _ = viz.spectrum();
            if *viz.state() == trak::audio::TapState::Idle {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("  cycle {n:>3}: up as {up:?}, down as {:?}", viz.state());
    }
    // The worker is joined here, so this line is after every `Drop` has run.
    drop(viz);
    println!("  after {count} cycles, every Drop run:");
    census();
    println!("\n{count} cycles done, every Drop run. Check the visible side with:");
    println!("  system_profiler SPAudioDataType | diff - /tmp/before.txt -");
    println!("and read that with §4 in mind: Trak's aggregate device is private, so it");
    println!("is absent from that output even while a tap is live.");
}

fn signal() {
    let mut viz = AudioPipeline::new(VisualizerSource::Auto);
    viz.set_wanted(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = viz.spectrum();
        if *viz.state() != trak::audio::TapState::Idle {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("  state before the signal: {:?}", viz.state());
    println!("  raising SIGTERM at self (pid {})", std::process::id());
    // SAFETY: raising a signal at this process with the default disposition still
    // installed ends the process, which is what the probe is checking. The tap is
    // already up, so this is the crash-adjacent path with the tap live.
    unsafe {
        libc_kill(std::process::id() as i32, 15);
    }
    std::thread::sleep(Duration::from_secs(5));
    println!("  STILL ALIVE — the handler swallowed SIGTERM");
    std::process::exit(1);
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const std::ffi::c_void,
        size: *mut u32,
    ) -> i32;
}

/// How many `AudioObjectID`s the system object lists under `selector`.
fn count(selector: &[u8; 4]) -> Result<u32, i32> {
    let address = PropertyAddress {
        selector: u32::from_be_bytes(*selector),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let mut size = 0u32;
    // SAFETY: the system object (1) with a global-scope address and no qualifier;
    // the out pointer is one `u32`, which is what the call writes.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(1, &address, 0, std::ptr::null(), &mut size) };
    if status == 0 {
        Ok(size / 4)
    } else {
        Err(status)
    }
}

/// The devices and process taps Core Audio shows this process.
fn census() {
    match count(b"dev#") {
        Ok(n) => println!("  devices : {n}"),
        Err(e) => println!("  devices : error {e}"),
    }
    // `kAudioHardwarePropertyTapList`. Only this process's own taps show up here:
    // a second process counted 0 while one was live elsewhere.
    match count(b"tps#") {
        Ok(n) => println!("  taps    : {n}"),
        Err(e) => println!("  taps    : error {e}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(what) = args.next() else { usage() };
    match what.as_str() {
        "run" => run(args.next().and_then(|s| s.parse().ok()).unwrap_or(10.0)),
        "idle" => idle(args.next().and_then(|s| s.parse().ok()).unwrap_or(10.0)),
        "cycles" => cycles(args.next().and_then(|s| s.parse().ok()).unwrap_or(10)),
        "signal" => signal(),
        "census" => census(),
        _ => usage(),
    }
}
