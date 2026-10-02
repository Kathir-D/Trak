//! Picking one colour out of an album cover, for the accent (TODO 4.2).
//!
//! The rule that makes this usable rather than merely clever: **a dominant
//! colour is not automatically a good UI colour.** Most covers are dominated by
//! black, white or beige, and tinting a whole interface with the beige corner of
//! an album sleeve is worse than not tinting it at all. So the extractor throws
//! away the near-black and near-white pixels, prefers the *saturated* remainder,
//! and then checks the result is dark enough to read white text on and light
//! enough to read black text on — which is the constraint that actually matters
//! for a terminal.
//!
//! It works on a tiny copy of the image. The cover is already in memory as
//! `DynamicImage`; shrinking it to about 32 pixels is a few hundred microseconds
//! and makes the result depend on the *composition* of the cover rather than on
//! which exact pixels happened to be sampled.
//!
//! **One measure throughout: WCAG relative luminance.** The first version mixed
//! HSL lightness (for rejecting pixels) with WCAG luminance (for fixing up the
//! result), which meant colours the filter had approved got "corrected" into
//! different colours. Lightness and luminance are not the same scale and neither
//! is wrong; using both is just wrong.

use image::imageops::FilterType;
use ratatui::style::Color;

/// Side length of the square the cover is shrunk to before sampling. 32×32 is
/// 1024 samples, which is plenty for a dominant colour and costs nothing.
const SAMPLE: u32 = 32;

/// Below this an accent disappears into a **dark** terminal background, and above
/// this it disappears into a **light** one. The band is the whole point: the
/// terminal's colours are unknown, so the accent has to work on both, and that
/// is a narrower requirement than "legible with some text".
const MIN_LUMA: f32 = 0.10;
const MAX_LUMA: f32 = 0.60;
/// Saturation below this is a grey, and a grey accent is indistinguishable from
/// "no accent", so it is rejected.
const MIN_SATURATION: f32 = 0.20;
/// A colour this far from grey is a strong candidate, and one above it wins over
/// everything else if it also passes the luma test.
const STRONG_SATURATION: f32 = 0.45;
/// The contrast ratio an accent has to reach against whichever of black or white
/// text is put on it. 3:1 is the WCAG threshold for a UI component rather than
/// body text, which is what an accent colour is.
const MIN_CONTRAST: f32 = 3.0;

/// The accent for a cover, or `None` when nothing in it is usable.
///
/// `None` is a real answer, not a failure: a black-and-white cover should leave
/// the accent alone rather than pick a colour out of the noise.
pub fn dominant_colour(image: &image::DynamicImage) -> Option<Color> {
    let small = image::imageops::resize(&image.to_rgba8(), SAMPLE, SAMPLE, FilterType::Triangle);
    from_samples(small.as_raw())
}

/// The decision, separated from the sampling so it can be tested against exact
/// pixel values.
fn from_samples(rgba: &[u8]) -> Option<Color> {
    assert_eq!(rgba.len() % 4, 0, "RGBA pixels are four bytes each");
    let (mut best, mut best_score) = (None, 0.0f32);
    let (mut fallback, mut fallback_score) = (None, 0.0f32);

    for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
        // Spotify artwork is opaque, but a PNG with alpha is not, and a fully
        // transparent pixel says nothing about the cover.
        if a < 128 {
            continue;
        }
        let (_h, s, _lightness) = hsl(r, g, b);
        let y = luma(r, g, b);
        if s < MIN_SATURATION || y < MIN_LUMA || y > MAX_LUMA {
            continue;
        }
        // Weight saturation by how central the luminance is: a colour at the very
        // edge of the usable band is one quantisation step from disappearing into
        // a background of the other kind.
        let middle = (MIN_LUMA + MAX_LUMA) / 2.0;
        let span = (MAX_LUMA - MIN_LUMA) / 2.0;
        let centrality = 1.0 - (y - middle).abs() / span;
        let score = s * 0.7 + centrality * 0.3;
        if score > best_score {
            best = Some((r, g, b));
            best_score = score;
        }
        if s >= STRONG_SATURATION && score > fallback_score {
            fallback = Some((r, g, b));
            fallback_score = score;
        }
    }

    // A strongly saturated candidate beats a merely central one: it is the
    // colour a person would name if asked what colour this album is.
    let (r, g, b) = fallback.or(best)?;
    Some(Color::Rgb(r, g, b))
}

/// RGB to hue in degrees, saturation and lightness in 0..=1.
fn hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let l = (max + min) / 2.0;
    if delta == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = delta / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), l)
}

/// Clamp a colour's luminance into the band where it works on both a light and a
/// dark terminal, preserving its hue.
///
/// Scaling the channels by a constant preserves the hue exactly, which is the
/// point: nudging each channel towards grey would turn a red accent pink. A
/// colour already inside the band comes back untouched, and terminal palette
/// colours are not ours to change.
pub fn ensure_contrast(colour: Color) -> Color {
    let Color::Rgb(r, g, b) = colour else {
        return colour;
    };
    let l = luma(r, g, b);
    if l <= 0.0 {
        // Pure black cannot be scaled into the band: there is no hue to preserve
        // and no magnitude to multiply. The caller is expected to fall back (see
        // `is_usable`), and pretending otherwise would invent a colour.
        return colour;
    }
    if l < MIN_LUMA {
        let k = MIN_LUMA / l;
        Color::Rgb(
            (f32::from(r) * k).min(255.0) as u8,
            (f32::from(g) * k).min(255.0) as u8,
            (f32::from(b) * k).min(255.0) as u8,
        )
    } else if l > MAX_LUMA {
        let k = MAX_LUMA / l;
        Color::Rgb(
            (f32::from(r) * k) as u8,
            (f32::from(g) * k) as u8,
            (f32::from(b) * k) as u8,
        )
    } else {
        colour
    }
}

/// The text colour to put **on** an accent: whichever of black or white reaches
/// the better contrast ratio, so the selected tab stays readable whatever colour
/// the cover turned out to be.
pub fn text_on(accent: Color) -> Color {
    let l = luma_of(accent);
    let on_white = 1.05 / (l + 0.05);
    let on_black = (l + 0.05) / 0.05;
    if on_black >= on_white {
        Color::Black
    } else {
        Color::White
    }
}

/// Luminance of a colour, understanding the four named ones that matter here.
///
/// The named colours are *not* all mid-grey: an earlier version of this treated
/// every non-`Rgb` as 0.5, which made `contrast_ratio(accent, White)` come out at
/// 2.7:1 for a colour that is really 5:1 against white, and the test that was
/// meant to prove legibility failed for a colour that was fine. Terminal palette
/// colours genuinely are unknowable; black and white are not.
fn luma_of(colour: Color) -> f32 {
    match colour {
        Color::Rgb(r, g, b) => luma(r, g, b),
        Color::Black => 0.0,
        Color::White => 1.0,
        _ => 0.5,
    }
}

/// The WCAG contrast ratio between two colours.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (x, y) = (luma_of(a), luma_of(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// Whether a colour is usable as an accent at all: it has to be inside the
/// luminance band, and it has to be able to carry text.
///
/// This is what the theme asks before using a colour from the cover, and it is
/// the single place the rule lives.
pub fn is_usable(colour: Color) -> bool {
    let l = luma_of(colour);
    if !(MIN_LUMA..=MAX_LUMA).contains(&l) {
        return false;
    }
    // A grey accent is indistinguishable from having no accent at all, so the
    // saturation floor is part of "usable" rather than only a sampling filter.
    // (For grey, `hsl` returns hue 0, so the hue alone cannot be the test.)
    if saturation_of(colour) < MIN_SATURATION {
        return false;
    }
    contrast_ratio(colour, text_on(colour)) >= MIN_CONTRAST
}

/// HSL saturation of a colour, 0..=1. Split out so `is_usable` does not have to
/// unpack a tuple it only wants one field of.
pub fn saturation_of(colour: Color) -> f32 {
    let Color::Rgb(r, g, b) = colour else {
        // A named terminal colour: we cannot know, so assume it is usable and
        // leave the decision to the user's palette.
        return 1.0;
    };
    hsl(r, g, b).1
}

/// Relative luminance, the WCAG definition, which is what "can you read this"
/// actually means.
pub fn luma(r: u8, g: u8, b: u8) -> f32 {
    let f = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn solid(r: u8, g: u8, b: u8) -> image::DynamicImage {
        let mut img = RgbImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = Rgb([r, g, b]);
        }
        image::DynamicImage::ImageRgb8(img)
    }

    /// Colours as solid quadrants of a 32×32 image, so a test's composition is
    /// exact *and* survives the extractor's downscale. A 4×4 fixture does not: the
    /// resampling smears every colour into its black neighbours and the filter
    /// then rejects the lot, which looks like a broken extractor and is not.
    fn of(colours: &[(u8, u8, u8)]) -> image::DynamicImage {
        let n = colours.len().max(1);
        let mut img = RgbImage::new(32, 32);
        let cols = n.min(4);
        for (i, p) in img.pixels_mut().enumerate() {
            let (x, y) = (i % 32, i / 32);
            let which = (y / 16) * cols + (x / (32 / cols));
            let (r, g, b) = colours[which % n];
            *p = Rgb([r, g, b]);
        }
        image::DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn a_saturated_colour_is_picked() {
        assert_eq!(
            dominant_colour(&solid(200, 40, 40)),
            Some(Color::Rgb(200, 40, 40))
        );
    }

    /// The case the whole module exists for: most covers are mostly black, and a
    /// black accent would make the interface unreadable.
    #[test]
    fn a_mostly_black_cover_does_not_produce_a_black_accent() {
        // Three quarters black, one quadrant of a deep blue.
        let img = of(&[(0, 0, 0), (0, 0, 0), (0, 0, 0), (60, 120, 220)]);
        let got = dominant_colour(&img).expect("one usable pixel is enough");
        let Color::Rgb(r, g, b) = got else {
            panic!("expected rgb, got {got:?}");
        };
        assert!(b > 150, "the one blue quadrant should win: {got:?}");
        assert!(luma(r, g, b) > MIN_LUMA, "and it must be legible: {got:?}");
    }

    /// A black-and-white cover has no usable colour, and the right answer is to
    /// change nothing rather than pick grey.
    #[test]
    fn a_greyscale_cover_yields_no_accent() {
        assert_eq!(dominant_colour(&solid(128, 128, 128)), None);
        assert_eq!(dominant_colour(&solid(0, 0, 0)), None);
        assert_eq!(dominant_colour(&solid(255, 255, 255)), None);
    }

    /// Beige and cream are the other common case: saturated enough to be a hue,
    /// but nearly white.
    #[test]
    fn a_pale_cover_is_rejected_rather_than_washed_out() {
        assert_eq!(dominant_colour(&solid(250, 245, 230)), None);
        // Dark enough, saturated enough: this one survives.
        let just_ok = dominant_colour(&solid(170, 130, 50));
        assert!(just_ok.is_some(), "a colour inside the band survives");
    }

    /// A strongly saturated colour beats a milder one that is more central.
    #[test]
    fn a_strong_colour_beats_a_mild_one() {
        // Three quarters of a mild olive, one of a strong red.
        let img = of(&[
            (120, 110, 60),
            (120, 110, 60),
            (120, 110, 60),
            (200, 30, 30),
        ]);
        let Color::Rgb(r, _, _) = dominant_colour(&img).expect("a colour") else {
            panic!("expected rgb");
        };
        assert!(r > 180, "the red quarter should win: {r}");
    }

    /// Transparent pixels say nothing about the cover and must not be counted.
    #[test]
    fn transparent_pixels_are_ignored() {
        let mut img = image::RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = image::Rgba([255, 0, 0, 0]);
        }
        assert_eq!(
            dominant_colour(&image::DynamicImage::ImageRgba8(img)),
            None,
            "a fully transparent image has no colour to offer"
        );
    }

    /// The extractor must not panic on a tiny or awkward image.
    #[test]
    fn odd_sizes_do_not_panic() {
        for (w, h) in [(1, 1), (1, 7), (7, 1), (3, 3)] {
            let mut img = RgbImage::new(w, h);
            for p in img.pixels_mut() {
                *p = Rgb([200, 30, 90]);
            }
            let _ = dominant_colour(&image::DynamicImage::ImageRgb8(img));
        }
    }

    /// The band is a contract with the renderer, so `is_usable` is the single
    /// gate and it has to agree with the constants.
    #[test]
    fn usable_and_unusable_are_decided_in_one_place() {
        for good in [
            Color::Rgb(200, 40, 40),
            Color::Rgb(40, 200, 90),
            Color::Rgb(60, 120, 220),
            Color::Rgb(170, 130, 50),
        ] {
            assert!(is_usable(good), "{good:?} should be usable");
            assert_eq!(ensure_contrast(good), good, "and left alone");
        }
        for bad in [
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            Color::Rgb(250, 245, 230),
            Color::Rgb(128, 128, 128),
        ] {
            assert!(!is_usable(bad), "{bad:?} should be refused");
        }
    }

    #[test]
    fn contrast_helper_only_touches_unreadable_colours() {
        // Already inside the band: untouched, exactly.
        let good = Color::Rgb(200, 40, 40);
        assert_eq!(ensure_contrast(good), good);
        // Terminal colours are not ours to change.
        assert_eq!(ensure_contrast(Color::Red), Color::Red);
        assert_eq!(ensure_contrast(Color::Reset), Color::Reset);
    }

    /// The accent has to work on a light terminal and a dark one, and the text
    /// drawn on it has to be readable either way. This is the property that
    /// matters, so it is asserted for every colour the extractor can produce.
    #[test]
    fn every_accent_can_carry_text_and_survives_a_light_and_a_dark_terminal() {
        for img in [
            solid(200, 40, 40),
            solid(40, 200, 90),
            solid(60, 90, 220),
            solid(170, 130, 50),
            solid(120, 40, 180),
            of(&[
                (0, 0, 0),
                (0, 0, 0),
                (0, 0, 0),
                (30, 30, 140),
                (250, 250, 250),
                (250, 250, 250),
                (250, 250, 250),
                (0, 0, 0),
            ]),
        ] {
            let Some(found) = dominant_colour(&img) else {
                continue;
            };
            let accent = ensure_contrast(found);
            let text = text_on(accent);
            assert!(
                contrast_ratio(accent, text) >= MIN_CONTRAST,
                "{found:?} -> {accent:?} with {text:?} is only {:.2}:1",
                contrast_ratio(accent, text)
            );
            // And it is distinguishable from a black and a white background, which
            // is what the luma band is for.
            assert!(
                contrast_ratio(accent, Color::Black) >= MIN_CONTRAST
                    || contrast_ratio(accent, Color::White) >= MIN_CONTRAST,
                "{accent:?} vanishes on one of the two backgrounds"
            );
        }
    }

    #[test]
    fn text_on_picks_whatever_is_legible() {
        // A pale accent takes dark text, a deep one takes light text.
        assert_eq!(text_on(Color::Rgb(230, 220, 200)), Color::Black);
        assert_eq!(text_on(Color::Rgb(20, 30, 60)), Color::White);
        // A terminal colour is not ours to reason about; black is the safe answer.
        assert_eq!(text_on(Color::Red), Color::Black);
    }

    #[test]
    fn the_contrast_helper_lifts_black_and_drops_white() {
        // Pure black is the one thing it cannot lift, and it says so by leaving it
        // alone rather than inventing a colour.
        assert_eq!(ensure_contrast(Color::Rgb(0, 0, 0)), Color::Rgb(0, 0, 0));
        assert!(!is_usable(Color::Rgb(0, 0, 0)));
        for c in [Color::Rgb(2, 2, 6), Color::Rgb(10, 12, 14)] {
            let Color::Rgb(r, g, b) = ensure_contrast(c) else {
                panic!("expected rgb");
            };
            assert!(
                luma(r, g, b) >= MIN_LUMA,
                "{c:?} was left unreadably dark: {:?}",
                Color::Rgb(r, g, b)
            );
        }
        for c in [Color::Rgb(255, 255, 255), Color::Rgb(250, 248, 244)] {
            let Color::Rgb(r, g, b) = ensure_contrast(c) else {
                panic!("expected rgb");
            };
            assert!(
                luma(r, g, b) <= MAX_LUMA,
                "{c:?} was left unreadably light: {:?}",
                Color::Rgb(r, g, b)
            );
        }
    }

    /// Whatever comes out must be readable, on a light terminal and a dark one.
    /// That is the only requirement that matters for a colour scheme.
    #[test]
    fn whatever_comes_out_can_carry_text() {
        for img in [
            solid(200, 40, 40),
            solid(40, 200, 90),
            solid(60, 90, 220),
            solid(230, 200, 60),
            of(&[
                (0, 0, 0),
                (0, 0, 0),
                (0, 0, 0),
                (30, 30, 90),
                (250, 250, 250),
                (250, 250, 250),
                (250, 250, 250),
                (0, 0, 0),
            ]),
        ] {
            if let Some(c) = dominant_colour(&img) {
                let fixed = ensure_contrast(c);
                let Color::Rgb(r, g, b) = fixed else {
                    panic!("rgb")
                };
                let l = luma(r, g, b);
                assert!(
                    (MIN_LUMA..=MAX_LUMA).contains(&l),
                    "{c:?} -> {fixed:?} luma {l}"
                );
            }
        }
    }

    /// The threshold constants are the contract between this module and the
    /// renderer, so pin them: a "tidy up" must not quietly change which covers
    /// get an accent.
    #[test]
    fn the_thresholds_are_where_they_are_documented() {
        assert_eq!((MIN_LUMA, MAX_LUMA), (0.10, 0.60), "the luma band");
        assert_eq!(MIN_SATURATION, 0.20, "grey is not an accent");
        assert_eq!(STRONG_SATURATION, 0.45, "a strong hue wins");
        assert_eq!(MIN_CONTRAST, 3.0, "a UI component, not body text");
        const { assert!(MIN_SATURATION < STRONG_SATURATION) };
        const { assert!(MIN_LUMA > 0.0 && MAX_LUMA < 1.0) };
        const { assert!(MIN_CONTRAST >= 1.0) };
    }

    #[test]
    fn hsl_matches_known_values() {
        let (h, s, l) = hsl(255, 0, 0);
        assert!(h.abs() < 0.01 && s > 0.99 && l > 0.49 && l < 0.51);
        let (h, s, _) = hsl(0, 255, 0);
        assert!((h - 120.0).abs() < 0.01 && s > 0.99);
        let (_, s, l) = hsl(128, 128, 128);
        assert!(s < 0.01 && (l - 0.502).abs() < 0.01);
    }

    #[test]
    fn luma_orders_colours_the_way_eyes_do() {
        assert!(luma(255, 255, 255) > luma(200, 200, 200));
        assert!(luma(200, 200, 200) > luma(20, 20, 20));
        // Green contributes most, so it outranks an equal-looking blue.
        assert!(luma(0, 255, 0) > luma(0, 0, 255));
    }
}

/// Three colours taken from one cover, for gradients (TODO 4.2, "gradients
/// instead of one colour").
///
/// One accent is not enough: a bar drawn in a single colour is a bar, and the
/// whole interface ends up the same hue as one piece of text. A ramp reads as
/// designed rather than configured, and it costs nothing at render time once the
/// colours are in the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The accent: what borders, the title and the bar head use.
    pub primary: Color,
    /// The far end of the ramp.
    pub secondary: Color,
    /// The middle, so a long gradient does not have to go straight from one end
    /// to the other.
    pub tertiary: Color,
}

/// How far apart two hues have to be before a second colour is worth using.
/// 24° is about where two hues stop reading as "the same colour, lighter".
const HUE_SEPARATION: f32 = 24.0;

/// How far a synthesised hue is rotated when the cover only offers one. Analogous
/// rather than complementary: a cover is one photo, so its colours are usually
/// neighbours, and a complementary jump looks like a different album.
const SYNTHETIC_ROTATION: f32 = 38.0;

impl Palette {
    /// A palette with no art behind it, so `accent = "green"` still gets a
    /// gradient. The Spotify green plus two rotations of it, which is an
    /// analogous ramp through teal to blue.
    pub fn from_accent(primary: Color) -> Palette {
        match primary {
            Color::Rgb(r, g, b) => Palette {
                primary,
                secondary: rotate(r, g, b, SYNTHETIC_ROTATION),
                tertiary: rotate(r, g, b, -SYNTHETIC_ROTATION),
            },
            // A terminal palette has no colours to rotate; one flat colour is the
            // honest answer, and `ramp` then returns it unchanged.
            other => Palette {
                primary: other,
                secondary: other,
                tertiary: other,
            },
        }
    }

    /// `len` colours from `primary` through `tertiary` to `secondary`.
    ///
    /// Interpolated in HSL rather than RGB: halfway between a pink and a blue in
    /// RGB is a muddy grey, and halfway between them in hue is the colour you
    /// would have picked.
    pub fn ramp(&self, len: usize) -> Vec<Color> {
        if len == 0 {
            return Vec::new();
        }
        if self.primary == self.secondary && self.secondary == self.tertiary {
            return vec![self.primary; len];
        }
        (0..len)
            .map(|i| {
                let t = if len == 1 {
                    0.0
                } else {
                    i as f32 / (len - 1) as f32
                };
                // Two halves: primary -> tertiary, then tertiary -> secondary.
                if t <= 0.5 {
                    mix(self.primary, self.tertiary, t * 2.0)
                } else {
                    mix(self.tertiary, self.secondary, (t - 0.5) * 2.0)
                }
            })
            .collect()
    }

    /// The colour at the far end of the ramp, for a single span that has to be
    /// one colour.
    pub fn end(&self) -> Color {
        self.secondary
    }
}

/// Interpolate two colours in HSL, taking the short way round the hue circle.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else {
        return a;
    };
    let t = t.clamp(0.0, 1.0);
    let (h1, s1, l1) = hsl(r1, g1, b1);
    let (h2, s2, l2) = hsl(r2, g2, b2);
    // The short way round, so a ramp never sweeps through red because the two
    // ends happen to sit either side of 0°.
    let mut delta = (h2 - h1) % 360.0;
    if delta > 180.0 {
        delta -= 360.0;
    }
    if delta < -180.0 {
        delta += 360.0;
    }
    let h = (h1 + delta * t).rem_euclid(360.0);
    let s = s1 + (s2 - s1) * t;
    let l = l1 + (l2 - l1) * t;
    from_hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0))
}

/// Rotate a colour's hue by `degrees`, keeping its saturation and lightness.
fn rotate(r: u8, g: u8, b: u8, degrees: f32) -> Color {
    let (h, s, l) = hsl(r, g, b);
    from_hsl((h + degrees).rem_euclid(360.0), s, l)
}

/// HSL back to RGB. Written out rather than pulled from a crate: `image` has no
/// HSL conversion and this is twenty lines.
fn from_hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h6 = h / 60.0;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::Rgb(
        (((r + m) * 255.0).round().clamp(0.0, 255.0)) as u8,
        (((g + m) * 255.0).round().clamp(0.0, 255.0)) as u8,
        (((b + m) * 255.0).round().clamp(0.0, 255.0)) as u8,
    )
}

/// The ramp for a cover: up to three genuinely different hues, and synthesised
/// rotations when the cover only has one.
///
/// Every colour goes through `is_usable`, so a palette can never contain a
/// colour the interface should not draw.
pub fn palette(image: &image::DynamicImage) -> Palette {
    let bins = hue_bins(image);
    let primary = bins
        .first()
        .map(|bin| bin.1)
        .unwrap_or(SPOTIFY_GREEN_LITERAL);
    Palette {
        primary,
        secondary: pick_separate(&bins, primary, 1)
            .unwrap_or_else(|| rotate_rgb(primary, SYNTHETIC_ROTATION)),
        tertiary: pick_separate(&bins, primary, 2)
            .unwrap_or_else(|| rotate_rgb(primary, -SYNTHETIC_ROTATION)),
    }
}

/// A hue, the colour that won it, and how strongly it was represented.
type Bin = (f32, Color, f32);

/// Bucket the usable pixels of a cover by hue, keeping the best colour in each.
fn hue_bins(image: &image::DynamicImage) -> Vec<Bin> {
    let small = image::imageops::resize(&image.to_rgba8(), SAMPLE, SAMPLE, FilterType::Triangle);
    let mut bins: Vec<Bin> = Vec::new();
    for &[r, g, b, a] in small.as_raw().as_chunks::<4>().0 {
        if a < 128 {
            continue;
        }
        let (h, s, _l) = hsl(r, g, b);
        let y = luma(r, g, b);
        if s < MIN_SATURATION || y < MIN_LUMA || y > MAX_LUMA {
            continue;
        }
        let weight = s * y;
        let colour = Color::Rgb(r, g, b);
        match bins.iter_mut().find(|bin| {
            let d = (bin.0 - h).abs();
            d.min(360.0 - d) < HUE_SEPARATION
        }) {
            Some(bin) if weight > bin.2 => *bin = (bin.0, colour, weight),
            Some(_) => {}
            None => bins.push((h, colour, weight)),
        }
    }
    bins.sort_by(|a, b| b.2.total_cmp(&a.2));
    bins
}

/// The best colour at least `want` bins away from `primary`, so the ramp has
/// somewhere to go.
fn pick_separate(bins: &[Bin], primary: Color, want: u32) -> Option<Color> {
    let Color::Rgb(pr, pg, pb) = primary else {
        return None;
    };
    let (ph, _, _) = hsl(pr, pg, pb);
    bins.iter()
        .filter(|bin| {
            let d = (bin.0 - ph).abs();
            d.min(360.0 - d) >= HUE_SEPARATION * want as f32
        })
        .map(|bin| bin.1)
        .next()
}

fn rotate_rgb(colour: Color, degrees: f32) -> Color {
    match colour {
        Color::Rgb(r, g, b) => rotate(r, g, b, degrees),
        other => other,
    }
}

/// The Spotify green as a literal, so this module does not depend on the theme's
/// copy of it.
const SPOTIFY_GREEN_LITERAL: Color = Color::Rgb(0x1D, 0xB9, 0x54);
