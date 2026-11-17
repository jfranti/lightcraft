//! Film: a scene-referred film rendering — negative, print and grain — that takes over from the camera
//! tone curve when the Film section has a stock (`DevelopSettings::film`).
//!
//! It follows the structure of measured film pipelines (a camera into scene-linear light, balance in
//! linear, the negative's response, the print's, grain in the negative) with our own physically
//! motivated curves, not measured data or anyone's LUTs:
//!
//! 1. **Layer exposure**: scene-linear Rec.2020 through broad film-layer sensitivities, in log10
//!    exposure relative to middle grey (Exposure and White Balance are the photographer's "balance
//!    in linear"); optional filmic compression of the brightest scene tones.
//! 2. **Negative**: per-layer characteristic (H&D) curves — base fog, toe, a straight line of the
//!    stock's gamma, a long shoulder (a negative's latitude) — then interlayer (DIR coupler)
//!    effects that push each layer's density away from the others' (film's colour separation;
//!    the vibrance levels scale them).
//! 3. **Grain** in negative density: per layer, partly shared between layers, strongest at middle
//!    densities, so the print curve shapes it as it does on film.
//! 4. **Print**: printer lights balanced so middle grey prints neutral, the print stock's curve
//!    (a motion-picture print's high contrast and dense blacks, or a gentler lab scan), and its
//!    dyes' unwanted absorptions (the print's colour signature), then display light.
//!
//! Reversal (slide) stocks have no print: one positive curve from layer exposure to display density.

use lightcraft_color::luminance_2020;

/// A film stock (see [`STOCKS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stock {
    pub id: &'static str,
    pub name: &'static str,
    pub maker: &'static str,
    pub kind: Kind,
    pub iso: u32,
    /// Balanced for daylight (D) or tungsten (T).
    pub balance: char,
    /// One line on the stock's character.
    pub notes: &'static str,
    /// Straight-line slope of density against log10 exposure, per layer (R, G, B).
    pub gamma: [f32; 3],
    /// Density range of the curve (fog to the shoulder's top): the stock's latitude.
    pub range: f32,
    /// Log10 exposure, relative to middle grey, of each layer's curve midpoint (the stock's colour balance
    /// and where grey sits on the curve).
    pub mid: [f32; 3],
    /// Interlayer effect strength (colour separation) at the Vibrant level.
    pub interlayer: f32,
    /// Dye-set unwanted absorptions of the image (print or reversal) dyes: R, G, B rows.
    pub dyes: [[f32; 3]; 3],
    /// RMS density of the grain at 35 mm and grain 50.
    pub grain: f32,
    /// Halation at Halation 50: how much of the brightest light comes back off the film base into the
    /// (mostly red) layers. Cinema negatives' remjet backing keeps it low; without one it glows.
    pub halation: f32,
    /// Black and white: the emulsion's sensitivity to linear R, G, B (sums to 1); `None`: colour.
    pub mono: Option<[f32; 3]>,
}

/// Negative (printed) or reversal (slide).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Negative,
    Reversal,
}

/// A print stock (for negatives).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Print {
    pub id: &'static str,
    pub name: &'static str,
    pub gamma: f32,
    /// Density range (base to maximum black).
    pub range: f32,
    pub dyes: [[f32; 3]; 3],
}

/// Print stocks: a motion-picture release print (high contrast, dense blacks) and a lab scan of a
/// still negative (gentler, open shadows).
pub const PRINTS: &[Print] = &[
    Print {
        id: "cine",
        name: "Motion-picture print (2383-style)",
        gamma: 2.7,
        range: 3.9,
        dyes: [[1.0, 0.08, 0.02], [0.06, 1.0, 0.10], [0.02, 0.12, 1.0]],
    },
    Print { id: "paper", name: "Fibre paper (black and white)", gamma: 1.9, range: 2.1, dyes: NEG_DYES },
    Print { id: "scan", name: "Lab scan", gamma: 1.75, range: 2.4, dyes: [[1.0, 0.05, 0.01], [0.04, 1.0, 0.06], [0.01, 0.07, 1.0]] },
];

const NEG_DYES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Film stocks: our own readings of each stock's documented character ("-style").
pub const STOCKS: &[Stock] = &[
    Stock {
        id: "vision3-500t",
        name: "Vision3 500T-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 500,
        balance: 'T',
        notes: "Fast tungsten cinema negative: wide latitude, soft highlights, visible grain.",
        gamma: [0.60, 0.62, 0.64],
        range: 2.6,
        mid: [0.45, 0.45, 0.43],
        interlayer: 0.22,
        dyes: NEG_DYES,
        grain: 0.030,
        halation: 0.35,
        mono: None,
    },
    Stock {
        id: "vision3-250d",
        name: "Vision3 250D-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 250,
        balance: 'D',
        notes: "Daylight cinema negative: natural colour, fine grain.",
        gamma: [0.62, 0.63, 0.64],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.22,
        dyes: NEG_DYES,
        grain: 0.022,
        halation: 0.3,
        mono: None,
    },
    Stock {
        id: "vision3-50d",
        name: "Vision3 50D-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 50,
        balance: 'D',
        notes: "Slow daylight cinema negative: the finest grain, crisp and saturated.",
        gamma: [0.64, 0.65, 0.66],
        range: 2.5,
        mid: [0.42, 0.42, 0.42],
        interlayer: 0.26,
        dyes: NEG_DYES,
        grain: 0.012,
        halation: 0.25,
        mono: None,
    },
    Stock {
        id: "eterna-250d",
        name: "Eterna 250D-style",
        maker: "Fujifilm",
        kind: Kind::Negative,
        iso: 250,
        balance: 'D',
        notes: "Low-contrast, low-saturation cinema negative with cool, cyan-green shadows.",
        gamma: [0.55, 0.57, 0.56],
        range: 2.7,
        mid: [0.45, 0.44, 0.44],
        interlayer: 0.12,
        dyes: NEG_DYES,
        grain: 0.022,
        halation: 0.25,
        mono: None,
    },
    Stock {
        id: "portra-400",
        name: "Portra 400-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 400,
        balance: 'D',
        notes: "Portrait negative: gentle contrast, warm natural skin, huge latitude.",
        gamma: [0.55, 0.56, 0.56],
        range: 2.8,
        mid: [0.47, 0.47, 0.46],
        interlayer: 0.14,
        dyes: NEG_DYES,
        grain: 0.024,
        halation: 0.15,
        mono: None,
    },
    Stock {
        id: "ektar-100",
        name: "Ektar 100-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 100,
        balance: 'D',
        notes: "Vivid, fine-grained negative: punchy saturation, deep reds and blues.",
        gamma: [0.66, 0.67, 0.68],
        range: 2.5,
        mid: [0.42, 0.42, 0.42],
        interlayer: 0.32,
        dyes: NEG_DYES,
        grain: 0.012,
        halation: 0.15,
        mono: None,
    },
    Stock {
        id: "gold-200",
        name: "Gold 200-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 200,
        balance: 'D',
        notes: "Consumer negative: warm, golden, nostalgic.",
        gamma: [0.62, 0.61, 0.58],
        range: 2.6,
        mid: [0.42, 0.44, 0.48],
        interlayer: 0.2,
        dyes: NEG_DYES,
        grain: 0.022,
        halation: 0.18,
        mono: None,
    },
    Stock {
        id: "cinestill-800t",
        name: "CineStill 800T-style",
        maker: "CineStill",
        kind: Kind::Negative,
        iso: 800,
        balance: 'T',
        notes: "Cinema 500T emulsion without its anti-halation backing, pushed: red-orange glows around lights, cool tungsten balance.",
        gamma: [0.60, 0.62, 0.64],
        range: 2.6,
        mid: [0.45, 0.45, 0.43],
        interlayer: 0.22,
        dyes: NEG_DYES,
        grain: 0.034,
        halation: 1.6,
        mono: None,
    },
    Stock {
        id: "ektachrome-e100",
        name: "Ektachrome E100-style",
        maker: "Kodak",
        kind: Kind::Reversal,
        iso: 100,
        balance: 'D',
        notes: "Slide film: clean, neutral-to-cool, fine grain, steep contrast.",
        gamma: [1.55, 1.58, 1.62],
        range: 3.4,
        mid: [0.05, 0.05, 0.03],
        interlayer: 0.18,
        dyes: [[1.0, 0.06, 0.02], [0.05, 1.0, 0.08], [0.02, 0.08, 1.0]],
        grain: 0.010,
        halation: 0.1,
        mono: None,
    },
    Stock {
        id: "velvia-50",
        name: "Velvia 50-style",
        maker: "Fujifilm",
        kind: Kind::Reversal,
        iso: 50,
        balance: 'D',
        notes: "Slide film: very saturated, very contrasty, vivid greens and blues.",
        gamma: [1.75, 1.78, 1.8],
        range: 3.6,
        mid: [0.04, 0.05, 0.05],
        interlayer: 0.34,
        dyes: [[1.0, 0.10, 0.04], [0.08, 1.0, 0.12], [0.03, 0.10, 1.0]],
        grain: 0.008,
        halation: 0.08,
        mono: None,
    },
    Stock {
        id: "kodachrome-64",
        name: "Kodachrome 64-style",
        maker: "Kodak",
        kind: Kind::Reversal,
        iso: 64,
        balance: 'D',
        notes: "Slide film: rich warm reds and yellows, dense shadows, deep blue skies.",
        gamma: [1.65, 1.6, 1.55],
        range: 3.5,
        mid: [0.02, 0.05, 0.08],
        interlayer: 0.24,
        dyes: [[1.0, 0.12, 0.05], [0.06, 1.0, 0.14], [0.02, 0.12, 1.0]],
        grain: 0.010,
        halation: 0.08,
        mono: None,
    },
    Stock {
        id: "tri-x-400",
        name: "Tri-X 400-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 400,
        balance: 'D',
        notes: "Classic photojournalism black and white: punchy contrast, gritty grain.",
        gamma: [0.68, 0.68, 0.68],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.0,
        dyes: NEG_DYES,
        grain: 0.04,
        halation: 0.12,
        mono: Some([0.3, 0.59, 0.11]),
    },
    Stock {
        id: "hp5-400",
        name: "HP5 Plus 400-style",
        maker: "Ilford",
        kind: Kind::Negative,
        iso: 400,
        balance: 'D',
        notes: "Forgiving black and white: gentler contrast, open shadows, visible grain.",
        gamma: [0.6, 0.6, 0.6],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.0,
        dyes: NEG_DYES,
        grain: 0.036,
        halation: 0.12,
        mono: Some([0.31, 0.57, 0.12]),
    },
    Stock {
        id: "tmax-100",
        name: "T-Max 100-style",
        maker: "Kodak",
        kind: Kind::Negative,
        iso: 100,
        balance: 'D',
        notes: "Modern tabular-grain black and white: very fine grain, crisp, neutral tones.",
        gamma: [0.64, 0.64, 0.64],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.0,
        dyes: NEG_DYES,
        grain: 0.014,
        halation: 0.12,
        mono: Some([0.28, 0.6, 0.12]),
    },
    Stock {
        id: "acros-100",
        name: "Acros 100-style",
        maker: "Fujifilm",
        kind: Kind::Negative,
        iso: 100,
        balance: 'D',
        notes: "Fine-grained black and white with low red sensitivity: darker reds and skies, smooth tones.",
        gamma: [0.66, 0.66, 0.66],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.0,
        dyes: NEG_DYES,
        grain: 0.012,
        halation: 0.12,
        mono: Some([0.22, 0.64, 0.14]),
    },
    Stock {
        id: "delta-3200",
        name: "Delta 3200-style",
        maker: "Ilford",
        kind: Kind::Negative,
        iso: 3200,
        balance: 'D',
        notes: "Very fast black and white: low contrast, heavy grain, made for available light.",
        gamma: [0.55, 0.55, 0.55],
        range: 2.6,
        mid: [0.45, 0.45, 0.45],
        interlayer: 0.0,
        dyes: NEG_DYES,
        grain: 0.065,
        halation: 0.12,
        mono: Some([0.3, 0.58, 0.12]),
    },
];

/// The stock with id `id`.
pub fn stock(id: &str) -> Option<&'static Stock> {
    STOCKS.iter().find(|s| s.id == id)
}

/// The print with id `id`, else the motion-picture print.
pub fn print(id: &str) -> &'static Print {
    PRINTS.iter().find(|p| p.id == id).unwrap_or(&PRINTS[0])
}

/// The print a stock is usually seen through: cinema stocks the motion-picture print, still negatives a lab scan.
pub fn default_print(s: &Stock) -> &'static str {
    if s.mono.is_some() {
        "paper"
    } else if s.name.contains("Vision3") || s.name.contains("Eterna") {
        "cine"
    } else {
        "scan"
    }
}

/// Linear Rec.2020 → the film layers' broad sensitivities (half way to Rec.709 primaries).
const SENS: [[f32; 3]; 3] = [[1.3303, -0.2938, -0.0364], [-0.0623, 1.0665, -0.0042], [-0.0091, -0.0503, 1.0594]];
/// Linear Rec.709 → linear Rec.2020 (the print's display light into the working space).
const R709_TO_2020: [[f32; 3]; 3] = [[0.6274, 0.3293, 0.0433], [0.0691, 0.9195, 0.0114], [0.0164, 0.0880, 0.8956]];
/// Display luminance a middle-grey scene prints to (about L* 47: prints sit a little dense).
const GREY_OUT: f32 = 0.165;

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// A logistic of slope `gamma` at `mid`, spanning `range`.
#[derive(Clone, Copy, Debug)]
struct Curve {
    k: f32,
    mid: f32,
    range: f32,
}

impl Curve {
    fn new(gamma: f32, mid: f32, range: f32) -> Curve {
        Curve { k: 4.0 * gamma / range, mid, range }
    }
    fn eval(&self, x: f32) -> f32 {
        self.range / (1.0 + (-self.k * (x - self.mid)).exp())
    }
    /// The `x` where the curve reaches `y` (0 < y < range).
    fn inverse(&self, y: f32) -> f32 {
        self.mid - (self.range / y.clamp(1e-4, self.range - 1e-4) - 1.0).ln() / self.k
    }
}

/// Everything a render needs, prepared once from the settings.
#[derive(Clone, Debug)]
pub struct Model {
    reversal: bool,
    /// Black and white: the emulsion's R, G, B sensitivity (one silver layer).
    mono: Option<[f32; 3]>,
    neg: [Curve; 3],
    interlayer: f32,
    /// Printer lights (log10), per layer.
    lights: [f32; 3],
    print: [Curve; 3],
    dyes: [[f32; 3]; 3],
    /// Display density of the print's base (its whitest), per channel, after the dyes.
    base: [f32; 3],
    compress: bool,
    desaturate: bool,
    /// Grain RMS in negative density (0: none) and its cell size relative to the long edge.
    pub grain: f32,
    pub grain_cell: f32,
    /// Scene-light gain before the film ([`Model::match_grey`]): the film's balance to the photo.
    pub gain: f32,
}

impl Model {
    /// The model for `film` (`None` when it is off or names no known stock).
    pub fn new(film: &lightcraft_develop::Film) -> Option<Model> {
        let s = stock(&film.stock)?;
        let vib = film.vibrance.clamp(0.0, 2.0) as f32;
        let interlayer = s.interlayer * (0.55 + 0.45 * vib);
        let reversal = s.kind == Kind::Reversal;
        let neg: [Curve; 3] = std::array::from_fn(|i| Curve::new(s.gamma[i], s.mid[i], s.range));
        let (print, dyes, lights) = if reversal {
            // one positive curve: more exposure, less density
            (neg, s.dyes, [0.0; 3])
        } else {
            // black and white prints on paper whatever print is named
            let p = print(if film.print.is_empty() || s.mono.is_some() { default_print(s) } else { &film.print });
            let target = -(GREY_OUT.log10());
            let curves: [Curve; 3] = std::array::from_fn(|_| Curve::new(p.gamma, 0.0, p.range));
            // printer lights: grey's negative density, plus the exposure that prints it to the target
            let grey = Self::negative(&neg, interlayer, [0.0; 3]);
            let lights = std::array::from_fn(|i| curves[i].inverse(target) + grey[i]);
            (curves, p.dyes, lights)
        };
        let mut m = Model {
            reversal,
            mono: s.mono,
            neg,
            interlayer,
            lights,
            print,
            dyes,
            base: [0.0; 3],
            compress: film.highlight_compression,
            desaturate: film.desaturate_ends,
            grain: 0.0,
            grain_cell: 0.0,
            gain: 1.0,
        };
        m.base = mul(&m.dyes, m.print_density([0.0; 3], true));
        // reversal: scale so middle grey lands on the target too
        if reversal {
            let g = m.display([0.0; 3], [0.0; 3]);
            let y = 0.2126 * g[0] + 0.7152 * g[1] + 0.0722 * g[2];
            let shift = (y / GREY_OUT).log10() / s.gamma[1];
            m.neg = std::array::from_fn(|i| Curve::new(s.gamma[i], s.mid[i] + shift, s.range));
            m.print = m.neg;
        }
        let (size, rms) = match film.format.as_str() {
            "16mm" => (1.6, 1.45),
            "120" => (0.7, 0.6),
            _ => (1.0, 1.0),
        };
        let speed = (s.iso as f32 / 100.0).max(0.25).sqrt();
        m.grain = s.grain * rms * (film.grain.clamp(0.0, 100.0) as f32 / 50.0);
        m.grain_cell = (0.0005 + 0.00055 * speed) * size;
        Some(m)
    }

    /// Negative densities (above fog) of layer exposures `x` (log10 relative to grey), with interlayer effects.
    fn negative(neg: &[Curve; 3], interlayer: f32, x: [f32; 3]) -> [f32; 3] {
        let d: [f32; 3] = std::array::from_fn(|i| neg[i].eval(x[i]));
        let mean = (d[0] + d[1] + d[2]) / 3.0;
        d.map(|v| v + interlayer * (v - mean))
    }

    /// Print (or reversal) densities for layer exposures `x`; `white` asks for the base's (clear film).
    fn print_density(&self, x: [f32; 3], white: bool) -> [f32; 3] {
        if self.reversal {
            if white {
                return [0.0; 3];
            }
            // positive: density falls as exposure rises
            let d: [f32; 3] = std::array::from_fn(|i| self.print[i].range - self.print[i].eval(x[i]));
            let mean = (d[0] + d[1] + d[2]) / 3.0;
            return d.map(|v| (v + self.interlayer * (v - mean)).max(0.0));
        }
        if white {
            return [0.0; 3];
        }
        let neg = Self::negative(&self.neg, self.interlayer, x);
        std::array::from_fn(|i| self.print[i].eval(self.lights[i] - neg[i]))
    }

    /// Display light (linear Rec.709) for layer exposures `x`, with `grain` added to the negative densities.
    fn display(&self, x: [f32; 3], grain: [f32; 3]) -> [f32; 3] {
        let p = if self.reversal {
            let p = self.print_density(x, false);
            std::array::from_fn(|i| (p[i] - grain[i]).max(0.0))
        } else {
            let neg = Self::negative(&self.neg, self.interlayer, x);
            let neg: [f32; 3] = std::array::from_fn(|i| neg[i] + grain[i]);
            std::array::from_fn(|i| self.print[i].eval(self.lights[i] - neg[i]))
        };
        let p = mul(&self.dyes, p);
        std::array::from_fn(|i| 10f32.powf(-(p[i] - self.base[i]).max(0.0)))
    }

    /// Grain densities for noise samples `n` (each about −1…1) at layer exposures `x`: strongest in the
    /// middle of the negative's range, where silver halide grains are largest relative to the density.
    pub fn grain_for(&self, x: [f32; 3], n: [f32; 3]) -> [f32; 3] {
        if self.grain <= 0.0 {
            return [0.0; 3];
        }
        std::array::from_fn(|i| {
            let t = (self.neg[i].eval(x[i]) / self.neg[i].range).clamp(0.0, 1.0);
            let w = 0.35 + 0.65 * (4.0 * t * (1.0 - t));
            self.grain * w * n[i]
        })
    }

    /// The model's numbers for the GPU kernel (`lightcraft-gpu`'s `finish.wgsl` mirrors [`Model::render`]):
    /// `[reversal, mono?, mono R, G, B, compress, desaturate, interlayer, grain, 1 / grain cell]`, then per
    /// channel the negative curve `(k, mid, range)`, the printer light, the print curve `(k, mid, range)`,
    /// the dyes' row and the base density.
    pub fn gpu_words(&self) -> Vec<f32> {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        let w = self.mono.unwrap_or([0.0; 3]);
        let mut v = vec![
            b(self.reversal),
            b(self.mono.is_some()),
            w[0],
            w[1],
            w[2],
            b(self.compress),
            b(self.desaturate),
            self.interlayer,
            self.grain,
            1.0 / self.grain_cell.max(1e-9),
        ];
        for i in 0..3 {
            let (n, p) = (self.neg[i], self.print[i]);
            v.extend([n.k, n.mid, n.range, self.lights[i], p.k, p.mid, p.range, self.dyes[i][0], self.dyes[i][1], self.dyes[i][2], self.base[i]]);
        }
        v.push(self.gain);
        v
    }

    /// Balance the film to the photo: the scene-light gain that renders middle grey to `target` (display
    /// luminance: what the camera's own tone curve gives it), so choosing a stock keeps the photo's
    /// brightness and changes its character — as balancing exposure in linear does before a film LUT.
    pub fn match_grey(&mut self, target: f32) {
        let target = target.clamp(0.05, 0.6);
        let (mut lo, mut hi) = (-5f32, 6f32);
        for _ in 0..32 {
            let m = (lo + hi) / 2.0;
            self.gain = m.exp2();
            if luminance_2020(self.render([0.18; 3], [0.0; 3])) < target {
                lo = m;
            } else {
                hi = m;
            }
        }
        self.gain = ((lo + hi) / 2.0).exp2();
    }

    /// Black and white: its grain is the same in every channel.
    pub fn is_mono(&self) -> bool {
        self.mono.is_some()
    }

    /// Layer exposures (log10 relative to middle grey) of scene-linear Rec.2020 `c`.
    pub fn exposure(&self, c: [f32; 3]) -> [f32; 3] {
        // black and white: one silver layer with the emulsion's own colour sensitivity
        let layers = match self.mono {
            Some(w) => [w[0] * c[0] + w[1] * c[1] + w[2] * c[2]; 3],
            None => mul(&SENS, c),
        };
        layers.map(|v| {
            let x = (v * self.gain).max(1e-6).log10() - 0.18f32.log10();
            if self.compress && x > 0.3 {
                // a filmic roll-off of the brightest scene tones
                0.3 + (x - 0.3) / (1.0 + (x - 0.3) / 0.9)
            } else {
                x
            }
        })
    }

    /// Scene-linear Rec.2020 `c` through the film, with grain densities `grain`, as display-linear Rec.2020.
    pub fn render(&self, c: [f32; 3], grain: [f32; 3]) -> [f32; 3] {
        let x = self.exposure(c);
        let mut d = mul(&R709_TO_2020, self.display(x, grain));
        if self.desaturate {
            let y = luminance_2020(d);
            let w = 0.35 * (1.0 - smooth(0.02, 0.15, y)) + 0.35 * smooth(0.55, 0.95, y);
            d = d.map(|v| v + (y - v) * w);
        }
        d
    }
}

/// Halation for `film` in a render `px_per_long` pixels across the long edge, where scene-linear values
/// times `scale` are the film's exposure (middle grey at 0.18: the exposure slider's gain times the
/// film's grey match): `[strength, sigma_px, threshold]`, the threshold in the image's own linear units,
/// or `None` when there is none. The glow's size is a fraction of the frame — light spreading a fraction
/// of a millimetre in the base is a bigger share of a 16 mm frame than of 35 mm or 120.
pub fn halation(film: &lightcraft_develop::Film, px_per_long: f64, scale: f32) -> Option<[f32; 3]> {
    let s = stock(&film.stock)?;
    let strength = s.halation * (film.halation.clamp(0.0, 100.0) as f32 / 50.0);
    let frac = match film.format.as_str() {
        "16mm" => 0.016,
        "120" => 0.004,
        _ => 0.006,
    };
    let sigma = (frac * px_per_long) as f32;
    // light about two and a half stops over middle grey starts to come back off the base
    let threshold = HALATION_KNEE / scale.max(1e-6);
    (strength > 0.0 && sigma >= 0.5).then_some([strength, sigma, threshold])
}

/// Film exposure (middle grey 0.18) where halation begins: about 2.5 stops over grey.
const HALATION_KNEE: f32 = 1.0;

/// Add halation to scene-linear `img`: the light over `threshold` (see [`halation`]) comes back off the
/// film base, spread by `sigma` (with a wider, fainter fill), into the red layer most, the green a
/// little — the red-orange glow around lamps, windows, speculars and bright sky against dark edges.
/// The image under it stays sharp.
pub fn add_halation(img: &mut lightcraft_raster::Rgb32f, [strength, sigma, threshold]: [f32; 3]) {
    let bright = img.map(|c| halation_source(c, threshold));
    let near = lightcraft_raster::blur::gaussian(&bright, sigma);
    let far = lightcraft_raster::blur::gaussian(&bright, sigma * 3.0);
    for ((p, a), b) in img.data.iter_mut().zip(&near.data).zip(&far.data) {
        for k in 0..3 {
            p[k] += strength * (0.7 * a[k] + 0.3 * b[k]);
        }
    }
}

/// The light pixel `c` sends back off the base: its excess over `threshold` (a soft knee, rolling off
/// so a clipped sun does not flood the frame), tinted red-orange.
pub fn halation_source(c: [f32; 3], threshold: f32) -> [f32; 3] {
    const TINT: [f32; 3] = [1.0, 0.32, 0.06];
    let over = (luminance_2020(c) / threshold - 1.0).max(0.0);
    let h = threshold * over / (1.0 + over / 4.0);
    TINT.map(|t| t * h)
}

/// The film's sharpness as a Gaussian blur of the scene image, in output pixels (0: none) for a render
/// `px_per_long` pixels across the long edge. The negative-and-print system resolves about 25 line pairs
/// per mm at its 50 % contrast point (a little more for slow stocks); a Gaussian of `σ = 0.187 / f50`
/// (in picture widths, `f50` in cycles across the frame) has that MTF50. So 35 mm softens a 6000 px
/// image by about 1.2 px, 16 mm (a frame 10.3 mm wide) by about 4.4 px, 120 by under 1 px — real
/// detail lost, as in printing to film, before the grain.
pub fn mtf_sigma(film: &lightcraft_develop::Film, px_per_long: f64) -> f32 {
    let Some(s) = stock(&film.stock) else { return 0.0 };
    let width_mm = match film.format.as_str() {
        "16mm" => 10.3,
        "120" => 56.0,
        _ => 36.0,
    };
    let lp_per_mm = 25.0 * (100.0 / s.iso.max(1) as f64).powf(0.15);
    (0.187 / (width_mm * lp_per_mm) * px_per_long) as f32
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    use lightcraft_develop::Film;

    fn film(stock: &str) -> lightcraft_develop::Film {
        lightcraft_develop::Film { stock: stock.into(), ..Default::default() }
    }

    #[test]
    fn every_stock_prints_grey_neutral_and_in_order() {
        for s in STOCKS {
            for p in ["", "cine", "scan"] {
                let m = Model::new(&lightcraft_develop::Film { print: p.into(), ..film(s.id) }).expect(s.id);
                let mut prev = -1.0f32;
                for i in 0..=40 {
                    let v = 0.18 * 2f32.powf(-8.0 + i as f32 * 0.35);
                    let o = m.render([v; 3], [0.0; 3]);
                    let y = luminance_2020(o);
                    assert!(o.iter().all(|c| c.is_finite() && *c >= 0.0 && *c <= 1.0001), "{} {p}: {o:?}", s.id);
                    assert!(y >= prev - 1e-5, "{} {p}: not monotone at {i}", s.id);
                    prev = y;
                    // neutral in, (near) neutral out
                    let spread = o.iter().cloned().fold(f32::MIN, f32::max) - o.iter().cloned().fold(f32::MAX, f32::min);
                    assert!(spread < 0.06 + 0.05 * y, "{} {p}: grey {v} → {o:?}", s.id);
                }
                // middle grey lands near the target, deep shadow is dark, a bright highlight is light
                let grey = luminance_2020(m.render([0.18; 3], [0.0; 3]));
                assert!((grey - GREY_OUT).abs() < 0.06, "{} {p}: grey → {grey}", s.id);
                assert!(luminance_2020(m.render([0.002; 3], [0.0; 3])) < 0.05, "{} {p}", s.id);
                assert!(luminance_2020(m.render([2.0; 3], [0.0; 3])) > 0.6, "{} {p}", s.id);
            }
        }
    }

    #[test]
    fn black_and_white_stocks_render_grey_with_their_own_colour_response() {
        let red = [0.4, 0.05, 0.05];
        let tri_x = Model::new(&film("tri-x-400")).unwrap();
        let acros = Model::new(&film("acros-100")).unwrap();
        for m in [&tri_x, &acros] {
            let o = m.render(red, [0.0; 3]);
            assert!((o[0] - o[1]).abs() < 1e-4 && (o[1] - o[2]).abs() < 1e-4, "{o:?}");
        }
        // Acros sees red less: a red renders darker
        assert!(luminance_2020(acros.render(red, [0.0; 3])) < luminance_2020(tri_x.render(red, [0.0; 3])));
    }

    #[test]
    fn halation_glows_around_bright_light_only() {
        let mut img = lightcraft_raster::Rgb32f::filled(64, 64, [0.1; 3]);
        img.data[32 * 64 + 32] = [40.0; 3];
        let before = img.clone();
        add_halation(&mut img, [1.0, 3.0, 1.0]);
        let near = img.data[32 * 64 + 35];
        assert!(near[0] > before.data[32 * 64 + 35][0] && near[0] > near[1] && near[1] > near[2], "red-orange glow: {near:?}");
        assert_eq!(img.data[2 * 64 + 2], before.data[2 * 64 + 2], "far from the light nothing changes");
        let off = Film { halation: 0.0, ..film("vision3-500t") };
        assert!(
            halation(&off, 6000.0, 1.0).is_none()
                && halation(&film("cinestill-800t"), 6000.0, 1.0).unwrap()[0] > halation(&film("vision3-500t"), 6000.0, 1.0).unwrap()[0]
        );
        // the threshold follows the exposure: a raw file two stops under still glows where it is bright
        let [_, _, under] = halation(&film("vision3-500t"), 6000.0, 4.0).unwrap();
        assert!((under - 0.25).abs() < 1e-6);
        let mut dim = lightcraft_raster::Rgb32f::filled(64, 64, [0.025; 3]);
        dim.data[32 * 64 + 32] = [1.0; 3];
        let before = dim.clone();
        add_halation(&mut dim, [1.0, 3.0, under]);
        assert!(dim.data[32 * 64 + 35][0] > before.data[32 * 64 + 35][0]);
    }

    #[test]
    fn vibrance_separates_colour_and_grain_follows_the_settings() {
        let colour = [0.30, 0.12, 0.06];
        let sat = |v: f64| {
            let m = Model::new(&lightcraft_develop::Film { vibrance: v, ..film("vision3-250d") }).unwrap();
            let o = m.render(colour, [0.0; 3]);
            o[0] / o[2].max(1e-6)
        };
        assert!(sat(2.0) > sat(1.0) && sat(1.0) > sat(0.0));
        let g =
            |grain: f64, format: &str| Model::new(&lightcraft_develop::Film { grain, format: format.into(), ..film("vision3-500t") }).unwrap().grain;
        assert!(g(0.0, "35mm") == 0.0 && g(100.0, "35mm") > g(50.0, "35mm") && g(50.0, "16mm") > g(50.0, "35mm"));
        assert!(Model::new(&film("")).is_none() && Model::new(&film("no-such-stock")).is_none());
    }
}
