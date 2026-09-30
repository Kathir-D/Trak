//! The visualizer: the seam its numbers come through, the simulated source that
//! ships behind it today, and the four renderers that draw it.
//!
//! Three jobs, in the order the data flows. [`AudioSource`] is the seam a real
//! Core Audio tap implements; [`SimulatedSource`] is the one behind it now; and
//! the renderers — [`spectrum`], [`mirrored`], [`waveform`], [`circular`] — are
//! pure functions from a spectrum buffer to lines of text, which is why they can
//! be tested with no audio device, no tap and no permission prompt, which is what
//! CI has to satisfy (`docs/ARCHITECTURE.md`).
//!
//! **Simulated is the default, not a placeholder.** TODO 1.5 refuted R1 — a
//! global tap needs no "System Audio Recording" grant at all — but it
//! materialised R6: every process-specific tap description fails with
//! `kAudioHardwareBadObjectError` (`docs/AUDIO-TAP.md` §3c). Tapping all system
//! audio instead would capture Sonar's as well, which is what COMPAT rule 3
//! exists to prevent, so 8.3's tap stays blocked on an `objc2` bypass of the
//! binding. Until that lands, a simulated spectrum is what ships, and it has to
//! be good enough to look like music: smooth frame to frame, driven by the
//! playback position, different per track, and silent when paused.
//!
//! Every renderer takes band magnitudes in 0..=1, a width and a height, and
//! returns exactly `height` lines of exactly `width` columns. Including at 0 and
//! 1: the pane this draws into can be any size at all — a narrow terminal, a
//! stacked layout, a resize mid-frame — and a panic in the render loop takes the
//! whole TUI with it.

use std::f64::consts::{FRAC_PI_2, TAU};

/// A filled cell, a cell filled in its lower half, and the dot the circular
/// renderer marks cells with. All three are ambiguous-width glyphs: one column in
/// an ordinary terminal, two in a CJK one. Nothing here depends on which, and the
/// tests measure lines with `unicode-width`, which agrees with the common case.
const FULL: char = '█';
const HALF: char = '▄';
const DOT: char = '•';
const EMPTY: char = ' ';

/// The hole the circular renderer leaves in the middle of its ring, as a fraction
/// of the radius. The ring needs a hole or a loud track is a filled disc with no
/// structure to read.
const RING_HOLE: f64 = 0.18;

/// How much of the ring a silent band still fills. It keeps the quiet state from
/// being an empty pane.
const RING_FLOOR: f64 = 0.3;

/// A stream of band magnitudes for the renderers to draw.
///
/// The whole contract between where the numbers come from and what they look
/// like. Above it, nothing knows whether the bars came from a Core Audio tap, a
/// recorded fixture, or [`SimulatedSource`]; below it, the renderers know only
/// that there are some magnitudes in 0..=1.
///
/// `Send`, because a source belongs to a worker thread rather than to the event
/// loop: a tap must never block the UI thread (TODO 8.3), so it runs on its own
/// thread and the render path only ever reads what it has already produced.
/// `Sync` is deliberately *not* required. One owner at a time is the simplest
/// thing that can work, and it means a tap's ring buffer needs no lock; asking
/// for `Sync` would commit 8.3 to a locking discipline it should not have.
pub trait AudioSource: Send {
    /// The latest frame of band magnitudes, one per band, each in 0..=1.
    ///
    /// Normalising is a real obligation rather than a formality: `cavacore`'s
    /// output is not 0..=1, and with autosens it ramps from near zero over about
    /// a second (TODO 1.6). A tap therefore has to scale against the pane and a
    /// recent peak before it gets here, or the meaning of a bar depends on how
    /// long the tap has been listening.
    ///
    /// Takes `&mut self` because it advances — a tap moves its read cursor and
    /// its peak follower, a simulated source fades and decays. Call it once per
    /// frame, then hand the vector to a renderer.
    ///
    /// An owned `Vec` rather than a caller-provided buffer: at the 30 fps cap
    /// this is one small allocation a frame, and it keeps the borrow rules simple
    /// enough that a tap can hand back a slice of its own ring buffer without
    /// needing a second type for the same thing.
    fn spectrum(&mut self) -> Vec<f32>;
}

/// The number of bands [`SimulatedSource`] produces.
///
/// 32 is what TODO 1.6 settled on for `cavacore` against the same panes, so the
/// simulated and the real source draw the same shape and one fixture can be
/// developed against both.
pub const BARS: usize = 32;

/// One band's constants: where its wobble starts, and how fast it moves.
struct Band {
    phase: f64,
    rate: f64,
}

/// How much of a column one magnitude fills.
struct Cells {
    /// Cells drawn from the bottom up, all of them solid.
    full: usize,
    /// Whether the next cell up is partly drawn.
    partial: bool,
}

/// A spectrum that looks like music and is not.
///
/// Every frame is a pure function of the seed, the track key and the playback
/// position, so a test can assert on it and a screenshot can be reproduced. The
/// call order is the render loop's: say whether music is playing and where it is,
/// then take one spectrum, once per frame.
///
/// It is a *simulation*, not noise, and the difference is the whole point: the
/// bars drift instead of jumping, they keep the shape of a spectrum (energy
/// falling off with frequency), they pulse with a beat at the track's own
/// tempo, and they hold still when playback is paused. Random noise would pass a
/// "does it change" test and fail the eye in under a second.
pub struct SimulatedSource {
    /// Mixed into every track's constants so two sources with different seeds
    /// never draw the same shape.
    seed: u64,
    playing: bool,
    position_secs: f64,
    bands: Vec<Band>,
    /// Beats per minute for this track, which is what makes the pulse a pulse.
    bpm: f64,
    /// How loud this track is.
    energy: f64,
    /// The fade-in and decay envelope, 0..=1.
    level: f32,
}

impl SimulatedSource {
    /// A source whose every frame is a pure function of `seed`.
    pub fn new(seed: u64) -> Self {
        let mut src = SimulatedSource {
            seed,
            playing: true,
            position_secs: 0.0,
            bands: Vec::new(),
            bpm: 120.0,
            energy: 0.8,
            level: 1.0,
        };
        src.set_track("");
        src
    }

    /// Point the source at a different track.
    ///
    /// `key` is whatever identifies the song stably — the `spotify:track:` URI,
    /// or the artist and title when there is none, which is the case for an
    /// advert. The same key always draws the same spectrum for a given seed.
    ///
    /// The level drops to zero so a track change fades in rather than cutting,
    /// the way the real one would.
    pub fn set_track(&mut self, key: &str) {
        let track = fnv1a(key.as_bytes()) ^ self.seed.rotate_left(17);
        self.bands = (0..BARS)
            .map(|i| {
                Band {
                    phase: unit(mix(track, i as u64)) * TAU,
                    // Slow enough that a frame at the 30 fps cap of TODO 8.4 is a
                    // small step; a faster wobble would read as noise.
                    rate: 0.25 + 0.9 * unit(mix(track, i as u64 ^ 0x9e37_79b9_7f4a_7c15)),
                }
            })
            .collect();
        self.bpm = 84.0 + 64.0 * unit(mix(track, 0x5eed));
        self.energy = 0.55 + 0.45 * unit(mix(track, 0x00c0_ffee));
        self.level = 0.0;
    }

    /// Whether music is playing. A paused source decays to silence and a resumed
    /// one fades back in, so a pause reads as the music stopping rather than as
    /// the visualizer being switched off.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// The playback position in seconds, as `App::interpolated_position` reports
    /// it. That value advances smoothly between polls, which is what keeps the
    /// motion smooth; a seek is a jump in it and looks like one.
    ///
    /// A position that is not a sane number — AppleScript's `missing value`
    /// arrives as one — is read as silence rather than propagated into `sin`.
    pub fn set_position(&mut self, position_secs: f64) {
        self.position_secs = match position_secs {
            s if s.is_finite() && s > 0.0 => s,
            _ => 0.0,
        };
    }
}

impl AudioSource for SimulatedSource {
    fn spectrum(&mut self) -> Vec<f32> {
        // One per frame, so a pause and a resume are the same motion in opposite
        // directions and neither depends on wall-clock time, which would make the
        // source untestable.
        self.level = if self.playing {
            (self.level + 0.18).min(1.0)
        } else {
            self.level * 0.9
        };
        let t = self.position_secs;
        // cubed, so the pulse is narrow and percussive rather than a sine swell
        let beat = (0.5 + 0.5 * (TAU * t * self.bpm / 60.0).sin()).powi(3);
        let n = self.bands.len();
        let mut out = Vec::with_capacity(n);
        for (i, band) in self.bands.iter().enumerate() {
            // Real spectra lose energy as frequency rises. A flat profile across
            // the bands reads as a bug, not as music.
            let tilt = if n > 1 {
                1.0 - 0.55 * (i as f64 / (n - 1) as f64)
            } else {
                1.0
            };
            let wobble = 0.5 + 0.5 * (TAU * band.rate * t + band.phase).sin();
            let shape = 0.45 * wobble + 0.55 * beat;
            // The gamma lifts the quiet bands into view without inventing motion
            // the source does not have.
            let v = (self.energy * tilt * shape).clamp(0.0, 1.0).powf(0.8) as f32;
            out.push(v * self.level);
        }
        out
    }
}

/// Bars from the floor up, one column per band.
pub fn spectrum(bars: &[f32], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return blank(width, height);
    }
    draw_columns(&columns(bars, width), height)
}

/// The same bars mirrored left and right about the centre column, so the shape is
/// symmetric and reads the same from either side.
pub fn mirrored(bars: &[f32], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return blank(width, height);
    }
    let half = columns(bars, width);
    let cols: Vec<f32> = (0..width).map(|x| half[x.min(width - 1 - x)]).collect();
    draw_columns(&cols, height)
}

/// A left-to-right trace, drawn in braille dots.
///
/// A braille cell is two dot columns by four dot rows, so a `width` × `height`
/// pane is a `2*width` × `4*height` grid: four times the vertical detail of the
/// block glyphs at one character per column. The approach follows scope-tui's,
/// which SPEC §7 points at.
///
/// Dots between two samples are filled in rather than sampled. Without that the
/// trace breaks into dashes wherever it moves more than one dot row inside a
/// single cell, which is most of the time.
pub fn waveform(bars: &[f32], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return blank(width, height);
    }
    let cols = columns(bars, width);
    let rows = height * 4;
    // One entry per dot *column*, so the trace can be walked along it and the
    // dots between two samples filled in.
    let mut dots = vec![vec![false; rows]; width * 2];
    let mut prev: Option<usize> = None;
    for (j, column) in dots.iter_mut().enumerate() {
        let at = sample(&cols, j, 2);
        let row = (f64::from((1.0 - at).clamp(0.0, 1.0)) * (rows - 1) as f64).round();
        let row = (row as usize).min(rows - 1);
        let span = match prev {
            Some(p) => p.min(row)..=p.max(row),
            None => row..=row,
        };
        for r in span {
            column[r] = true;
        }
        prev = Some(row);
    }
    let mut cells = vec![vec![0u8; width]; height];
    for (j, column) in dots.iter().enumerate() {
        for (r, &on) in column.iter().enumerate() {
            if on {
                cells[r / 4][j / 2] |= dot_bit(j % 2, r % 4);
            }
        }
    }
    lines(
        cells
            .iter()
            .map(|row| row.iter().map(|&bits| braille(bits)).collect())
            .collect(),
    )
}

/// The bars wrapped around a ring, so the shape reads the same from every side
/// and a track's loudest band is a direction as much as a height.
///
/// Drawn cell by cell rather than band by band. Marching each band along its own
/// ray leaves gaps between the bands on a small pane, and on an even-width pane
/// it puts a dot exactly on the centre line, where no cell can hold it — which
/// leaves the ring half a cell off the middle. Working out where each cell falls
/// in the ring instead makes a flat spectrum come out exactly symmetric.
pub fn circular(bars: &[f32], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return blank(width, height);
    }
    let cols = columns(bars, width);
    if cols.is_empty() {
        return blank(width, height);
    }
    let n = cols.len();
    let mut grid = vec![vec![EMPTY; width]; height];
    // The pane is rarely square, so the ring is an ellipse: one radius for both
    // axes squashes it into a lens on a wide pane.
    let (cx, cy) = ((width - 1) as f64 / 2.0, (height - 1) as f64 / 2.0);
    let (rx, ry) = (width as f64 / 2.0, height as f64 / 2.0);
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let (ux, uy) = ((x as f64 - cx) / rx, (y as f64 - cy) / ry);
            let rho = (ux * ux + uy * uy).sqrt();
            if rho > 1.0 {
                continue;
            }
            let v = cols[band(ux.atan2(uy), n)].clamp(0.0, 1.0) as f64;
            // Silence still draws a thin ring. An empty pane reads as a broken
            // visualizer rather than as a quiet song.
            let tip = RING_HOLE + (1.0 - RING_HOLE) * (RING_FLOOR + (1.0 - RING_FLOOR) * v);
            if rho >= RING_HOLE && rho <= tip {
                *cell = DOT;
            }
        }
    }
    lines(grid)
}

/// The band a point on the ring belongs to: band 0 at the top, running clockwise,
/// so the bass is at twelve o'clock the way it is at the left of a flat spectrum.
fn band(theta: f64, n: usize) -> usize {
    let turn = (theta + FRAC_PI_2).rem_euclid(TAU);
    (((turn / TAU) * n as f64) as usize).min(n - 1)
}

/// The magnitudes for each column of the pane.
///
/// Bands are averaged into their column, so narrowing a spectrum loses the noise
/// between bands instead of dropping every other one (which makes a tall thin
/// pane flicker). Fewer bands than columns repeats each one, which is what a
/// narrow pane on a wide spectrum needs.
fn columns(bars: &[f32], width: usize) -> Vec<f32> {
    let n = bars.len();
    let mut out = vec![0.0; width];
    if width == 0 || n == 0 {
        return out;
    }
    for (x, cell) in out.iter_mut().enumerate() {
        let lo = x * n / width;
        let hi = ((x + 1) * n / width).max(lo + 1).min(n);
        // `max` rather than a clamp, so a NaN from the DSP drops out here
        // instead of poisoning every column it touches.
        let sum: f32 = bars[lo..hi].iter().map(|b| b.max(0.0)).sum();
        *cell = sum / (hi - lo) as f32;
    }
    out
}

/// How much of a column one magnitude fills. The partial cell is drawn as a lower
/// half block: the bar grows upwards from the floor, so the part of the topmost
/// cell inside the bar is the part nearest the base.
fn filled_cells(v: f32, height: usize) -> Cells {
    let filled = v.clamp(0.0, 1.0) as f64 * height as f64;
    let full = filled.floor().max(0.0) as usize;
    Cells {
        full: full.min(height),
        partial: filled - full as f64 > 0.0 && full < height,
    }
}

/// Floor-anchored columns of blocks.
fn draw_columns(cols: &[f32], height: usize) -> Vec<String> {
    let width = cols.len();
    let mut grid = vec![vec![EMPTY; width]; height];
    for (x, &v) in cols.iter().enumerate() {
        let cells = filled_cells(v, height);
        for k in 0..cells.full {
            grid[height - 1 - k][x] = FULL;
        }
        if cells.partial {
            grid[height - 1 - cells.full][x] = HALF;
        }
    }
    lines(grid)
}

/// The magnitude at dot column `j`, interpolated across its cell so the trace
/// glides instead of stepping once per cell.
fn sample(cols: &[f32], j: usize, per_cell: usize) -> f32 {
    if cols.is_empty() {
        return 0.0;
    }
    let x = j as f32 / per_cell as f32;
    let i = (x.floor().max(0.0) as usize).min(cols.len() - 1);
    let next = (i + 1).min(cols.len() - 1);
    cols[i] + (cols[next] - cols[i]) * (x - i as f32).clamp(0.0, 1.0)
}

/// The bit for one dot of a braille cell, `col` across and `row` down.
///
/// The Unicode braille patterns number their dots column by column: 1-4 down
/// the left, 5-8 down the right.
fn dot_bit(col: usize, row: usize) -> u8 {
    debug_assert!(col < 2 && row < 4, "a cell is two dots by four");
    let bit = if row < 3 { row + 3 * col } else { 6 + col };
    1u8 << bit
}

/// The character for a cell's dot bits, or a space when no dot is set — an empty
/// braille cell would still paint a background.
fn braille(bits: u8) -> char {
    if bits == 0 {
        return EMPTY;
    }
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(EMPTY)
}

/// A pane-sized block of blanks: `height` lines of `width` columns, which is what
/// every renderer returns for a pane with no room in it.
fn blank(width: usize, height: usize) -> Vec<String> {
    vec![" ".repeat(width); height]
}

/// The grid as the lines that fill the pane.
fn lines(grid: Vec<Vec<char>>) -> Vec<String> {
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// FNV-1a, as `art.rs` uses it: five lines, and stable across toolchain
/// upgrades in a way `DefaultHasher` is not.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fold a counter into a value so `mix(track, 0)`, `mix(track, 1)` … do not walk
/// a correlated line through the hash's output.
fn mix(value: u64, counter: u64) -> u64 {
    let mut h = value ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// A hashed value in 0..1.
fn unit(h: u64) -> f64 {
    // 53 bits is the most a f64 can hold exactly, so the whole top of the range
    // is reachable instead of being rounded up to 1.0 for a quarter of it.
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

    /// Every pane size a renderer can be handed, including the degenerate ones.
    const SIZES: [(usize, usize); 15] = [
        (0, 0),
        (1, 1),
        (0, 4),
        (4, 0),
        (1, 3),
        (3, 1),
        (2, 1),
        (3, 2),
        (7, 3),
        (8, 4),
        (13, 5),
        (16, 9),
        (21, 7),
        (33, 17),
        (120, 40),
    ];

    /// One of the renderers under test, by the name a failure should quote.
    type Renderer = fn(&[f32], usize, usize) -> Vec<String>;

    /// The four renderers, so one assertion covers all of them.
    const RENDERERS: [(&str, Renderer); 4] = [
        ("spectrum", spectrum),
        ("mirrored", mirrored),
        ("waveform", waveform),
        ("circular", circular),
    ];

    fn flat(len: usize, v: f32) -> Vec<f32> {
        vec![v; len]
    }

    /// One band loud, the rest at `rest`, so a peak has a floor to stand out
    /// against.
    fn peak(len: usize, at: usize, rest: f32) -> Vec<f32> {
        let mut bars = flat(len, rest);
        if let Some(b) = bars.get_mut(at) {
            *b = 1.0;
        }
        bars
    }

    /// Values a DSP or a test can produce that are not magnitudes: NaN, both
    /// infinities, a negative, and a value no pane can draw.
    fn junk() -> Vec<f32> {
        vec![
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0,
            1e30,
            0.0,
            0.5,
            f32::NAN,
        ]
    }

    fn render(name: &str, bars: &[f32], w: usize, h: usize) -> Vec<String> {
        let f = RENDERERS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, f)| *f)
            .expect("a renderer by that name");
        f(bars, w, h)
    }

    /// The contract every renderer owes its pane.
    fn assert_shape(name: &str, out: &[String], width: usize, height: usize) {
        assert_eq!(
            out.len(),
            height,
            "{name} at {width}x{height} gave {} lines",
            out.len()
        );
        for (y, line) in out.iter().enumerate() {
            assert_eq!(
                UnicodeWidthStr::width(line.as_str()),
                width,
                "{name} at {width}x{height}, line {y}: {line:?}"
            );
        }
    }

    /// Ink per column: how many cells a column has drawn, top to bottom.
    fn ink_per_column(out: &[String]) -> Vec<usize> {
        let width = out
            .first()
            .map_or(0, |l| UnicodeWidthStr::width(l.as_str()));
        (0..width)
            .map(|x| {
                out.iter()
                    .filter(|line| line.chars().nth(x).is_some_and(|c| c != EMPTY))
                    .count()
            })
            .collect()
    }

    /// A braille character's dot bits, so a test can count the dots in a cell.
    fn bits(c: char) -> u32 {
        (c as u32).wrapping_sub(0x2800)
    }

    /// A drawn cell, whatever the style draws cells with.
    fn is_dot(c: char) -> bool {
        c == DOT || ('\u{2800}'..='\u{28ff}').contains(&c)
    }

    /// Dots in a range of columns, counted in cells rather than bytes: `•` is one
    /// column and three bytes.
    fn dots_between(out: &[String], from: usize, to: usize) -> usize {
        out.iter()
            .map(|l| {
                l.chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .filter(|c| *c == DOT)
                    .count()
            })
            .sum()
    }

    /// Drive a source the way the render loop does.
    fn frames(seed: u64, track: &str, from: f64, count: usize, dt: f64) -> Vec<Vec<f32>> {
        let mut src = SimulatedSource::new(seed);
        src.set_track(track);
        (0..count)
            .map(|i| {
                src.set_position(from + i as f64 * dt);
                src.spectrum()
            })
            .collect()
    }

    /// One spectrum from a source that has been playing long enough to be at full
    /// level, which is where the owner ever sees it.
    fn settled(seed: u64, track: &str, secs: f64) -> Vec<f32> {
        frames(seed, track, secs, 8, 0.0)
            .pop()
            .expect("eight frames")
    }

    fn max_delta(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from((*x - *y).abs()))
            .fold(0.0, f64::max)
    }

    fn mean_delta(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().max(1) as f64;
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from((*x - *y).abs()))
            .sum::<f64>()
            / n
    }

    #[test]
    fn every_renderer_fills_its_pane_at_every_size() {
        // Four inputs on purpose: nothing, a flat spectrum, a peak, and the values
        // a DSP or a parser can hand over that are not magnitudes at all. A NaN
        // that reached an index would be a panic in the render loop.
        for (name, render) in RENDERERS {
            for &(w, h) in &SIZES {
                for bars in [vec![], flat(32, 0.5), peak(32, 5, 0.1), junk()] {
                    assert_shape(name, &render(&bars, w, h), w, h);
                }
            }
        }
    }

    #[test]
    fn golden_buffers_per_style_at_three_sizes() {
        // One loud band over a quiet floor, at an odd and an even width and a
        // short pane: whatever a renderer draws from this has to stay drawn. A
        // change here means someone changed a glyph or the arithmetic behind it,
        // and that should be a deliberate act rather than a diff nobody reads.
        let bars = peak(16, 3, 0.25);
        let cases: [(&str, Renderer, usize, usize, &[&str]); 12] = [
            (
                "spectrum",
                spectrum,
                9,
                4,
                &["         ", "  ▄      ", "  █      ", "█████████"],
            ),
            (
                "spectrum",
                spectrum,
                10,
                4,
                &["  █       ", "  █       ", "  █       ", "██████████"],
            ),
            (
                "spectrum",
                spectrum,
                8,
                3,
                &["        ", " ▄      ", "▄█▄▄▄▄▄▄"],
            ),
            (
                "mirrored",
                mirrored,
                9,
                4,
                &["         ", "  ▄   ▄  ", "  █   █  ", "█████████"],
            ),
            (
                "mirrored",
                mirrored,
                10,
                4,
                &["  █    █  ", "  █    █  ", "  █    █  ", "██████████"],
            ),
            (
                "mirrored",
                mirrored,
                8,
                3,
                &["        ", " ▄    ▄ ", "▄█▄▄▄▄█▄"],
            ),
            (
                "waveform",
                waveform,
                9,
                4,
                &["         ", "  ⣤      ", "⣀⣸⠉⣇⣀⣀⣀⣀⣀", "         "],
            ),
            (
                "waveform",
                waveform,
                10,
                4,
                &["  ⣿       ", " ⢠⠿⡄      ", "⣀⣸ ⣇⣀⣀⣀⣀⣀⣀", "          "],
            ),
            (
                "waveform",
                waveform,
                8,
                3,
                &["        ", "⢠⠿⡄     ", "⠉ ⠉⠉⠉⠉⠉⠉"],
            ),
            (
                "circular",
                circular,
                9,
                4,
                &["         ", "  •••••  ", "  •••••  ", "    ••   "],
            ),
            (
                "circular",
                circular,
                10,
                4,
                &["          ", "  ••••••  ", "  ••••••  ", "    ••    "],
            ),
            (
                "circular",
                circular,
                8,
                3,
                &["        ", "  •  •  ", "  ••    "],
            ),
        ];
        for (name, render, w, h, expected) in cases {
            let out = render(&bars, w, h);
            assert_shape(name, &out, w, h);
            assert_eq!(out, expected, "{name} at {w}x{h}");
        }
    }

    #[test]
    fn spectrum_columns_average_when_there_are_more_bands_than_columns() {
        // Narrowing must not throw every other band away: that is what makes a
        // tall thin pane flicker instead of settling.
        let bars = vec![0.0, 1.0, 0.0, 1.0];
        assert_eq!(columns(&bars, 2), vec![0.5, 0.5]);
        assert_eq!(columns(&bars, 4), bars);
        assert_eq!(columns(&bars, 2)[0], 0.5, "the mean, not the peak");
    }

    #[test]
    fn spectrum_columns_repeat_when_there_are_more_columns_than_bands() {
        assert_eq!(columns(&[0.25], 4), vec![0.25; 4]);
        assert_eq!(columns(&[0.25, 0.75], 4), vec![0.25, 0.25, 0.75, 0.75]);
    }

    #[test]
    fn a_flat_spectrum_draws_flat_bars() {
        let out = render("spectrum", &flat(32, 0.5), 16, 5);
        assert_shape("spectrum", &out, 16, 5);
        for (y, line) in out.iter().enumerate() {
            // A flat spectrum is the same height everywhere, so each line is one
            // repeated glyph rather than a row of ragged tops.
            let first = line.chars().next().expect("a cell");
            assert!(
                line.chars().all(|c| c == first),
                "line {y} is not uniform: {line:?}"
            );
        }
        assert_eq!(ink_per_column(&out), vec![3; 16]);
    }

    #[test]
    fn a_peak_draws_one_tall_bar() {
        let out = render("spectrum", &peak(16, 8, 0.0), 16, 6);
        assert_shape("spectrum", &out, 16, 6);
        let ink = ink_per_column(&out);
        let tallest = *ink.iter().max().expect("a column");
        assert_eq!(tallest, 6, "a full-height band fills its column: {ink:?}");
        assert_eq!(ink[8], tallest, "and it is the band that was loud: {ink:?}");
        assert_eq!(
            ink.iter().filter(|&&h| h == tallest).count(),
            1,
            "one peak: {ink:?}"
        );
        assert_eq!(
            ink.iter().filter(|&&h| h > 0).count(),
            1,
            "and no other bar: {ink:?}"
        );
        assert_eq!(
            out[0].matches(FULL).count(),
            1,
            "up at the ceiling: {out:?}"
        );
    }

    #[test]
    fn a_louder_band_draws_a_taller_bar() {
        let quiet = render("spectrum", &flat(32, 0.2), 8, 10);
        let loud = render("spectrum", &flat(32, 0.9), 8, 10);
        assert!(
            ink_per_column(&loud)[0] > ink_per_column(&quiet)[0],
            "0.9 fills more cells than 0.2"
        );
    }

    #[test]
    fn a_stronger_band_draws_a_taller_partial_cell() {
        // Two values that both land inside the same cell: the half block is what
        // tells them apart, so a renderer that only drew whole cells would be
        // quantised to the pane height and look stepped.
        let just_under = render("spectrum", &flat(32, 0.49), 4, 2);
        let just_over = render("spectrum", &flat(32, 0.51), 4, 2);
        let floor = just_under.last().expect("the bottom row");
        assert!(
            floor.contains(HALF),
            "0.49 of 2 cells is a half block: {just_under:?}"
        );
        assert!(
            just_over.last().expect("the bottom row").contains(FULL),
            "0.51 of 2 cells has crossed into the next one: {just_over:?}"
        );
    }

    #[test]
    fn mirrored_is_symmetric_about_the_centre() {
        for &(w, h) in &SIZES {
            let out = render("mirrored", &peak(32, 3, 0.2), w, h);
            assert_shape("mirrored", &out, w, h);
            for line in &out {
                let reversed: String = line.chars().rev().collect();
                assert_eq!(*line, reversed, "mirrored at {w}x{h}: {line:?}");
            }
        }
    }

    #[test]
    fn mirrored_shows_the_peak_twice() {
        let out = render("mirrored", &peak(16, 0, 0.0), 16, 4);
        assert_shape("mirrored", &out, 16, 4);
        let ink = ink_per_column(&out);
        assert_eq!(ink[0], 4, "the band, in its own column");
        assert_eq!(ink[15], 4, "and again, mirrored");
        assert_eq!(
            ink.iter().filter(|&&h| h > 0).count(),
            2,
            "no other bars: {ink:?}"
        );
    }

    #[test]
    fn a_flat_waveform_draws_a_straight_line() {
        let out = render("waveform", &flat(32, 0.5), 20, 8);
        assert_shape("waveform", &out, 20, 8);
        let lit: Vec<&String> = out.iter().filter(|l| l.chars().any(is_dot)).collect();
        assert_eq!(lit.len(), 1, "one row of the dot grid: {out:?}");
        let chars: Vec<char> = lit[0].chars().collect();
        assert!(
            chars.windows(2).all(|w| w[0] == w[1]),
            "a flat trace is the same dot in every cell: {lit:?}"
        );
    }

    #[test]
    fn a_peak_in_a_waveform_stands_out_of_its_row() {
        let flat = render("waveform", &flat(32, 0.5), 20, 8);
        let peaked = render("waveform", &peak(32, 12, 0.4), 20, 8);
        assert_ne!(flat, peaked, "the trace must depend on its input");
        let dots = |out: &Vec<String>| -> Vec<u32> {
            out.iter()
                .flat_map(|l| l.chars())
                .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
                .map(bits)
                .collect()
        };
        let baseline = dots(&flat).into_iter().max().expect("a trace");
        let peak = dots(&peaked).into_iter().max().expect("a trace");
        assert!(
            peak > baseline,
            "the peak cell carries more dots ({peak}) than a flat one ({baseline})"
        );
    }

    #[test]
    fn waveform_tells_every_dot_of_a_cell_apart() {
        let mut seen: Vec<char> = Vec::new();
        for col in 0..2 {
            for row in 0..4 {
                let ch = braille(dot_bit(col, row));
                assert!(
                    ('\u{2801}'..='\u{28ff}').contains(&ch),
                    "dots {col},{row} drew {ch:?}, which is not a braille pattern"
                );
                assert!(
                    !seen.contains(&ch),
                    "dots {col},{row} collide with an earlier dot at {ch:?}"
                );
                seen.push(ch);
            }
        }
        assert_eq!(seen.len(), 8, "all eight sub-cells are distinct");
    }

    #[test]
    fn waveform_connects_the_dots_between_two_samples() {
        // A trace that falls from the top of the cell to the bottom of the next
        // one has to be filled in, or it arrives as unrelated marks: four dot
        // columns and one dot each.
        let out = render("waveform", &[1.0, 0.0], 2, 1);
        assert_shape("waveform", &out, 2, 1);
        let drawn: usize = out[0].chars().map(|c| bits(c).count_ones() as usize).sum();
        assert!(
            drawn > 4,
            "the trace must join up between the dots: {out:?}"
        );
    }

    #[test]
    fn a_flat_circular_spectrum_draws_a_ring() {
        let out = render("circular", &flat(32, 0.0), 20, 9);
        assert_shape("circular", &out, 20, 9);
        let lit = out.iter().filter(|l| l.contains(DOT)).count();
        assert!(lit >= 3, "a ring fills more than one row: {out:?}");
        assert!(
            out[4].contains(DOT),
            "and it is centred on the pane: {out:?}"
        );
        for line in &out {
            let reversed: String = line.chars().rev().collect();
            assert_eq!(*line, reversed, "a uniform ring is symmetric: {line:?}");
        }
    }

    #[test]
    fn a_peak_in_a_circular_spectrum_pushes_out_on_one_side() {
        // Band 8 of 24 is three o'clock, so a peak there must reach further right
        // than the ring does and further right than the ring's left edge.
        let ring = render("circular", &flat(32, 0.0), 24, 11);
        let peaked = render("circular", &peak(24, 8, 0.0), 24, 11);
        let right = |out: &Vec<String>| dots_between(out, 12, 24);
        let left = |out: &Vec<String>| dots_between(out, 0, 12);
        assert_eq!(
            left(&peaked),
            left(&ring),
            "nothing moved on the far side: {peaked:?}"
        );
        assert!(
            right(&peaked) > right(&ring),
            "and the band reached out: {peaked:?}"
        );
        assert!(right(&ring) == left(&ring), "a flat ring is symmetric");
    }

    #[test]
    fn a_circular_ring_is_symmetric_at_both_parities() {
        // A ring that is half a cell off its centre is not a ring. Marching the
        // bands along their own rays does exactly that on an even-width pane,
        // where the centre line falls between two cells.
        for &w in &[20, 21, 40, 41] {
            for &h in &[3, 5, 9] {
                let out = render("circular", &flat(32, 0.0), w, h);
                for line in &out {
                    let reversed: String = line.chars().rev().collect();
                    assert_eq!(*line, reversed, "{w}x{h}: {line:?}");
                }
                for flipped in out.iter().rev() {
                    assert!(
                        out.contains(flipped),
                        "{w}x{h}: {out:?} is not top-bottom symmetric"
                    );
                }
            }
        }
    }

    #[test]
    fn the_simulated_source_is_reproducible_from_its_seed() {
        let a = settled(7, "spotify:track:abc", 42.5);
        let b = settled(7, "spotify:track:abc", 42.5);
        assert_eq!(a, b, "same seed, same track, same position");
        assert_ne!(
            a,
            settled(8, "spotify:track:abc", 42.5),
            "a different seed differs"
        );
    }

    #[test]
    fn the_simulated_source_differs_per_track() {
        let one = settled(7, "spotify:track:abc", 30.0);
        let two = settled(7, "spotify:track:xyz", 30.0);
        assert_ne!(one, two, "two tracks must not draw the same spectrum");
        for bars in [&one, &two] {
            let peak = bars.iter().copied().fold(0.0, f32::max);
            assert!(
                peak > 0.3,
                "and each must look like music, not like a flat line: {peak}"
            );
        }
    }

    #[test]
    fn the_simulated_source_follows_the_playback_position() {
        let early = settled(7, "spotify:track:abc", 1.0);
        let later = settled(7, "spotify:track:abc", 61.0);
        assert_ne!(
            early, later,
            "the same track a minute in is a different shape"
        );
        assert_eq!(early.len(), BARS);
    }

    #[test]
    fn the_simulated_source_is_smooth_frame_to_frame() {
        // 30 fps is the cap TODO 8.4 sets, so a third of a second is the step the
        // owner ever sees.
        let at30 = frames(7, "spotify:track:abc", 100.0, 120, 1.0 / 30.0);
        let drift = at30
            .windows(2)
            .map(|p| mean_delta(&p[0], &p[1]))
            .sum::<f64>()
            / (at30.len() - 1) as f64;
        // The signature of continuity rather than noise: neighbouring moments
        // resemble each other and moments a third of a second apart do not.
        // Reseeded bars would show neither.
        let distant = at30
            .windows(10)
            .map(|p| mean_delta(&p[0], &p[9]))
            .sum::<f64>()
            / (at30.len() - 9) as f64;
        assert!(
            drift * 2.0 < distant,
            "it drifts: {drift} against {distant}"
        );
        let biggest = at30
            .windows(2)
            .map(|p| max_delta(&p[0], &p[1]))
            .fold(0.0, f64::max);
        assert!(biggest < 0.3, "and never jumps: {biggest}");

        // Finer steps move less, which is the other half of the same claim.
        let at60 = frames(7, "spotify:track:abc", 100.0, 120, 1.0 / 60.0);
        let finer = at60
            .windows(2)
            .map(|p| max_delta(&p[0], &p[1]))
            .fold(0.0, f64::max);
        assert!(
            finer < biggest * 0.8,
            "half the step, less than the move: {finer} vs {biggest}"
        );
    }

    #[test]
    fn the_simulated_source_decays_to_nothing_while_paused() {
        let mut src = SimulatedSource::new(7);
        src.set_track("spotify:track:abc");
        src.set_position(30.0);
        for _ in 0..8 {
            src.spectrum();
        }
        let mut last = src.spectrum();
        let loud = last.iter().copied().fold(0.0, f32::max);
        assert!(loud > 0.3, "it was playing: {loud}");
        src.set_playing(false);
        for _ in 0..90 {
            let bars = src.spectrum();
            let peak = bars.iter().copied().fold(0.0, f32::max);
            assert!(
                peak <= last.iter().copied().fold(0.0, f32::max),
                "a pause only fades"
            );
            last = bars;
        }
        assert!(
            last.iter().copied().fold(0.0, f32::max) < 0.005,
            "and it reaches silence"
        );
    }

    #[test]
    fn the_simulated_source_comes_back_when_playback_resumes() {
        let mut src = SimulatedSource::new(7);
        src.set_track("spotify:track:abc");
        src.set_position(30.0);
        for _ in 0..90 {
            src.spectrum();
        }
        src.set_playing(false);
        for _ in 0..90 {
            src.spectrum();
        }
        src.set_playing(true);
        let mut bars = vec![0.0];
        for _ in 0..12 {
            bars = src.spectrum();
        }
        assert!(
            bars.iter().copied().fold(0.0, f32::max) > 0.5,
            "a resume fades back in: {bars:?}"
        );
    }

    #[test]
    fn a_track_change_starts_from_silence() {
        let mut src = SimulatedSource::new(7);
        src.set_track("spotify:track:abc");
        src.set_position(12.0);
        src.spectrum();
        src.set_track("spotify:track:xyz");
        assert!(
            src.spectrum().iter().copied().fold(0.0, f32::max) < 0.25,
            "a cut between tracks should be a fade"
        );
    }

    #[test]
    fn every_simulated_band_is_a_magnitude() {
        for seed in 0..8u64 {
            for step in 0..40 {
                for bars in frames(seed, "spotify:track:abc", step as f64 * 7.0, 1, 1.0 / 30.0) {
                    assert_eq!(bars.len(), BARS);
                    for (i, v) in bars.iter().enumerate() {
                        assert!(
                            v.is_finite() && (0.0..=1.0).contains(v),
                            "seed {seed}, band {i}: {v}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_simulated_source_survives_a_nonsense_position() {
        for secs in [f64::NAN, -1.0, 0.0, f64::INFINITY, 1e300] {
            for bars in frames(7, "spotify:track:abc", secs, 2, 1.0 / 30.0) {
                assert!(
                    bars.iter()
                        .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                    "position {secs}: {bars:?}"
                );
            }
        }
    }

    #[test]
    fn the_simulated_source_can_be_moved_to_another_thread() {
        // The reason `AudioSource: Send` is part of the contract, and TODO 8.3's
        // tap will need it.
        fn assert_send<T: Send>() {}
        assert_send::<Box<dyn AudioSource>>();
    }

    #[test]
    fn a_simulated_spectrum_is_something_the_renderers_can_draw() {
        let bars = settled(7, "spotify:track:abc", 30.0);
        for (name, render) in RENDERERS {
            let out = render(&bars, 21, 7);
            assert_shape(name, &out, 21, 7);
            let ink = out
                .iter()
                .map(|l| l.chars().filter(|c| *c != EMPTY).count())
                .sum::<usize>();
            assert!(ink > 0, "{name} drew nothing at all for a real spectrum");
        }
    }
}
