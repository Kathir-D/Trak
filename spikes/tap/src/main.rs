// TODO 1.5: tap **Spotify's process only** and print RMS levels, so the
// visualizer can run on real audio without disturbing Sonar's own tap.
//
// Uses `with_mono_mixdown_of_processes` (an *allow*list) rather than cidre's
// example `with_stereo_global_tap_excluding_processes` (a deny-list over system
// audio). That matters twice: it taps only the pid we name, and it is the shape
// SPEC section 7 and COMPAT rule 3 require.
//
// What this spike is for, in order of importance:
//   1. Does an un-bundled CLI get a usable tap at all on this machine?
//   2. What does *failure* look like -- an OSStatus, silence, or a hang?
//      trak needs a bounded timeout either way (TODO 8.3).
//   3. Does it survive Sonar's tap being active at the same time? (TODO 10.7)
//
// (1) and (3) need a human: the "System Audio Recording" grant is a modal system
// panel. On a machine with no such grant at all -- checked, the TCC database has
// no kTCCServiceMicrophone rows -- the first call is expected to fail or prompt.
// This program is written so the owner can just run it and click Allow, and so
// that a failure prints the OSStatus straight into docs/AUDIO-TAP.md.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use cidre::{arc, cf, core_audio as ca, ns};

const SPOTIFY_BUNDLE_ID: &str = "com.spotify.client";

/// pids of every running instance of a bundle id.
fn pids_for_bundle(bundle_id: &str) -> Vec<i64> {
    let apps = ns::RunningApp::with_bundle_id(&ns::String::with_str(bundle_id));
    apps.iter().map(|a| a.pid() as i64).collect()
}

#[allow(dead_code)]
struct Ctx {
    frames: AtomicU64,
    running: AtomicBool,
    last_report: AtomicU64,
    peak: f32,
    sample_rate: f64,
}

/// RMS + peak over the f32 samples in the buffer list, read straight from the
/// Core Audio pointers. Interleaved float32, which is what the tap's ASBD says.
fn measure(list: &cidre::cat::AudioBufList<2>) -> (f64, f32, usize) {
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    let mut n = 0usize;
    for i in 0..list.number_buffers as usize {
        let b = list.buffers[i];
        if b.data.is_null() || b.data_bytes_size == 0 {
            continue;
        }
        // SAFETY: Core Audio guarantees `data` points to `data_bytes_size` valid
        // bytes for the duration of the IO callback, and we only read.
        let samples = unsafe {
            std::slice::from_raw_parts(b.data as *const f32, b.data_bytes_size as usize / 4)
        };
        for s in samples {
            sum += (s * s) as f64;
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        n += samples.len();
    }
    let rms = if n == 0 { 0.0 } else { (sum / n as f64).sqrt() };
    (rms, peak, n)
}

extern "C" fn on_audio(
    _device: ca::Device,
    _now: &cidre::cat::AudioTimeStamp,
    input_data: &cidre::cat::AudioBufList<2>,
    _input_time: &cidre::cat::AudioTimeStamp,
    _output_data: &mut cidre::cat::AudioBufList<2>,
    _output_time: &cidre::cat::AudioTimeStamp,
    ctx: Option<&mut Ctx>,
) -> cidre::os::Status {
    let Some(ctx) = ctx else {
        return Default::default();
    };
    if !ctx.running.load(Ordering::Relaxed) {
        return Default::default();
    }
    let (rms, peak, n) = measure(input_data);
    if n == 0 {
        return Default::default();
    }
    if peak > ctx.peak {
        ctx.peak = peak;
    }
    let total = ctx.frames.fetch_add(n as u64, Ordering::Relaxed);
    // report about 4x a second
    let window = (ctx.sample_rate * 0.25) as u64;
    if window > 0 && total / window != ctx.last_report.load(Ordering::Relaxed) / window {
        ctx.last_report.store(total, Ordering::Relaxed);
        let db = 20.0 * rms.max(1e-9).log10();
        let bars = ((rms * 12.0).clamp(0.0, 1.0) * 44.0) as usize;
        println!(
            "  rms={rms:.6}  {db:>7.2} dBFS  peak={:.4}  |{:<44}|",
            ctx.peak,
            "#".repeat(bars)
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
        ctx.peak = 0.0;
    }
    Default::default()
}

/// Reach the flag the callback reads, without moving `ctx`.
fn ctx_running(ctx: &mut Ctx) -> &AtomicBool {
    &ctx.running
}

fn die(stage: &str, e: cidre::os::Error) -> ! {
    eprintln!();
    eprintln!("{stage} FAILED");
    eprintln!("  os status    : {}", e.0);
    eprintln!("  fourcc       : {:?}", e);
    eprintln!();
    eprintln!("Record this in docs/AUDIO-TAP.md. If it is an authorisation status,");
    eprintln!("the fix is to grant System Audio Recording to cmux in");
    eprintln!("System Settings > Privacy & Security > Screen & System Audio Recording,");
    eprintln!("then re-run. trak must treat this as \"fall back to simulated\" (TODO 8.3),");
    eprintln!("never as a fatal error.");
    std::process::exit(1);
}

fn main() {
    println!("trak tap spike — TODO 1.5");
    println!(
        "build target: {} (process taps need macOS 14.2+)",
        std::env::consts::ARCH
    );

    let pids = pids_for_bundle(SPOTIFY_BUNDLE_ID);
    if pids.is_empty() {
        eprintln!(
            "ERROR: Spotify (com.spotify.client) is not running, so there is nothing to tap."
        );
        eprintln!("       Start Spotify and play something, then re-run.");
        eprintln!("       trak must never launch Spotify as a side effect (COMPAT rule 2).");
        std::process::exit(2);
    }
    println!("tapping pids {pids:?} — Spotify ONLY; Sonar's audio is excluded by construction");

    let output_device = match ca::System::default_output_device() {
        Ok(d) => d,
        Err(e) => die("default output device lookup", e),
    };
    let output_uid = output_device.uid().unwrap();

    // ALLOWLIST: mix down only these pids. cidre duplicates mono into both
    // channels, so asking for a mono mixdown costs nothing.
    // pids are pid_t (int32). cidre's docs do not say which NSNumber width the
    // selector wants, and a double-typed NSNumber is the kind of thing macOS
    // silently rejects -- so TRAK_PID_TYPE can switch and the spike can find out.
    let numbers: Vec<arc::R<ns::Number>> = pids
        .iter()
        .map(|p| match std::env::var("TRAK_PID_TYPE").as_deref() {
            Ok("i32") => ns::Number::with_i32(*p as i32),
            Ok("i64") => ns::Number::with_i64(*p),
            Ok("u32") => ns::Number::with_u32(*p as u32),
            _ => ns::Number::with_f64(*p as f64),
        })
        .collect();
    let proc_ids = ns::Array::from_slice_retained(&numbers);

    // TRAK_TAP_GLOBAL=1 taps system audio with an empty deny-list instead, purely
    // to tell "the tap mechanism is refused for this process" apart from "my
    // allow-list is malformed". Not something trak would ever ship.
    // Try each tap-description shape until one is accepted, so the spike
    // reports which of them macOS/cidre actually supports.
    let mode = std::env::var("TRAK_TAP_MODE").unwrap_or_else(|_| "mono-mixdown".into());
    let tap_desc = match mode.as_str() {
        "global-mono" => ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new()),
        "global-stereo" => {
            ca::TapDesc::with_stereo_global_tap_excluding_processes(&ns::Array::new())
        }
        "mono-mixdown" => ca::TapDesc::with_mono_mixdown_of_processes(&proc_ids),
        "stereo-mixdown" => ca::TapDesc::with_stereo_mixdown_of_processes(&proc_ids),
        "global-exclude-spotify" => {
            // tap everything EXCEPT Spotify: proves the deny-list path works
            ca::TapDesc::with_mono_global_tap_excluding_processes(&proc_ids)
        }
        other => {
            eprintln!("unknown TRAK_TAP_MODE {other}");
            std::process::exit(2);
        }
    };
    println!("  tap description mode: {mode}");
    println!("creating process tap (this is where a permission prompt appears)...");
    let tap = match tap_desc.create_process_tap() {
        Ok(t) => t,
        Err(e) => die("create_process_tap", e),
    };
    println!(
        "  tap created: {}",
        tap.name().map(|n| n.to_string()).unwrap_or_default()
    );

    let tap_uid = match tap.uid() {
        Ok(u) => u,
        Err(e) => die("tap uid", e),
    };
    let asbd = match tap.asbd() {
        Ok(a) => a,
        Err(e) => die("tap asbd", e),
    };
    let sample_rate = asbd.sample_rate;
    println!(
        "  format: {sample_rate} Hz, {} ch, {} bit, flags={:?}",
        asbd.channels_per_frame, asbd.bits_per_channel, asbd.format_flags
    );
    println!("  ^ trak must configure cavacore with THIS rate, not a constant (TODO 1.6).");

    use cidre::core_audio::aggregate_device_keys as agg_keys;
    use cidre::core_audio::sub_device_keys as sub_keys;
    let sub_device =
        cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[output_uid.as_type_ref()]);
    let sub_tap = cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[tap_uid.as_type_ref()]);

    let dict = cf::DictionaryOf::with_keys_values(
        &[
            agg_keys::is_private(),
            agg_keys::is_stacked(),
            agg_keys::tap_auto_start(),
            agg_keys::name(),
            agg_keys::main_sub_device(),
            agg_keys::uid(),
            agg_keys::sub_device_list(),
            agg_keys::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::str!(c"trak tap").as_type_ref(),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );
    println!("creating aggregate device (check `system_profiler SPAudioDataType`)...");
    let agg_device = match ca::AggregateDevice::with_desc(&dict) {
        Ok(d) => d,
        Err(e) => die("AggregateDevice::with_desc", e),
    };
    println!("  aggregate device created");

    let running = AtomicBool::new(true);
    let mut ctx = Ctx {
        frames: AtomicU64::new(0),
        running,
        last_report: AtomicU64::new(0),
        peak: 0.0,
        sample_rate,
    };

    println!("starting IO proc — 20s window, play something in Spotify");
    let proc_id = match agg_device.create_io_proc_id(on_audio, Some(&mut ctx)) {
        Ok(p) => p,
        Err(e) => die("create_io_proc_id", e),
    };
    // Keep the StartedDevice alive: dropping it stops the device, and dropping it
    // early is what made the first version of this spike look like it hung.
    let started = match ca::device_start(agg_device, Some(proc_id)) {
        Ok(d) => d,
        Err(e) => die("device_start", e),
    };

    std::thread::sleep(Duration::from_secs(20));
    // `ctx` (and the flag inside it) is borrowed by the IO proc, so signal it
    // through that same borrow rather than a moved-out copy.
    ctx_running(&mut ctx).store(false, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(200));

    let n = ctx.frames.load(Ordering::Relaxed);
    println!();
    println!(
        "--- {n} float samples in 20s ({:.1} samples/s) ---",
        n as f64 / 20.0
    );
    if n == 0 {
        println!("NOTHING ARRIVED. The tap started but delivered no audio:");
        println!("  - Spotify may be silent, paused, or between tracks, or");
        println!("  - the grant exists but is not being applied to this process.");
        println!("Both are 'fall back to simulated' for trak, not fatal errors.");
    }
    // Explicit teardown. TODO 8.5 must make "no leftover device" a test, not a
    // hope: 10 start/stop cycles, then compare `system_profiler SPAudioDataType`.
    drop(started);
    println!("stopped and dropped the device, tap and aggregate device");
    println!("TODO 8.5 must verify with `system_profiler SPAudioDataType` that none remain");
}
