// TODO 1.6: is `cavacore` usable for trak's visualizer, and does it build on
// stable Rust for both release targets?
//
// Two questions:
//   1. Does it build for aarch64-apple-darwin AND x86_64-apple-darwin on stable?
//      (1.6 explicitly requires both, because trak ships a universal binary.)
//   2. What sample rate, bar count and buffer size actually work, and what do the
//      bar heights look like for a known synthetic signal?
//
// The synthetic-signal part matters: it gives trak a deterministic fixture so
// TODO 8.x's renderers can be tested without a tap and without audio hardware.

use std::num::NonZeroU32;
use std::num::NonZeroUsize;

use cavacore::Channels;
use cavacore::SampleRate as BoundedSampleRate;

const SAMPLE_RATE: u32 = 44_100;
const BARS: usize = 32;

fn build() -> Result<cavacore::Cava, Vec<cavacore::Error>> {
    cavacore::CavaBuilder::default()
        .bars_per_channel(NonZeroUsize::new(BARS).unwrap())
        .sample_rate(BoundedSampleRate::new(SAMPLE_RATE).expect("in range"))
        .audio_channels(Channels::Mono)
        .enable_autosens(true)
        .noise_reduction(0.2)
        .frequency_range(NonZeroU32::new(60).unwrap()..NonZeroU32::new(16_000).unwrap())
        .build()
}

/// Bar heights for a synthetic signal, printed as a fixed-width block so the
/// output is diffable and can be committed as a fixture.
///
/// A *fresh* Cava per signal: cava carries peak/autosens memory between calls,
/// so reusing one instance makes a 440 Hz run's leftovers show up as a 80 Hz
/// signal's spectrum and hides whether the frequency mapping works at all.
fn render(name: &str, samples: &[f64], frames: usize) -> Vec<f64> {
    let mut cava = match build() {
        Ok(c) => c,
        Err(e) => {
            println!("BUILD FAILED: {e:?}");
            std::process::exit(1);
        }
    };
    let mut out = vec![0.0f64; BARS];
    // Prime through the whole signal so autosens settles, then read the last window.
    let mut last = out.clone();
    for chunk in samples.chunks(frames) {
        cava.execute(chunk, &mut out);
        last.copy_from_slice(&out);
    }
    let out = last;
    println!("--- {name} (frames={}, sr={})", samples.len(), SAMPLE_RATE);
    let peak = out.iter().cloned().fold(0.0f64, f64::max);
    for (i, v) in out.iter().enumerate() {
        let bars = ((v / peak.max(1e-9)) * 40.0).round() as usize;
        println!("  {i:2} {:>7.4} |{:<40}|", v, "#".repeat(bars.min(40)));
    }
    println!(
        "  peak={peak:.4} mean={:.4}",
        out.iter().sum::<f64>() / BARS as f64
    );
    out
}

fn sine(freq: f64, seconds: f64) -> Vec<f64> {
    let n = (SAMPLE_RATE as f64 * seconds) as usize;
    (0..n)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / SAMPLE_RATE as f64).sin())
        .collect()
}

fn main() {
    println!("cavacore spike — sample_rate={SAMPLE_RATE} bars={BARS} channels=mono");
    match build() {
        Ok(_) => println!("build ok"),
        Err(e) => {
            println!("BUILD FAILED: {e:?}");
            std::process::exit(1);
        }
    }

    // 1 s is the 1.6 requirement. 2048 frames ≈ 46 ms, roughly one FFT window.
    let frames = 2048;

    let a = render("440 Hz sine (music-range)", &sine(440.0, 2.0), frames);
    let b = render("80 Hz sine (bass)", &sine(80.0, 2.0), frames);
    let c = render("silence", &vec![0.0f64; SAMPLE_RATE as usize * 2], frames);
    let d = render(
        "white noise",
        &{
            // deterministic LCG, so the fixture is reproducible
            let mut s = 12345u64;
            (0..SAMPLE_RATE as usize)
                .map(|_| {
                    s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
                })
                .collect::<Vec<f64>>()
        },
        frames,
    );

    println!();
    println!("summary:");
    println!(
        "  440Hz peak={:.4}",
        a.iter().cloned().fold(0.0f64, f64::max)
    );
    println!(
        "   80Hz peak={:.4}",
        b.iter().cloned().fold(0.0f64, f64::max)
    );
    println!(
        "  silence peak={:.4}",
        c.iter().cloned().fold(0.0f64, f64::max)
    );
    println!(
        "   noise peak={:.4}",
        d.iter().cloned().fold(0.0f64, f64::max)
    );

    // Does the output actually discriminate? If 80Hz and 440Hz produce the same
    // profile, the tap is not worth wiring up.
    let low_energy: usize = (0..BARS / 2).filter(|&i| b[i] > 0.01).count();
    let high_energy: usize = (BARS / 2..BARS).filter(|&i| a[i] > 0.01).count();
    println!("  80Hz bars with energy in the low half : {low_energy}");
    println!("  440Hz bars with energy in the high half: {high_energy}");

    println!();
    println!("sample-rate sweep (does 48k work too?):");
    for sr in [44_100u32, 48_000, 96_000] {
        let ok = cavacore::CavaBuilder::default()
            .bars_per_channel(NonZeroUsize::new(BARS).unwrap())
            .sample_rate(BoundedSampleRate::new(sr).expect("in range"))
            .audio_channels(Channels::Mono)
            .enable_autosens(true)
            .build()
            .is_ok();
        println!("  {sr:>6} Hz -> {}", if ok { "ok" } else { "FAILED" });
    }

    println!();
    println!("bar-count sweep at 44.1kHz:");
    for bars in [16usize, 24, 32, 48, 64, 96] {
        let ok = cavacore::CavaBuilder::default()
            .bars_per_channel(NonZeroUsize::new(bars).unwrap())
            .sample_rate(BoundedSampleRate::new(SAMPLE_RATE).expect("in range"))
            .audio_channels(Channels::Mono)
            .enable_autosens(true)
            .build()
            .is_ok();
        println!("  {bars:>3} bars -> {}", if ok { "ok" } else { "FAILED" });
    }
}
