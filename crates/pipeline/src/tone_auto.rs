//! Auto tone: Exposure, Contrast, Highlights, Shadows, Whites, Blacks and Vibrance measured on the
//! photo itself.
//!
//! A small proxy is rendered through the real pipeline (profile, white balance, curves and all) and
//! every slider is solved against targets in the rendered result (CIE L*, 0 black … 100 white). The
//! method follows what robust auto-exposure systems have in common (notes in the repository's
//! research log): the statistics are **weighted towards the subject** — detailed areas count more
//! than flat ones (as in the MIT-Adobe FiveK predictors), the top of the frame counts less and a
//! flat sky that touches the top edge counts little (as camera metering does), the centre counts
//! more — so a large overcast sky doesn't capture the median; the midtone target follows the
//! scene's **key** (Reinhard's automatic key estimate, damped), so snow stays bright and night
//! stays dark instead of everything being pulled to middle grey; and Exposure **blends** the
//! midtone solution with a highlight anchor (the brightest non-specular tones, or the sky, near
//! white; RawTherapee's Auto Levels does the same) and a visible-detail optimum (Mertens'
//! well-exposedness). Then each slider is solved in turn (top-down, as Adobe recommends), only in
//! the direction it is needed and within a cap, so the correction is shared rather than one slider
//! slammed to its end. A [`Style`] picks the targets.

use lightcraft_color::luminance_2020;
use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Rgb32f, Rgba8};
use serde::{Deserialize, Serialize};

use crate::SourceInfo;
use crate::auto::AutoTone;

/// The long edge, in pixels, of the renders Auto measures.
pub(crate) const MEASURE_EDGE: usize = 256;
/// Bisection steps per slider (after checking both ends of its range).
const SOLVE_STEPS: usize = 7;
/// Golden-section steps per slider in [`settle`].
const SETTLE_STEPS: usize = 10;

/// A family of Auto targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Style {
    /// Balanced, true to the scene's key.
    #[default]
    Natural,
    /// Brighter midtones, open shadows, softer contrast.
    Bright,
    /// More contrast and colour, deeper blacks.
    Punchy,
    /// Holds the highlights back and opens the shadows (an HDR-like rendering).
    Highlights,
    /// Low contrast with lifted, faded blacks and a soft shoulder.
    Film,
}

impl Style {
    pub const ALL: [Style; 5] = [Style::Natural, Style::Bright, Style::Punchy, Style::Highlights, Style::Film];

    pub fn name(self) -> &'static str {
        match self {
            Style::Natural => "natural",
            Style::Bright => "bright",
            Style::Punchy => "punchy",
            Style::Highlights => "highlights",
            Style::Film => "film",
        }
    }

    pub fn from_name(s: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|v| v.name().eq_ignore_ascii_case(s.trim()))
    }

    fn targets(self) -> Targets {
        let natural = Targets {
            mid_shift: 0.0,
            top: 90.0,
            sky: 87.0,
            shadow: 18.0,
            shadows_cap: 60.0,
            white: 95.0,
            black: 4.0,
            lift_blacks: false,
            spread: 30.0,
            contrast_cap: 35.0,
            vibrance: 10.0,
            saturation: 0.0,
        };
        match self {
            Style::Natural => natural,
            Style::Bright => Targets {
                mid_shift: 9.0,
                top: 93.0,
                sky: 90.0,
                shadow: 24.0,
                white: 97.0,
                black: 8.0,
                lift_blacks: true,
                spread: 24.0,
                vibrance: 5.0,
                saturation: -5.0,
                ..natural
            },
            Style::Punchy => {
                Targets { shadow: 12.0, white: 97.0, black: 2.0, spread: 36.0, contrast_cap: 50.0, vibrance: 20.0, saturation: 5.0, ..natural }
            }
            Style::Highlights => Targets {
                mid_shift: -2.0,
                top: 85.0,
                sky: 82.0,
                shadow: 22.0,
                shadows_cap: 75.0,
                white: 92.0,
                black: 5.0,
                spread: 26.0,
                vibrance: 15.0,
                ..natural
            },
            Style::Film => Targets {
                top: 88.0,
                sky: 85.0,
                shadow: 18.0,
                white: 92.0,
                black: 9.0,
                lift_blacks: true,
                spread: 22.0,
                contrast_cap: 20.0,
                vibrance: 5.0,
                saturation: -10.0,
                ..natural
            },
        }
    }
}

/// What a [`Style`] aims at, in L* of the rendered result.
#[derive(Clone, Copy, Debug)]
struct Targets {
    /// Added to the key-derived midtone target.
    mid_shift: f64,
    /// The brightest non-specular tones (unweighted 99th percentile) at most here.
    top: f64,
    /// A detected sky's median at most here.
    sky: f64,
    /// Subject-weighted 10th percentile at least here (Shadows lifts).
    shadow: f64,
    shadows_cap: f64,
    /// 99.5th percentile (Whites).
    white: f64,
    /// 0.5th percentile (Blacks deepens down to it, or with `lift_blacks` also lifts up to it).
    black: f64,
    lift_blacks: bool,
    /// Subject-weighted interquartile range at least this (Contrast adds).
    spread: f64,
    contrast_cap: f64,
    vibrance: f64,
    saturation: f64,
}

/// What Auto knows about the capture beyond the pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capture {
    /// The camera's exposure value at ISO 100, `log2(N² / t) − log2(ISO / 100)`, from EXIF.
    pub ev100: Option<f64>,
    /// Faces in the rendered frame (crop applied), `[x0, y0, x1, y1]` in fractions of its width and
    /// height: the subject, when there are any (a face detector's boxes).
    pub faces: Vec<[f32; 4]>,
}

impl Capture {
    /// From EXIF aperture (f-number), exposure time (s) and ISO.
    pub fn from_exif(f_number: Option<f64>, exposure_time: Option<f64>, iso: Option<f64>) -> Capture {
        let ev100 = match (f_number, exposure_time, iso) {
            (Some(n), Some(t), Some(iso)) if n > 0.0 && t > 0.0 && iso > 0.0 => Some((n * n / t).log2() - (iso / 100.0).log2()),
            _ => None,
        };
        Capture { ev100, faces: Vec::new() }
    }
}

/// Weight of face pixels relative to the rest of the subject (CAPE and FiveK both weigh faces most).
const FACE_WEIGHT: f32 = 3.0;
/// Where the brighter quarter of a face's tones (its 75th percentile) is put: a little under light skin's
/// Zone VI (L* ≈ 66), since CAPE found moving a face about half way better than all the way (a face lit
/// darker than the scene keeps some of that light's character).
const FACE_TARGET: f64 = 60.0;

/// Scene EV100 below which a scene starts to be taken as genuinely dark (dim interior, dusk), and
/// at or below which it fully is (night): Auto then keeps its mood instead of lifting it to day.
const DARK_SCENE_EV: f64 = 7.0;
const NIGHT_SCENE_EV: f64 = 3.0;
/// How many stops the midtone target moves for a fully high- or low-key scene (Reinhard's automatic
/// key moves it by 2; damped, since the sliders then have less to fix).
const KEY_STOPS: f64 = 1.6;
/// Exposure goes only as far as the brightest tones, held back by this much Highlights, stay within
/// `TOP_SLACK` of their target (Highlights then does the rest), and a backlit subject is left at
/// most `BACKLIT_SHORT` EV short of its own exposure.
const HELD_BY_HIGHLIGHTS: f64 = -60.0;
const TOP_SLACK: f64 = 4.0;
const BACKLIT_SHORT: f64 = 1.0;
/// How far (L*) the subject's midtones may sit from the target before Exposure fully corrects them.
const COMFORT_L: f64 = 12.0;
/// Within this (L*) of the target the midtones count as well exposed: Exposure leaves them be.
const COMFORT_IN: f64 = 5.0;

/// `src` fitted into `edge × edge`, each proxy pixel the mean of the valid (finite) source pixels
/// under it; proxy pixels with none get the mean of the valid ones. `None` when no pixel is valid.
pub(crate) fn valid_proxy(src: &Rgb32f, edge: usize) -> Option<Rgb32f> {
    let (mut clean, mut weight) = (src.clone(), Rgb32f::new(src.width, src.height));
    let mut any = false;
    for (p, w) in clean.data.iter_mut().zip(weight.data.iter_mut()) {
        if p.iter().all(|v| v.is_finite()) {
            *w = [1.0; 3];
            any = true;
        } else {
            *p = [0.0; 3];
        }
    }
    if !any {
        return None;
    }
    let fit = |img: &Rgb32f| lightcraft_raster::resample::fit(img, edge, edge, lightcraft_raster::resample::Filter::Box);
    let (mut img, weight) = (fit(&clean), fit(&weight));
    let (mut sum, mut n) = ([0f64; 3], 0usize);
    for (p, w) in img.data.iter_mut().zip(&weight.data) {
        if w[0] > 1e-6 {
            *p = p.map(|v| v / w[0]);
            if p.iter().all(|v| v.is_finite()) {
                (0..3).for_each(|i| sum[i] += p[i] as f64);
                n += 1;
                continue;
            }
        }
        *p = [f32::NAN; 3];
    }
    if n == 0 {
        return None;
    }
    let mean = sum.map(|v| (v / n as f64) as f32);
    img.data.iter_mut().filter(|p| p[0].is_nan()).for_each(|p| *p = mean);
    Some(img)
}

/// The scene's log-average luminance under the current white balance, in EV relative to middle grey
/// (the mean of log2(Y / 0.18) over a 512 px proxy, scene-linear, before any tone curve). Pixels that
/// aren't finite (a damaged decode) are left out. `None` when no pixel is valid.
pub fn scene_log_mean(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Option<f64> {
    let mut kept = valid_proxy(src, 512)?;
    let base = DevelopSettings { wb: s.wb, process: s.process, ..DevelopSettings::default() };
    // (white balance is per pixel: the valid pixels can be balanced in a row of their own)
    kept.width = kept.data.len();
    kept.height = 1;
    crate::local::scene_linear_pre(&mut kept, info, &base);
    let (mut sum, mut n) = (0f64, 0usize);
    for c in &kept.data {
        let ev = (luminance_2020(*c).max(1e-6) / 0.18).log2();
        if ev.is_finite() {
            sum += ev as f64;
            n += 1;
        }
    }
    (n > 0).then(|| sum / n as f64)
}

pub(crate) fn apply(d: &mut DevelopSettings, t: &AutoTone) {
    d.light.exposure = t.exposure;
    d.light.contrast = t.contrast;
    d.light.highlights = t.highlights;
    d.light.shadows = t.shadows;
    d.light.whites = t.whites;
    d.light.blacks = t.blacks;
    d.color.vibrance = t.vibrance;
    d.color.saturation = t.saturation;
}

/// CIE L* of an 8-bit sRGB pixel.
pub(crate) fn lstar(p: [u8; 4]) -> f32 {
    static LIN: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    let lin = LIN.get_or_init(|| std::array::from_fn(|i| lightcraft_color::transfer::decode_srgb8(i as u8)));
    let y = 0.2126 * lin[p[0] as usize] + 0.7152 * lin[p[1] as usize] + 0.0722 * lin[p[2] as usize];
    if y > 216.0 / 24389.0 { 116.0 * y.cbrt() - 16.0 } else { y * 24389.0 / 27.0 }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the subject is, from the unedited render: a weight per pixel and the sky mask.
pub(crate) struct Layout {
    pub w: Vec<f32>,
    pub sky: Vec<bool>,
    /// Inside a detected face.
    pub face: Vec<bool>,
    /// Detail weight alone (for the visible-detail exposure).
    pub tex: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl Layout {
    pub(crate) fn of(img: &Rgba8, faces: &[[f32; 4]]) -> Layout {
        let (w, h) = (img.width, img.height);
        let n = w * h;
        let l: Vec<f32> = img.data.iter().map(|p| lstar(*p)).collect();
        let valid: Vec<bool> = img.data.iter().map(|p| p[3] != 0).collect();
        // texture: gradient magnitude of L*, blurred, relative to its median
        let at = |x: usize, y: usize| l[y * w + x];
        let mut g = vec![0f32; n];
        for y in 0..h {
            for x in 0..w {
                let dx = at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y);
                let dy = at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1));
                g[y * w + x] = (dx * dx + dy * dy).sqrt();
            }
        }
        let g = box_blur(&box_blur(&g, w, h, 2), w, h, 2);
        let mut sorted: Vec<f32> = g.iter().zip(&valid).filter(|(_, v)| **v).map(|(g, _)| *g).collect();
        sorted.sort_by(f32::total_cmp);
        let med = sorted.get(sorted.len() / 2).copied().unwrap_or(1.0).max(0.5);
        let t: Vec<f32> = g.iter().map(|v| v / med).collect();

        // sky: bright, flat, neutral or blue, in the top 60 %, connected to the top edge
        let mut ls: Vec<f32> = l.iter().zip(&valid).filter(|(_, v)| **v).map(|(l, _)| *l).collect();
        ls.sort_by(f32::total_cmp);
        let p60 = ls.get(ls.len() * 6 / 10).copied().unwrap_or(100.0);
        let candidate = |i: usize| {
            let p = img.data[i];
            let (r, g_, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let chroma = (r.max(g_).max(b) - r.min(g_).min(b)) / 255.0;
            let blue = b > r && b >= g_ * 0.95;
            valid[i] && l[i] > p60.max(40.0) && t[i] < 0.6 && (chroma < 0.1 || blue) && (i / w) < h * 6 / 10
        };
        let mut sky = vec![false; n];
        let mut stack: Vec<usize> = (0..w).filter(|&x| candidate(x)).collect();
        stack.iter().for_each(|&i| sky[i] = true);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if !sky[j] && candidate(j) {
                    sky[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        let frac = sky.iter().filter(|s| **s).count() as f32 / n.max(1) as f32;
        if !(0.03..=0.7).contains(&frac) {
            sky.iter_mut().for_each(|s| *s = false);
        }

        let diag = ((w * w + h * h) as f32).sqrt();
        let mut wt = vec![0f32; n];
        let mut tex = vec![0f32; n];
        let mut face = vec![false; n];
        for i in 0..n {
            if !valid[i] {
                continue;
            }
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let pos = 0.5 + 0.5 * smoothstep(0.0, 0.35, y / h as f32);
            let te = t[i].clamp(0.25, 3.0).sqrt();
            let in_face = faces.iter().any(|f| {
                let (fw, fh) = (f[2] - f[0], f[3] - f[1]);
                let (u, v) = (x / w as f32, y / h as f32);
                u >= f[0] - 0.1 * fw && u <= f[2] + 0.1 * fw && v >= f[1] - 0.1 * fh && v <= f[3] + 0.1 * fh
            });
            face[i] = in_face;
            let sk = if sky[i] && !in_face { 0.15 } else { 1.0 };
            let (dx, dy) = (x - w as f32 / 2.0, y - h as f32 / 2.0);
            let r = (dx * dx + dy * dy).sqrt() / (0.4 * diag);
            let centre = (-0.5 * r * r).exp().max(0.5);
            wt[i] = pos * te * sk * centre * if in_face { FACE_WEIGHT } else { 1.0 };
            tex[i] = te * sk;
        }
        Layout { w: wt, sky, face, tex, width: w, height: h }
    }
}

fn box_blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; v.len()];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = (a..=b).map(|i| v[y * w + i]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    let mut out = vec![0f32; v.len()];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (a..=b).map(|j| tmp[j * w + x]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    out
}

/// One render, measured against a [`Layout`].
pub(crate) struct Measured {
    /// (L*, weight), ascending L*.
    weighted: Vec<(f32, f32)>,
    total: f64,
    /// Every valid pixel's L*, ascending.
    flat: Vec<f32>,
    /// Sky pixels' L*, ascending.
    sky: Vec<f32>,
    /// Face pixels' L*, ascending.
    face: Vec<f32>,
    /// Weighted mean chroma (0..1).
    pub chroma: f64,
    /// Σ detail weight × well-exposedness (Mertens), normalised.
    pub detail: f64,
}

impl Measured {
    pub(crate) fn of(img: &Rgba8, lay: &Layout) -> Measured {
        let mut weighted = Vec::with_capacity(img.data.len());
        let (mut flat, mut sky, mut face) = (Vec::with_capacity(img.data.len()), Vec::new(), Vec::new());
        let (mut total, mut chroma, mut detail, mut tex_total) = (0f64, 0f64, 0f64, 0f64);
        let same = img.width == lay.width && img.height == lay.height;
        for (i, p) in img.data.iter().enumerate() {
            if p[3] == 0 {
                continue;
            }
            let l = lstar(*p);
            let (w, s, t) = if same { (lay.w[i], lay.sky[i], lay.tex[i]) } else { (1.0, false, 1.0) };
            if same && lay.face[i] {
                face.push(l);
            }
            flat.push(l);
            if s {
                sky.push(l);
            }
            if w > 0.0 {
                weighted.push((l, w));
                total += w as f64;
                let c = (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f64 / 255.0;
                chroma += c * w as f64;
            }
            let v = l as f64 / 100.0 - 0.5;
            detail += t as f64 * (-(v * v) / 0.08).exp();
            tex_total += t as f64;
        }
        weighted.sort_by(|a, b| a.0.total_cmp(&b.0));
        flat.sort_by(f32::total_cmp);
        sky.sort_by(f32::total_cmp);
        face.sort_by(f32::total_cmp);
        Measured {
            weighted,
            total,
            flat,
            sky,
            face,
            chroma: if total > 0.0 { chroma / total } else { 0.0 },
            detail: if tex_total > 0.0 { detail / tex_total } else { 0.0 },
        }
    }

    /// The `p` (0..1) quantile of L*, subject-weighted.
    pub(crate) fn weighted(&self, p: f64) -> f64 {
        let goal = p * self.total;
        let mut acc = 0f64;
        for &(v, w) in &self.weighted {
            acc += w as f64;
            if acc >= goal {
                return v as f64;
            }
        }
        self.weighted.last().map_or(0.0, |v| v.0 as f64)
    }

    /// The `p` (0..1) quantile of L*, every pixel counting the same (clipping anywhere matters).
    pub(crate) fn flat(&self, p: f64) -> f64 {
        quantile(&self.flat, p)
    }

    /// The sky's median L*, if there is a sky.
    pub(crate) fn sky_median(&self) -> Option<f64> {
        (!self.sky.is_empty()).then(|| quantile(&self.sky, 0.5))
    }

    /// The `p` quantile of the faces' L*, if there are faces.
    pub(crate) fn face(&self, p: f64) -> Option<f64> {
        (!self.face.is_empty()).then(|| quantile(&self.face, p))
    }

    fn has_sky(&self) -> bool {
        !self.sky.is_empty()
    }
}

fn quantile(v: &[f32], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize] as f64
}

/// The `x` in `lo..=hi` where `f(x)` (increasing in `x`) reaches `target`, by bisection; the end
/// that comes closest when it never does.
fn solve(lo: f64, hi: f64, target: f64, mut f: impl FnMut(f64) -> f64) -> f64 {
    if f(hi) <= target {
        return hi;
    }
    if f(lo) >= target {
        return lo;
    }
    let (mut a, mut b) = (lo, hi);
    for _ in 0..SOLVE_STEPS {
        let m = (a + b) / 2.0;
        if f(m) < target { a = m } else { b = m }
    }
    (a + b) / 2.0
}

/// The `v` in `lo..=hi` that best balances reaching `target` (`f(v)` against it, in units of `tol`) against
/// how hard the slider is pushed (`v` in units of `scale`): the minimum of
/// `((f(v) − target) / tol)² + (v / scale)²`, by golden-section search. A target the slider can only
/// reach at great cost (or not at all) is met part way instead of slamming the slider to its end.
fn settle(lo: f64, hi: f64, target: f64, tol: f64, scale: f64, mut f: impl FnMut(f64) -> f64) -> f64 {
    let mut cost = |v: f64| ((f(v) - target) / tol).powi(2) + (v / scale).powi(2);
    let g = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (lo, hi);
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut fc, mut fd) = (cost(c), cost(d));
    for _ in 0..SETTLE_STEPS {
        if fc < fd {
            (b, d, fd) = (d, c, fc);
            c = b - g * (b - a);
            fc = cost(c);
        } else {
            (a, c, fc) = (c, d, fd);
            d = a + g * (b - a);
            fd = cost(d);
        }
    }
    (a + b) / 2.0
}

/// L* of a display-linear luminance.
fn lstar_of(y: f64) -> f64 {
    if y > 216.0 / 24389.0 { 116.0 * y.cbrt() - 16.0 } else { y * 24389.0 / 27.0 }
}

/// Display-linear luminance of an L*.
fn y_of(l: f64) -> f64 {
    if l > 8.0 { ((l + 16.0) / 116.0).powi(3) } else { l * 27.0 / 24389.0 }
}

/// Reinhard's automatic key (JGT 2002), damped: where the subject-weighted log-average sits
/// between the 1st and 99th percentiles, −1 (low key) … 1 (high key), of the unedited render.
fn key(m: &Measured) -> f64 {
    let (lo, hi) = (y_of(m.weighted(0.01)).max(1e-4).log2(), y_of(m.weighted(0.99)).max(1e-4).log2());
    if hi - lo < 0.5 {
        return 0.0;
    }
    let mut acc = 0f64;
    for &(l, w) in &m.weighted {
        acc += y_of(l as f64).max(1e-4).log2() * w as f64;
    }
    let avg = acc / m.total.max(1e-9);
    ((2.0 * avg - lo - hi) / (hi - lo)).clamp(-1.0, 1.0)
}

/// Auto settings for `src` under `s` (its tone values are ignored and replaced), in `style`.
pub fn auto_tone(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, style: Style, capture: Capture) -> AutoTone {
    let Some(proxy) = valid_proxy(src, MEASURE_EDGE * 2) else {
        return AutoTone::default();
    };
    // the levels the real renders place Shadows and Highlights by (the whole picture's, as the engine
    // measures them; from this source when the caller has none)
    let mut info = info.clone();
    if info.tone_levels.is_none() {
        info.tone_levels = crate::finish::tone_levels(&proxy);
    }
    let info = &info;
    let tg = style.targets();
    let mut base = s.clone();
    apply(&mut base, &AutoTone::default());
    let req = crate::RenderRequest { quality: crate::Quality::Draft, ..crate::RenderRequest::fit(MEASURE_EDGE, MEASURE_EDGE) };
    let render = |t: &AutoTone| {
        let mut d = base.clone();
        apply(&mut d, t);
        crate::render(&proxy, info, &d, &req).image
    };
    let neutral = render(&AutoTone::default());
    let lay = Layout::of(&neutral, &capture.faces);
    let measure = |t: &AutoTone| Measured::of(&render(t), &lay);
    // the look's own end points: what pure black and a white far past clipping render to through the
    // photo's curves, profile and LUT, so a faded curve's lifted black (or a softened white) is kept, not "fixed"
    let (curve_black, curve_white) = {
        let probe = Rgb32f::from_fn(32, 8, |x, _| if x < 16 { [0.0; 3] } else { [64.0; 3] });
        let img = crate::render(&probe, info, &base, &req).image;
        let at = |u: f32| lstar(img.data[(img.height / 2) * img.width + ((u * img.width as f32) as usize).min(img.width - 1)]) as f64;
        (at(0.25), at(0.75))
    };
    let (white, clip) = (tg.white.min(curve_white - 1.0), WHITE_CLIP_L.min(curve_white - 0.5));
    let m0 = Measured::of(&neutral, &lay);

    // a genuinely dark scene (from EXIF and the raw's own brightness) keeps its mood
    let scene_ev = capture.ev100.zip(scene_log_mean(src, info, s)).map(|(ev, m)| ev + m);
    let k = key(&m0);
    // how dark, 0 (EV ≥ DARK_SCENE_EV) … 1 (EV ≤ NIGHT_SCENE_EV); a high-key frame (a lit white
    // wall indoors) isn't a night scene however little light there was
    let dark = scene_ev.map_or(0.0, |ev| 1.0 - smoothstep(NIGHT_SCENE_EV as f32, DARK_SCENE_EV as f32, ev as f32) as f64)
        * (1.0 - smoothstep(0.2, 0.5, k as f32) as f64);
    let lerp = |a: f64, b: f64| a + (b - a) * dark;
    let key_mid = (lstar_of(0.18 * 2f64.powf(KEY_STOPS * k)) + tg.mid_shift).clamp(38.0, 70.0);
    let mid = lerp(key_mid, key_mid.min(42.0));
    let (shadow, black) = (lerp(tg.shadow, tg.shadow.min(11.0)), lerp(tg.black, tg.black.min(2.0)).max(curve_black + 1.0));
    let shadows_cap = lerp(tg.shadows_cap, tg.shadows_cap.min(35.0));
    let (ev_lo, ev_hi) = (-2.0, lerp(2.5, 1.0));
    let mut t = AutoTone::default();

    // Exposure: the subject's midtones and the visible-detail optimum, as far as the brightest tones
    // (the sky, else the 99th percentile) can still be held back by a moderate Highlights
    let ev_mid = solve(ev_lo, ev_hi, mid, |e| measure(&AutoTone { exposure: e, ..t }).weighted(0.5));
    let held = AutoTone { highlights: HELD_BY_HIGHLIGHTS, ..t };
    let ev_top = if m0.has_sky() {
        solve(ev_lo, ev_hi, tg.sky + TOP_SLACK, |e| measure(&AutoTone { exposure: e, ..held }).sky_median().unwrap_or(0.0))
    } else {
        solve(ev_lo, ev_hi, tg.top + TOP_SLACK, |e| measure(&AutoTone { exposure: e, ..held }).flat(0.99))
    };
    let ev_detail = {
        let (mut best, mut best_v) = (0.0, f64::MIN);
        let mut e = ev_lo;
        while e <= ev_hi + 1e-9 {
            let v = measure(&AutoTone { exposure: e, ..t }).detail;
            if v > best_v {
                (best, best_v) = (e, v);
            }
            e += 0.25;
        }
        best
    };
    // where the sky would blow, stop where Highlights can still hold it, but a backlit subject comes
    // no more than BACKLIT_SHORT short of its exposure (the sky then clips a little)
    // a photo whose subject already sits near its target is left nearly alone (do no harm): the
    // correction fades in as the gap grows from COMFORT_IN to COMFORT_L
    let gap = (m0.weighted(0.5) - mid).abs();
    let mut wanted = (0.8 * ev_mid + 0.2 * ev_detail) * smoothstep(COMFORT_IN as f32, COMFORT_L as f32, gap as f32) as f64;
    // faces: the subject when there are any; the brighter quarter of their tones goes part of the way to
    // light skin's zone (with the same comfort zone), and that leads the scene's own correction
    if let Some(face0) = m0.face(0.75) {
        let ev_face = solve(ev_lo, ev_hi, FACE_TARGET, |e| measure(&AutoTone { exposure: e, ..t }).face(0.75).unwrap_or(0.0));
        let comfort = smoothstep(COMFORT_IN as f32, COMFORT_L as f32, (face0 - FACE_TARGET).abs() as f32) as f64;
        wanted = 0.4 * wanted + 0.6 * ev_face * comfort;
    }
    t.exposure = (if wanted <= ev_top { wanted } else { ev_top.max(wanted - BACKLIT_SHORT) }).clamp(ev_lo, ev_hi);
    if std::env::var_os("LIGHTCRAFT_AUTO_DEBUG").is_some() {
        let sky = lay.sky.iter().filter(|s| **s).count() as f64 / lay.sky.len().max(1) as f64;
        eprintln!(
            "auto: faces {} scene EV {scene_ev:?} dark {dark:.2} key {k:.2} mid {mid:.1} sky {:.0}% ev mid {ev_mid:.2} top {ev_top:.2} detail {ev_detail:.2} → {:.2}",
            capture.faces.len(),
            sky * 100.0,
            t.exposure
        );
    }

    // Highlights (pull back only): the sky and the brightest tones
    let over = |m: &Measured| (m.flat(0.98) - tg.top).max(m.sky_median().map_or(f64::MIN, |v| v - tg.sky));
    if over(&measure(&t)) > 0.0 {
        t.highlights = settle(-100.0, 0.0, 0.0, 3.0, 70.0, |h| over(&measure(&AutoTone { highlights: h, ..t }))).round();
    }
    // Shadows (lift only)
    if measure(&t).weighted(0.10) < shadow {
        t.shadows = settle(0.0, shadows_cap, shadow, 3.0, 50.0, |v| measure(&AutoTone { shadows: v, ..t }).weighted(0.10)).round();
    }
    // a face still in shade (backlit, Exposure held back for the sky): Shadows opens it towards the
    // edge of the face's comfort zone, as CAPE lifts shadowed faces, as far as it's worth moving
    if let Some(face) = measure(&t).face(0.75)
        && face < FACE_TARGET - COMFORT_IN
    {
        let cap = shadows_cap.max(t.shadows);
        let lifted =
            settle(t.shadows, cap, FACE_TARGET - COMFORT_IN, 3.0, 50.0, |v| measure(&AutoTone { shadows: v, ..t }).face(0.75).unwrap_or(0.0));
        t.shadows = t.shadows.max(lifted.round());
    }
    // Whites: the end point, either way
    t.whites = white_point(&t, white, clip, &measure);
    // Blacks: as Lightroom sets them, to the brink of clipping — deepened until the darkest few pixels
    // (BLACK_Q) are near black — but never under a black point the look itself lifts (faded curves),
    // and for lifted-black styles lifted instead
    let b0 = measure(&t).flat(BLACK_Q);
    let black_end = (black - BLACK_BELOW).max(curve_black + 1.0);
    if !tg.lift_blacks && b0 > black_end + BLACK_SLACK {
        t.blacks = settle(-70.0, 0.0, black_end, 1.5, 50.0, |v| measure(&AutoTone { blacks: v, ..t }).flat(BLACK_Q)).round();
    } else if tg.lift_blacks && measure(&t).flat(0.005) < black {
        t.blacks = settle(0.0, 40.0, black, 2.5, 40.0, |v| measure(&AutoTone { blacks: v, ..t }).flat(0.005)).round();
    }
    // Contrast (add only), unless the photo is already strongly compressed
    let spread = |m: &Measured| m.weighted(0.75) - m.weighted(0.25);
    if t.shadows + t.highlights.abs() <= 120.0 && spread(&measure(&t)) < tg.spread {
        t.contrast = settle(0.0, tg.contrast_cap, tg.spread, 4.0, 30.0, |c| spread(&measure(&AutoTone { contrast: c, ..t }))).round();
    }
    // a last touch of Exposure for the midtones, then the white point again
    // (only back into the comfort zone around the target, not onto it)
    let (e, now) = (t.exposure, measure(&t).weighted(0.5));
    let edge = now.clamp(mid - COMFORT_IN, mid + COMFORT_IN);
    if (now - edge).abs() > 0.5 && capture.faces.is_empty() {
        t.exposure = solve((e - 0.3).max(ev_lo), (e + 0.3).min(ev_hi), edge, |e| measure(&AutoTone { exposure: e, ..t }).weighted(0.5));
    }
    t.exposure = (t.exposure * 100.0).round() / 100.0;
    t.whites = white_point(&t, white, clip, &measure);

    // colour: a little Vibrance, more for dull photos
    let chroma = measure(&t).chroma;
    t.vibrance = (tg.vibrance + ((0.15 - chroma) * 100.0).clamp(0.0, 10.0)).round();
    t.saturation = tg.saturation;
    // (a slider solved down to a hair below 0 rounds to -0)
    for v in [&mut t.exposure, &mut t.contrast, &mut t.highlights, &mut t.shadows, &mut t.whites, &mut t.blacks, &mut t.vibrance] {
        *v += 0.0;
    }
    t
}

/// Whites for `t`, as Lightroom sets them, to the brink of clipping: brought down when a real area is
/// blown (more than 2 % of the frame past `clip`; lamps, the sun and speculars may clip), else raised
/// until the brightest few pixels (WHITE_Q) reach the style's white end (`white` + WHITE_ABOVE, under
/// `clip`), else left at 0.
fn white_point(t: &AutoTone, white: f64, clip: f64, measure: &impl Fn(&AutoTone) -> Measured) -> f64 {
    let m = measure(t);
    let end = (white + WHITE_ABOVE).min(clip - 0.5);
    if m.flat(0.98) > clip {
        settle(-60.0, 0.0, clip - 1.0, 1.5, 40.0, |v| measure(&AutoTone { whites: v, ..*t }).flat(0.98)).round()
    } else if m.flat(WHITE_Q) < end - WHITE_SLACK {
        settle(0.0, 50.0, end, 1.5, 40.0, |v| measure(&AutoTone { whites: v, ..*t }).flat(WHITE_Q)).round()
    } else {
        0.0
    }
}

/// The share of pixels (from the top) that Whites brings to the brink of white, and how far above the
/// style's white that brink is; the share (from the bottom) that Blacks brings to the brink of black,
/// and how far under the style's black. Lightroom's Auto Whites stops when "only a few pixels are solid
/// white", its Blacks when "a few more pixels are solid black".
const WHITE_Q: f64 = 0.997;
const WHITE_ABOVE: f64 = 2.5;
const WHITE_SLACK: f64 = 1.0;
const BLACK_Q: f64 = 0.002;
const BLACK_BELOW: f64 = 2.5;

/// L* from which a pixel counts as blown for [`white_point`].
const WHITE_CLIP_L: f64 = 98.5;
/// How far above the brink of black the darkest tones may sit before Blacks deepens them.
const BLACK_SLACK: f64 = 1.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(img: &Rgb32f) -> AutoTone {
        auto_tone(img, &SourceInfo::default(), &DevelopSettings::default(), Style::Natural, Capture::default())
    }

    /// `img` rendered with `t` at the size Auto measures.
    fn rendered(img: &Rgb32f, t: &AutoTone) -> Rgba8 {
        let mut d = DevelopSettings::default();
        apply(&mut d, t);
        crate::render(img, &SourceInfo::default(), &d, &crate::RenderRequest::fit(MEASURE_EDGE, MEASURE_EDGE)).image
    }

    fn textured(lo: f32, hi: f32) -> Rgb32f {
        Rgb32f::from_fn(96, 64, |x, y| {
            let t = ((x * 7 + y * 13) % 23) as f32 / 22.0;
            [lo + (hi - lo) * t; 3]
        })
    }

    #[test]
    fn dark_and_bright_scenes_move_towards_the_middle_but_keep_their_key() {
        let (dark_img, bright_img) = (textured(0.003, 0.03), textured(0.8, 4.0));
        let (dark, bright) = (auto(&dark_img), auto(&bright_img));
        assert!(dark.exposure > 0.5 && bright.exposure < 0.0, "{dark:?} {bright:?}");
        let mid = |img: &Rgb32f, t: &AutoTone| {
            let r = rendered(img, t);
            Measured::of(&r, &Layout::of(&r, &[])).weighted(0.5)
        };
        let (dm, bm) = (mid(&dark_img, &dark), mid(&bright_img, &bright));
        // the dark scene comes up, the bright one stays brighter (its key is kept)
        assert!((30.0..65.0).contains(&dm) && (50.0..90.0).contains(&bm) && bm > dm, "{dm} {bm}");
    }

    #[test]
    fn sliders_follow_the_photo_and_none_is_slammed() {
        // a flat, hazy scene gets more contrast than one that already spans black to white
        let flat = auto(&textured(0.06, 0.2));
        let wide = auto(&Rgb32f::from_fn(96, 64, |x, y| [if (x / 8 + y / 8) % 2 == 0 { 0.004 } else { 1.5 }; 3]));
        assert!(flat.contrast > wide.contrast, "{flat:?} {wide:?}");
        // a bright, flat sky (the top third) over a detailed, darker subject: the sky is held back
        // and the subject brightened, rather than the sky being pulled to middle grey
        let sky_img = Rgb32f::from_fn(96, 96, |x, y| if y < 32 { [2.4, 2.5, 2.6] } else { [0.03 + ((x * 7 + y * 13) % 23) as f32 * 0.004; 3] });
        let sky = auto(&sky_img);
        // (a strongly backlit frame: the sky, 5 EV over the subject, holds Exposure back, by at most
        // BACKLIT_SHORT from what the subject wants)
        assert!(sky.exposure > -1.0 && sky.highlights < 0.0, "{sky:?}");
        for t in [flat, wide, sky] {
            assert!(t.highlights >= -100.0 && t.shadows <= 75.0 && t.contrast <= 35.0 && (-2.0..=2.5).contains(&t.exposure), "{t:?}");
        }
    }

    #[test]
    fn styles_differ_in_the_expected_direction() {
        let img = textured(0.02, 0.4);
        let at = |style| auto_tone(&img, &SourceInfo::default(), &DevelopSettings::default(), style, Capture::default());
        let (natural, bright, film) = (at(Style::Natural), at(Style::Bright), at(Style::Film));
        assert!(bright.exposure > natural.exposure, "{natural:?} {bright:?}");
        assert!(film.contrast <= natural.contrast && film.saturation < 0.0, "{natural:?} {film:?}");
        assert_eq!(Style::from_name("Punchy"), Some(Style::Punchy));
    }

    #[test]
    fn dark_scenes_from_exif_keep_their_mood() {
        // a dark scene with a few bright lights (street lamps): low key
        let img =
            Rgb32f::from_fn(96, 64, |x, y| if (x * 7 + y * 13) % 41 == 0 { [1.5; 3] } else { [0.001 + ((x * 5 + y * 3) % 17) as f32 * 0.0006; 3] });
        let day = auto(&img);
        // f/2, 1/15 s, ISO 3200: EV100 = log2(4·15) − 5 ≈ 0.9, a night street
        let night = auto_tone(
            &img,
            &SourceInfo::default(),
            &DevelopSettings::default(),
            Style::Natural,
            Capture::from_exif(Some(2.0), Some(1.0 / 15.0), Some(3200.0)),
        );
        assert!(night.exposure <= 1.0 && night.exposure < day.exposure, "{day:?} {night:?}");
    }

    #[test]
    fn pixels_that_are_not_finite_are_left_out() {
        let grey = auto(&textured(0.05, 0.25));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let src = textured(0.05, 0.25);
            let speckled = Rgb32f::from_fn(96, 64, |x, y| if (x * 7 + y * 13) % 97 == 0 { [bad; 3] } else { src.data[y * 96 + x] });
            let s = auto(&speckled);
            assert!((s.exposure - grey.exposure).abs() < 0.2, "{bad}: {s:?} {grey:?}");
        }
        for img in [Rgb32f::filled(8, 8, [f32::NAN; 3]), Rgb32f::filled(600, 600, [f32::INFINITY; 3]), Rgb32f::new(0, 0)] {
            assert_eq!(auto(&img), AutoTone::default());
        }
    }
}
