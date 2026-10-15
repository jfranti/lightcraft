//! Seeded film looks: a number becomes a film-like 3D LUT (a look LUT, display sRGB in and out, the
//! space LightCraft applies `.cube` profiles in, see [`crate::lut`]).
//!
//! A seed picks an [`Archetype`] (motion-picture print, slide, consumer negative, faded print,
//! cross-process, bleach bypass) and draws the parameters of a small film model within that
//! archetype's ranges ([`FilmParams::from_seed`]); [`FilmParams::lut`] bakes the model into a LUT.
//! The same seed always gives the same look, so a look can be shared as its number.
//!
//! The model, in the order applied:
//! 1. per-channel tone in the display-encoded domain: an S-curve of seeded contrast about middle
//!    grey, different by a little per channel (film's colour crossover), then a lifted black (fade)
//!    and a lowered white (the print's maximum);
//! 2. colour in OkLab: saturation that depends on lightness (film desaturates its deep shadows and
//!    its highlights differently), soft compression of very saturated colours (as a dye set does),
//!    hue shifts in two or three seeded hue bands (greens towards teal or olive, reds towards orange…),
//!    and seeded shadow and highlight tints.
//!
//! Guardrails keep every seed usable: the tone curves are monotone, a neutral grey drifts by at most
//! [`MAX_GREY_DRIFT`] in OkLab chroma, hue shifts near skin tones are damped, and saturation is
//! bounded.

use serde::{Deserialize, Serialize};

use crate::lut::Lut3d;

/// The family a seed's look is drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Archetype {
    /// Camera negative printed to motion-picture print stock: dense blacks, rich mids, soft
    /// shoulder, teal-leaning shadows and warm highlights.
    Print,
    /// Colour reversal (slide) film: high contrast, high saturation, deep blues and greens.
    Slide,
    /// Consumer colour negative, scanned: gentle contrast, a little fade, warm or green casts.
    Negative,
    /// An aged print: low contrast, lifted milky blacks, faded and shifted colour.
    Faded,
    /// Cross-processed film: strong crossover between the channels, punchy colour.
    Cross,
    /// Bleach bypass (skip-bleach): high contrast, low saturation, cool shadows.
    Bleach,
}

impl Archetype {
    pub fn name(self) -> &'static str {
        match self {
            Archetype::Print => "Print",
            Archetype::Slide => "Slide",
            Archetype::Negative => "Negative",
            Archetype::Faded => "Faded",
            Archetype::Cross => "Cross",
            Archetype::Bleach => "Bleach",
        }
    }
}

/// The parameters of a seeded look (see the module docs for what each does).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmParams {
    pub seed: u64,
    pub archetype: Archetype,
    /// S-curve strength about middle grey, per channel (R, G, B); 0 = none.
    pub contrast: [f32; 3],
    /// Per-channel offset of the curve's pivot (display units, ±): crossover.
    pub pivot: [f32; 3],
    /// Black lift (fade) and white level of the print, per channel.
    pub black: [f32; 3],
    pub white: [f32; 3],
    /// Overall saturation, and how much of it the deep shadows and the highlights keep (1 = all).
    pub saturation: f32,
    pub shadow_sat: f32,
    pub highlight_sat: f32,
    /// OkLab chroma above which colours are compressed (lower = stronger dye-set compression).
    pub chroma_knee: f32,
    /// Hue shifts: (band centre, width, shift), in degrees of OkLCh hue.
    pub hue_shifts: Vec<[f32; 3]>,
    /// Shadow and highlight tints, OkLab (a, b).
    pub shadow_tint: [f32; 2],
    pub highlight_tint: [f32; 2],
}

/// Largest OkLab chroma a neutral grey may take on through a seeded look.
pub const MAX_GREY_DRIFT: f32 = 0.03;
/// OkLCh hue (degrees) of typical skin, and the half width around it where hue shifts are damped.
const SKIN_HUE: f32 = 55.0;
const SKIN_HALF_WIDTH: f32 = 30.0;

/// SplitMix64: a small, well-mixed deterministic generator (the seed is the whole state).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next() >> 40) as f32 / (1u64 << 24) as f32)
    }
    fn chance(&mut self, p: f32) -> bool {
        self.range(0.0, 1.0) < p
    }
    /// A small (a, b) tint of chroma `lo..hi` in a hue drawn from `hues` (degrees ± 20).
    fn tint(&mut self, hues: &[f32], lo: f32, hi: f32) -> [f32; 2] {
        let h = hues[(self.next() % hues.len() as u64) as usize] + self.range(-20.0, 20.0);
        let c = self.range(lo, hi);
        [c * h.to_radians().cos(), c * h.to_radians().sin()]
    }
}

impl FilmParams {
    /// The look of `seed`.
    pub fn from_seed(seed: u64) -> FilmParams {
        let mut r = Rng(seed ^ 0x51F1_5EED_F11E_u64);
        let archetype = match r.range(0.0, 1.0) {
            x if x < 0.30 => Archetype::Print,
            x if x < 0.48 => Archetype::Slide,
            x if x < 0.70 => Archetype::Negative,
            x if x < 0.82 => Archetype::Faded,
            x if x < 0.92 => Archetype::Cross,
            _ => Archetype::Bleach,
        };
        // (contrast, crossover spread, fade, white, saturation, shadow sat, highlight sat, knee)
        let (c, x, fade, white, sat, ssat, hsat, knee) = match archetype {
            Archetype::Print => ((0.25, 0.45), 0.012, (0.0, 0.015), (0.94, 0.98), (0.95, 1.1), (0.7, 0.9), (0.55, 0.8), (0.14, 0.2)),
            Archetype::Slide => ((0.4, 0.65), 0.01, (0.0, 0.006), (0.98, 1.0), (1.12, 1.32), (0.85, 1.0), (0.75, 0.95), (0.18, 0.26)),
            Archetype::Negative => ((0.0, 0.2), 0.015, (0.015, 0.05), (0.93, 0.97), (0.88, 1.05), (0.75, 0.95), (0.6, 0.85), (0.15, 0.22)),
            Archetype::Faded => ((-0.12, 0.08), 0.02, (0.05, 0.09), (0.88, 0.94), (0.6, 0.85), (0.6, 0.85), (0.5, 0.8), (0.1, 0.16)),
            Archetype::Cross => ((0.35, 0.6), 0.045, (0.0, 0.03), (0.94, 1.0), (1.1, 1.3), (0.85, 1.0), (0.7, 0.9), (0.17, 0.25)),
            Archetype::Bleach => ((0.5, 0.8), 0.01, (0.0, 0.02), (0.95, 1.0), (0.4, 0.65), (0.6, 0.85), (0.5, 0.75), (0.08, 0.13)),
        };
        let base_c = r.range(c.0, c.1);
        let contrast = std::array::from_fn(|_| base_c + r.range(-0.06, 0.06) * (x / 0.01f32).min(2.0));
        let pivot = std::array::from_fn(|_| r.range(-x, x));
        let base_fade = r.range(fade.0, fade.1);
        let black = std::array::from_fn(|_| (base_fade + r.range(-0.006, 0.006)).max(0.0));
        let base_white = r.range(white.0, white.1);
        let white = std::array::from_fn(|_| (base_white + r.range(-0.012, 0.012)).min(1.0));
        let (saturation, shadow_sat, highlight_sat, chroma_knee) =
            (r.range(sat.0, sat.1), r.range(ssat.0, ssat.1), r.range(hsat.0, hsat.1), r.range(knee.0, knee.1));
        // hue shifts: greens to teal or olive, reds/oranges, blues to cyan or violet — 2 or 3 of them
        let bands: [(f32, f32, f32); 5] =
            [(140.0, -18.0, 18.0), (35.0, -8.0, 10.0), (250.0, -15.0, 15.0), (100.0, -15.0, 15.0), (200.0, -12.0, 12.0)];
        let n = 2 + r.chance(0.5) as usize;
        let mut hue_shifts = Vec::new();
        let mut used = [false; 5];
        while hue_shifts.len() < n {
            let i = (r.next() % bands.len() as u64) as usize;
            if used[i] {
                continue;
            }
            used[i] = true;
            let (centre, lo, hi) = bands[i];
            let scale = if archetype == Archetype::Cross { 1.6 } else { 1.0 };
            hue_shifts.push([centre, r.range(35.0, 60.0), r.range(lo, hi) * scale]);
        }
        let (shadow_tint, highlight_tint) = match archetype {
            Archetype::Print => (r.tint(&[200.0, 220.0], 0.008, 0.02), r.tint(&[60.0, 75.0], 0.006, 0.016)),
            Archetype::Slide => (r.tint(&[240.0, 280.0], 0.0, 0.012), r.tint(&[80.0, 100.0], 0.0, 0.01)),
            Archetype::Negative => (r.tint(&[150.0, 200.0, 300.0], 0.004, 0.014), r.tint(&[60.0, 90.0, 120.0], 0.006, 0.016)),
            Archetype::Faded => (r.tint(&[330.0, 200.0, 20.0], 0.01, 0.025), r.tint(&[60.0, 30.0], 0.008, 0.02)),
            Archetype::Cross => (r.tint(&[230.0, 160.0], 0.01, 0.025), r.tint(&[100.0, 80.0], 0.01, 0.025)),
            Archetype::Bleach => (r.tint(&[220.0, 240.0], 0.006, 0.016), r.tint(&[70.0], 0.0, 0.008)),
        };
        FilmParams {
            seed,
            archetype,
            contrast,
            pivot,
            black,
            white,
            saturation,
            shadow_sat,
            highlight_sat,
            chroma_knee,
            hue_shifts,
            shadow_tint,
            highlight_tint,
        }
    }

    /// A short name for the look: `Film 48213 · Print`.
    pub fn name(&self) -> String {
        format!("Film {} · {}", self.seed, self.archetype.name())
    }

    /// One display-encoded sRGB colour through the look.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        // 1. tone, per channel, in the encoded domain (pivot at middle grey's code value)
        let m = srgb_encode(0.18);
        let toned: [f32; 3] = std::array::from_fn(|i| {
            let p = rgb[i].clamp(0.0, 1.0);
            let s = s_curve(p, m + self.pivot[i], self.contrast[i]);
            self.black[i] + (self.white[i] - self.black[i]) * s
        });
        // 2. colour, in OkLab
        let [l, a, b] = oklab(toned.map(srgb_decode));
        let (mut a, mut b) = (a, b);
        let chroma = (a * a + b * b).sqrt();
        if chroma > 1e-6 {
            let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
            // saturation by lightness: full in the midtones, less in deep shadows and highlights
            let by_l = lerp(self.shadow_sat, 1.0, smooth(0.05, 0.45, l)) * lerp(1.0, self.highlight_sat, smooth(0.65, 0.98, l));
            let mut c = chroma * self.saturation * by_l;
            // dye-set compression of very saturated colours (soft knee)
            if c > self.chroma_knee {
                let over = c - self.chroma_knee;
                c = self.chroma_knee + over / (1.0 + over / (0.5 * self.chroma_knee));
            }
            // hue shifts, damped near skin
            let skin = 1.0 - 0.75 * (1.0 - smooth(0.0, SKIN_HALF_WIDTH, hue_dist(hue, SKIN_HUE)));
            let shift: f32 =
                self.hue_shifts.iter().map(|[centre, width, s]| s * (-(hue_dist(hue, *centre) / width).powi(2)).exp()).sum::<f32>() * skin;
            let h = (hue + shift).to_radians();
            (a, b) = (c * h.cos(), c * h.sin());
        }
        // tints: shadows and highlights, fading out through the midtones
        let (ws, wh) = (1.0 - smooth(0.1, 0.55, l), smooth(0.55, 0.95, l));
        a += self.shadow_tint[0] * ws + self.highlight_tint[0] * wh;
        b += self.shadow_tint[1] * ws + self.highlight_tint[1] * wh;
        oklab_inv([l, a, b]).map(|v| srgb_encode(v.clamp(0.0, 1.0)))
    }

    /// The look baked into a `size`³ LUT, with the grey guardrail applied: if any neutral input comes
    /// out with more than [`MAX_GREY_DRIFT`] chroma, the tints and crossover are scaled back until none does.
    pub fn lut(&self, size: usize) -> Lut3d {
        let mut p = self.clone();
        for _ in 0..8 {
            let drift = (0..=16).map(|i| p.apply([i as f32 / 16.0; 3])).map(grey_chroma).fold(0.0f32, f32::max);
            if drift <= MAX_GREY_DRIFT {
                break;
            }
            p.shadow_tint = p.shadow_tint.map(|v| v * 0.7);
            p.highlight_tint = p.highlight_tint.map(|v| v * 0.7);
            p.pivot = p.pivot.map(|v| v * 0.7);
            let mean_black = p.black.iter().sum::<f32>() / 3.0;
            let mean_white = p.white.iter().sum::<f32>() / 3.0;
            p.black = p.black.map(|v| mean_black + (v - mean_black) * 0.7);
            p.white = p.white.map(|v| mean_white + (v - mean_white) * 0.7);
        }
        let n = size.max(2);
        let mut data = Vec::with_capacity(n * n * n);
        // .cube order: red fastest
        for bi in 0..n {
            for gi in 0..n {
                for ri in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    data.push(p.apply([f(ri), f(gi), f(bi)]));
                }
            }
        }
        Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title: Some(self.name()) }
    }
}

/// An S-curve through (0,0), (`pivot`, `pivot`) and (1,1) of strength `k` (0 = identity; negative
/// flattens); monotone for `k` in −0.5…1.
fn s_curve(p: f32, pivot: f32, k: f32) -> f32 {
    let pivot = pivot.clamp(0.2, 0.8);
    // a smoothstep-shaped S on each side of the pivot, blended in by `k`
    let s = if p < pivot {
        let t = p / pivot;
        pivot * (t * t * (3.0 - 2.0 * t))
    } else {
        let t = (p - pivot) / (1.0 - pivot);
        pivot + (1.0 - pivot) * (t * t * (3.0 - 2.0 * t))
    };
    let k = k.clamp(-0.5, 1.0);
    p + k * (s - p)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn grey_chroma(c: [f32; 3]) -> f32 {
    let [_, a, b] = oklab(c.map(srgb_decode));
    (a * a + b * b).sqrt()
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Linear sRGB → OkLab (Björn Ottosson's published matrices).
fn oklab(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = c;
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).max(0.0).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).max(0.0).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).max(0.0).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OkLab → linear sRGB.
fn oklab_inv(c: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = c;
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_always_gives_the_same_look_and_seeds_differ() {
        assert_eq!(FilmParams::from_seed(48213), FilmParams::from_seed(48213));
        assert_eq!(FilmParams::from_seed(7).lut(9), FilmParams::from_seed(7).lut(9));
        assert_ne!(FilmParams::from_seed(1), FilmParams::from_seed(2));
        // every archetype turns up across seeds
        let kinds: std::collections::HashSet<_> = (0..400).map(|s| FilmParams::from_seed(s).archetype).collect();
        assert_eq!(kinds.len(), 6, "{kinds:?}");
    }

    #[test]
    fn every_seed_stays_usable() {
        for seed in 0..300u64 {
            let lut = FilmParams::from_seed(seed).lut(17);
            let at = |r: usize, g: usize, b: usize| lut.data[(b * 17 + g) * 17 + r];
            // greys stay near neutral and in order (darker in, darker out)
            let mut prev = -1.0f32;
            for i in 0..17 {
                let c = at(i, i, i);
                assert!(grey_chroma(c) <= MAX_GREY_DRIFT + 0.01, "seed {seed}: grey {i} drifts to {c:?}");
                let y = 0.2126 * srgb_decode(c[0]) + 0.7152 * srgb_decode(c[1]) + 0.0722 * srgb_decode(c[2]);
                assert!(y >= prev - 1e-4, "seed {seed}: greys not monotone at {i}");
                prev = y;
            }
            // nothing leaves the display range, nothing is NaN
            assert!(lut.data.iter().flatten().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "seed {seed}");
            // black stays dark and white stays bright
            assert!(at(0, 0, 0).iter().all(|v| *v < 0.35) && at(16, 16, 16).iter().all(|v| *v > 0.8), "seed {seed}");
        }
    }

    #[test]
    fn skin_hues_move_less_than_greens() {
        let p = FilmParams { hue_shifts: vec![[55.0, 40.0, 15.0], [140.0, 40.0, 15.0]], ..FilmParams::from_seed(3) };
        let shift_at = |hue: f32| {
            let c = 0.08;
            let lab = [0.6, c * hue.to_radians().cos(), c * hue.to_radians().sin()];
            let out = p.apply(oklab_inv(lab).map(|v| srgb_encode(v.clamp(0.0, 1.0))));
            let o = oklab(out.map(srgb_decode));
            hue_dist(o[2].atan2(o[1]).to_degrees().rem_euclid(360.0), hue)
        };
        assert!(shift_at(55.0) < shift_at(140.0) * 0.6, "{} {}", shift_at(55.0), shift_at(140.0));
    }
}
