//! The tuning curve: 88 target frequencies for one specific piano.
//!
//! # What is actually being decided
//!
//! Because of inharmonicity the interval families *provably disagree*. Match
//! partial 4 of a note to partial 2 of the note an octave up and the octave
//! comes out wider than 2:1 by an amount stiffness dictates; match 6:3 and it is
//! wider still; ask for a pure twelfth and you get a third answer. Individually
//! a cent or two apart, compounded up the keyboard into several cents at the
//! extremes. Every stretch scheme in existence is a *choice* about which to
//! favour and where — there is no arrangement that satisfies them all.
//!
//! So this is an optimisation, not a calculation, and the weights are where the
//! instrument's musical opinion lives.
//!
//! # Stretch, not temperament
//!
//! A subtlety that decides whether the whole approach is sound. Writing each
//! interval as "these two partials should not beat" produces a solver that also
//! re-derives the temperament: with no inharmonicity at all it would return some
//! compromise between pure fifths and pure thirds, which is not equal
//! temperament and not what anyone asked for.
//!
//! Each requirement is therefore stated as *preserve the beating the temperament
//! already intends* — an equal-tempered fifth is supposed to beat slightly
//! narrow, and should go on doing so. Written that way the interval's just ratio
//! cancels out of the algebra, leaving
//!
//! ```text
//! x_j - x_i = 600*log2(1 + B_i m^2) - 600*log2(1 + B_j n^2)
//! ```
//!
//! where `x` is a note's deviation from equal temperament in cents, and partial
//! `m` of the lower note meets partial `n` of the upper. Set every `B` to zero
//! and every difference becomes zero: equal temperament exactly. Everything the
//! solver does is therefore *stretch*, layered on whatever temperament is
//! wanted, and a historical temperament later is an additive table rather than a
//! rewrite.
//!
//! # Why it stays linear
//!
//! Working in cents rather than hertz, every requirement above is linear in the
//! unknowns, and stiffness enters only as a known constant. Eighty-eight
//! unknowns and several hundred weighted requirements plus a smoothness penalty
//! is then one ordinary least-squares solve: fast, always the same answer twice,
//! and — the part that matters for a professional tool — **explainable**, since
//! every target can be traced back to the requirements that produced it and how
//! well each was met.

use crate::piano::{key_nominal_hz, InharmonicityModel, KEYS};
use crate::synth::partial_hz;

/// The interval families a tuner actually listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Octave,
    DoubleOctave,
    Twelfth,
    Nineteenth,
    Fifth,
    Fourth,
    Third,
    Tenth,
    Seventeenth,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Octave => "octave",
            Family::DoubleOctave => "double octave",
            Family::Twelfth => "twelfth",
            Family::Nineteenth => "nineteenth",
            Family::Fifth => "fifth",
            Family::Fourth => "fourth",
            Family::Third => "major third",
            Family::Tenth => "major tenth",
            Family::Seventeenth => "major seventeenth",
        }
    }
}

/// One way of listening to one interval: partial `lower_partial` of the lower
/// note against partial `upper_partial` of the note `semitones` above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub family: Family,
    pub semitones: u8,
    pub lower_partial: u32,
    pub upper_partial: u32,
}

const fn check(family: Family, semitones: u8, lower_partial: u32, upper_partial: u32) -> Check {
    Check {
        family,
        semitones,
        lower_partial,
        upper_partial,
    }
}

/// Every coincidence worth weighing.
///
/// A single interval offers several, and they disagree — that disagreement is
/// the entire problem. A 4:2 octave and a 6:3 octave want different widths, and
/// the solver has to choose.
pub const CHECKS: &[Check] = &[
    check(Family::Octave, 12, 2, 1),
    check(Family::Octave, 12, 4, 2),
    check(Family::Octave, 12, 6, 3),
    check(Family::Octave, 12, 8, 4),
    check(Family::DoubleOctave, 24, 4, 1),
    check(Family::DoubleOctave, 24, 8, 2),
    check(Family::Twelfth, 19, 3, 1),
    check(Family::Twelfth, 19, 6, 2),
    check(Family::Nineteenth, 31, 6, 1),
    check(Family::Fifth, 7, 3, 2),
    check(Family::Fifth, 7, 6, 4),
    check(Family::Fourth, 5, 4, 3),
    check(Family::Fourth, 5, 8, 6),
    check(Family::Third, 4, 5, 4),
    check(Family::Third, 4, 10, 8),
    check(Family::Tenth, 16, 5, 2),
    check(Family::Tenth, 16, 10, 4),
    check(Family::Seventeenth, 28, 5, 1),
    check(Family::Seventeenth, 28, 10, 2),
];

/// How much each family is listened to, before register is considered.
#[derive(Clone, Copy, Debug)]
pub struct FamilyWeights {
    pub octave: f64,
    pub double_octave: f64,
    pub twelfth: f64,
    pub nineteenth: f64,
    pub fifth: f64,
    pub fourth: f64,
    pub third: f64,
    pub tenth: f64,
    pub seventeenth: f64,
}

impl FamilyWeights {
    fn get(&self, f: Family) -> f64 {
        match f {
            Family::Octave => self.octave,
            Family::DoubleOctave => self.double_octave,
            Family::Twelfth => self.twelfth,
            Family::Nineteenth => self.nineteenth,
            Family::Fifth => self.fifth,
            Family::Fourth => self.fourth,
            Family::Third => self.third,
            Family::Tenth => self.tenth,
            Family::Seventeenth => self.seventeenth,
        }
    }
}

impl Default for FamilyWeights {
    /// Progressive thirds through the middle, with octaves absorbing the
    /// compromise there and taking over at the extremes.
    ///
    /// Thirds outweigh octaves where thirds can actually be heard. That is not a
    /// preference imposed for tidiness: below the bass staff the partials a
    /// third depends on are weak and muddy, and above the treble staff its beat
    /// rate passes twenty a second and stops being a beat at all. The register
    /// windows in [`register_weight`] encode exactly that, so thirds lead in the
    /// middle and fall silent where they have nothing to say.
    fn default() -> Self {
        Self {
            octave: 1.0,
            double_octave: 0.35,
            twelfth: 0.7,
            nineteenth: 0.15,
            fifth: 0.35,
            fourth: 0.25,
            third: 2.5,
            tenth: 1.6,
            seventeenth: 0.8,
        }
    }
}

/// Smooth 0-to-1 ramp.
fn smoothstep(x: f64, edge0: f64, edge1: f64) -> f64 {
    if (edge1 - edge0).abs() < 1e-12 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A smooth window: rises across `a0..a1`, holds, falls across `b1..b0`.
fn window(x: f64, a0: f64, a1: f64, b1: f64, b0: f64) -> f64 {
    smoothstep(x, a0, a1) * (1.0 - smoothstep(x, b1, b0))
}

/// How usable one partial is for hearing beats, from where it lands and how far
/// up the series it sits.
///
/// This is the piece that decides *which way of listening* is available in each
/// register, and without it the solver satisfies requirements no ear could check.
/// A 4:2 octave at C7 would need partial 4 of C7, near 8.6 kHz, where a piano
/// string puts almost nothing — so in the treble only low partials survive and
/// octaves are heard 2:1. The bass runs the other way: partial 1 of A0 at 27 Hz
/// is inaudible on the instrument and useless for beats, which is precisely why
/// tuners listen to 4:2 and 6:3 down there.
///
/// So the same interval is a different measurement at each end of the keyboard,
/// and this function is what makes the solver aware of it.
pub fn partial_usability(hz: f64, partial: u32) -> f64 {
    // Too low to be present on the instrument. The threshold is about the
    // bottom octave's fundamentals, not the bottom octave: A0 at 27.5 Hz is
    // effectively missing, while its second partial at 55 Hz is perfectly
    // audible and beats perfectly well. Setting this too high was worth several
    // tens of cents of spurious bass stretch, because it drove the solver off
    // the 2:1 octave and onto 6:3 — and partial 6 of a stiff bass string sits
    // sixty cents sharp, so matching it demands an enormous and quite unmusical
    // octave.
    let low = smoothstep(hz, 25.0, 65.0);
    // Too high: piano partials fade out, and beats this fast read as roughness.
    let high = 1.0 - smoothstep(hz, 3200.0, 5200.0);
    // Energy falls off up the series, so a high partial is a fainter witness and
    // the beat it makes is correspondingly harder to hear.
    //
    // The exponent matters more than it looks. A cents error produces beats in
    // proportion to partial number, so anything shallower than about 1 leaves
    // the solver preferring high partials — and partial 8 of a stiff bass string
    // sits ninety cents sharp, which no one tunes an octave by.
    let strength = 1.0 / f64::from(partial).powf(1.2);
    low * high * strength
}

/// How much a family counts at a given place on the keyboard.
///
/// Thirds and their compounds live in the middle, for the acoustic reasons given
/// on [`FamilyWeights::default`]. Octaves and twelfths run everywhere and gain
/// weight at the extremes, precisely where the thirds fade out, so the criteria
/// hand over smoothly and no seam appears in the curve.
pub fn register_weight(family: Family, lower_key: u8) -> f64 {
    let k = f64::from(lower_key);
    // Where thirds are worth listening to at all.
    let middle = window(k, 12.0, 24.0, 48.0, 62.0);
    match family {
        Family::Third => middle,
        // Tenths and seventeenths use lower partials of the upper note, so they
        // stay usable a little further up than a plain third.
        Family::Tenth => window(k, 10.0, 22.0, 46.0, 60.0),
        Family::Seventeenth => window(k, 8.0, 20.0, 40.0, 54.0),
        Family::Fifth | Family::Fourth => window(k, 14.0, 24.0, 46.0, 60.0),
        // Octaves and twelfths run everywhere, but deliberately step back where
        // thirds can be heard and take over where they cannot. This is what
        // "progressive thirds in the middle, octaves absorbing the compromise"
        // means in arithmetic: through the temperament they yield, and at the
        // extremes they carry the curve alone.
        Family::Octave | Family::Twelfth => 0.5 + 1.7 * (1.0 - middle),
        Family::DoubleOctave | Family::Nineteenth => 0.3 + 1.1 * (1.0 - middle),
    }
}

/// How the curve is to be computed.
#[derive(Clone, Debug)]
pub struct CurveConfig {
    /// Reference pitch. Not necessarily 440 — a neglected piano is often better
    /// tuned to itself than dragged up in one visit.
    pub a4_hz: f64,
    /// The key held exactly at its nominal pitch. Everything else is relative.
    pub anchor_key: u8,
    /// How strongly note-to-note smoothness is enforced. Higher irons out local
    /// wrinkles at the cost of satisfying individual intervals.
    pub smoothness: f64,
    /// How much louder a fast beat counts than a slow one.
    ///
    /// 0 weighs every interval by its error in cents; 2 weighs by beat rate
    /// squared, which is the strictly correct thing if beats are all that
    /// matter, and which in practice lets the treble drown out the bass
    /// entirely. 1 is the compromise this ships with.
    pub beat_exponent: f64,
    /// Scales the whole stretch. 1.0 is what the measurements imply; 0 gives
    /// plain equal temperament; above 1 exaggerates.
    pub stretch: f64,
    pub families: FamilyWeights,
    /// Targets the technician has overruled, as (key, cents from equal
    /// temperament). Applied exactly, then blended out across
    /// [`override_width_keys`](Self::override_width_keys).
    pub overrides: Vec<(u8, f64)>,
    /// How far an override's influence reaches, in keys either side.
    ///
    /// Overrides are deliberately *local*. Feeding them into the global solve as
    /// heavily weighted requirements is tidier on paper, but it reshapes the
    /// whole instrument: a two cent nudge in the treble was moving A0 by a cent.
    /// That is wrong twice over — overruling one note expresses a judgment about
    /// that note, usually a bad string or an odd point in the scale, and notes
    /// already tuned earlier in a session must never move underneath the
    /// technician.
    pub override_width_keys: f64,
}

impl Default for CurveConfig {
    fn default() -> Self {
        Self {
            a4_hz: 440.0,
            anchor_key: 49, // A4
            smoothness: 12.0,
            beat_exponent: 1.0,
            stretch: 1.0,
            families: FamilyWeights::default(),
            overrides: Vec::new(),
            override_width_keys: 14.0,
        }
    }
}

/// 88 target frequencies for one instrument.
#[derive(Clone, Debug, PartialEq)]
pub struct TuningCurve {
    pub a4_hz: f64,
    /// Deviation from equal temperament, in cents, per key.
    pub cents: Vec<f64>,
    /// Target frequency per key.
    pub hz: Vec<f64>,
}

impl TuningCurve {
    pub fn cents_at(&self, key: u8) -> f64 {
        self.cents[usize::from(key) - 1]
    }

    pub fn hz_at(&self, key: u8) -> f64 {
        self.hz[usize::from(key) - 1]
    }

    /// Beat rate in Hz for one coincidence, on this curve.
    ///
    /// This is the number an aural tuner actually listens for, and the reason
    /// the readout can offer beat rates beside cents.
    pub fn beat_rate(&self, model: &InharmonicityModel, lower_key: u8, c: Check) -> Option<f64> {
        let upper_key = lower_key.checked_add(c.semitones)?;
        if lower_key < 1 || upper_key > KEYS {
            return None;
        }
        let lower = partial_hz(
            self.hz_at(lower_key),
            model.b_at(lower_key),
            c.lower_partial,
        );
        let upper = partial_hz(
            self.hz_at(upper_key),
            model.b_at(upper_key),
            c.upper_partial,
        );
        Some((lower - upper).abs())
    }
}

/// How well one interval came out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckResult {
    pub lower_key: u8,
    pub check: Check,
    /// How far the interval sits from where this coincidence wanted it, in
    /// cents. Positive means wider than that partial pair asked for.
    pub error_cents: f64,
    pub beat_hz: f64,
    /// What this requirement counted for in the solve.
    pub weight: f64,
}

/// Cents by which the upper note must depart from equal temperament, relative to
/// the lower, so that partial `m` of the lower and partial `n` of the upper keep
/// the beating the temperament already intends.
///
/// This single expression is the entire physical content of the solver. Note
/// what is *absent*: the interval's just ratio, which cancels because the target
/// is the temperament's own beating rather than no beating at all. Consequently
/// two ideal strings want no stretch whatsoever, whatever interval separates
/// them.
pub fn stretch_between(b_lower: f64, b_upper: f64, lower_partial: u32, upper_partial: u32) -> f64 {
    let m = f64::from(lower_partial);
    let n = f64::from(upper_partial);
    600.0 * (1.0 + b_lower * m * m).log2() - 600.0 * (1.0 + b_upper * n * n).log2()
}

fn desired_difference_cents(
    model: &InharmonicityModel,
    lower_key: u8,
    upper_key: u8,
    c: Check,
) -> f64 {
    stretch_between(
        model.b_at(lower_key),
        model.b_at(upper_key),
        c.lower_partial,
        c.upper_partial,
    )
}

/// What one coincidence counts for in the solve.
///
/// Shared by [`solve`] and [`explain`] so the reasoning shown to a technician
/// cannot drift away from the reasoning that produced the number.
fn check_weight(
    model: &InharmonicityModel,
    cfg: &CurveConfig,
    lower_key: u8,
    upper_key: u8,
    c: Check,
) -> f64 {
    let base = cfg.families.get(c.family) * register_weight(c.family, lower_key);
    if base <= 0.0 {
        return 0.0;
    }

    // Frequencies from nominal pitch, deliberately not from the configured
    // reference: these are weights, not physics, and a curve's shape must not
    // shift because the piano is being tuned two cents low.
    let lower_hz = partial_hz(
        key_nominal_hz(lower_key, 440.0),
        model.b_at(lower_key),
        c.lower_partial,
    );
    let upper_hz = partial_hz(
        key_nominal_hz(upper_key, 440.0),
        model.b_at(upper_key),
        c.upper_partial,
    );

    // Both partials must be audible: a coincidence is only as checkable as its
    // weaker half.
    let audible = partial_usability(lower_hz, c.lower_partial)
        * partial_usability(upper_hz, c.upper_partial);
    if audible <= 0.0 {
        return 0.0;
    }

    // The same error in cents beats faster, and is easier to hear, higher up the
    // keyboard. Taken from the note's own pitch, not from the coincident
    // partial: this is about where on the instrument we are, and letting it
    // depend on partial number would be a second, wrong, vote for high partials
    // on top of the one already cast in `partial_usability`.
    let note_hz = key_nominal_hz(lower_key, 440.0);
    let sensitivity = (note_hz / 440.0).powf(cfg.beat_exponent);

    base * audible * sensitivity
}

/// Solve for the 88 targets.
///
/// Returns `None` only if the assembled system is degenerate, which the anchor
/// requirement is there to prevent.
pub fn solve(model: &InharmonicityModel, cfg: &CurveConfig) -> Option<TuningCurve> {
    let n = usize::from(KEYS);
    let mut a = vec![0.0f64; n * n];
    let mut rhs = vec![0.0f64; n];

    // Each requirement says x_j - x_i = d, weighted. Accumulated straight into
    // the normal equations; at 88 unknowns the matrix is small enough that
    // nothing cleverer is warranted.
    let add_difference = |a: &mut Vec<f64>, rhs: &mut Vec<f64>, i: usize, j: usize, d: f64, w: f64| {
        a[i * n + i] += w;
        a[j * n + j] += w;
        a[i * n + j] -= w;
        a[j * n + i] -= w;
        rhs[i] -= w * d;
        rhs[j] += w * d;
    };

    for lower_key in 1..=KEYS {
        for c in CHECKS {
            let Some(upper_key) = lower_key.checked_add(c.semitones) else {
                continue;
            };
            if upper_key > KEYS {
                continue;
            }
            let w = check_weight(model, cfg, lower_key, upper_key, *c);
            if w <= 0.0 {
                continue;
            }
            let d = cfg.stretch * desired_difference_cents(model, lower_key, upper_key, *c);
            add_difference(
                &mut a,
                &mut rhs,
                usize::from(lower_key) - 1,
                usize::from(upper_key) - 1,
                d,
                w,
            );
        }
    }

    // Smoothness: penalise curvature, so the curve cannot wander note to note
    // even where the intervals are indifferent. Without this the extremes, where
    // few requirements reach, are barely determined at all.
    for k in 1..(n - 1) {
        let w = cfg.smoothness;
        // Second difference x[k-1] - 2x[k] + x[k+1] pushed toward zero.
        let idx = [k - 1, k, k + 1];
        let coeff = [1.0, -2.0, 1.0];
        for (r, &ir) in idx.iter().enumerate() {
            for (s, &is) in idx.iter().enumerate() {
                a[ir * n + is] += w * coeff[r] * coeff[s];
            }
        }
    }

    // The anchor fixes the one degree of freedom the differences leave open.
    let anchor = usize::from(cfg.anchor_key.clamp(1, KEYS)) - 1;
    a[anchor * n + anchor] += 1e6;

    let mut cents = cholesky_solve(a, rhs, n)?;

    // Overrides land after the solve, exactly, and fade out over a fixed span.
    // Recomputed one at a time so that each ends up where it was asked to be
    // even when two sit close enough to overlap.
    let width = cfg.override_width_keys.max(1.0);
    for &(key, target) in &cfg.overrides {
        if !(1..=KEYS).contains(&key) {
            continue;
        }
        let delta = target - cents[usize::from(key) - 1];
        for k in 1..=KEYS {
            let distance = (f64::from(k) - f64::from(key)).abs();
            // Full at the overridden note, nothing beyond the span, and flat at
            // both ends so no corner appears where the influence runs out.
            let falloff = 1.0 - smoothstep(distance, 0.0, width);
            cents[usize::from(k) - 1] += delta * falloff;
        }
    }

    let hz = (1..=KEYS)
        .map(|k| key_nominal_hz(k, cfg.a4_hz) * 2f64.powf(cents[usize::from(k) - 1] / 1200.0))
        .collect();

    Some(TuningCurve {
        a4_hz: cfg.a4_hz,
        cents,
        hz,
    })
}

/// Every interval on the finished curve, with how far it landed from what its
/// partial pair wanted and how fast it beats.
///
/// This is what makes a target defensible rather than merely computed: any note
/// can be traced to the requirements that shaped it.
pub fn explain(
    model: &InharmonicityModel,
    curve: &TuningCurve,
    cfg: &CurveConfig,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    for lower_key in 1..=KEYS {
        for c in CHECKS {
            let Some(upper_key) = lower_key.checked_add(c.semitones) else {
                continue;
            };
            if upper_key > KEYS {
                continue;
            }
            let weight = check_weight(model, cfg, lower_key, upper_key, *c);
            if weight <= 0.0 {
                continue;
            }
            let wanted = cfg.stretch * desired_difference_cents(model, lower_key, upper_key, *c);
            let actual = curve.cents_at(upper_key) - curve.cents_at(lower_key);
            out.push(CheckResult {
                lower_key,
                check: *c,
                error_cents: actual - wanted,
                beat_hz: curve.beat_rate(model, lower_key, *c).unwrap_or(f64::NAN),
                weight,
            });
        }
    }
    out
}

/// Solve a symmetric positive-definite system by Cholesky decomposition.
fn cholesky_solve(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        // Not positive definite means the system is under-determined — the
        // anchor requirement exists to prevent exactly that. NaN is checked
        // explicitly rather than relying on a negated comparison.
        if d.is_nan() || d <= 0.0 {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in (j + 1)..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in (i + 1)..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::piano::{fit_model, key_name, NoteSample};

    /// A model with the same stiffness everywhere, for isolating one effect.
    fn flat_model(b: f64) -> InharmonicityModel {
        let samples: Vec<NoteSample> = [1u8, 20, 40, 60, 88]
            .iter()
            .map(|&key| NoteSample {
                key,
                f0: key_nominal_hz(key, 440.0),
                b,
                weight: 1.0,
            })
            .collect();
        fit_model(&samples).expect("no model")
    }

    /// A plausible small piano: stiffness high in the bass, least in the tenor,
    /// climbing steeply through the treble.
    fn spinet_model() -> InharmonicityModel {
        let b_at = |key: u8| {
            let k = f64::from(key);
            10f64.powf(if k < 28.0 {
                -2.70 - 0.0415 * (k - 1.0)
            } else {
                -3.70 + 0.0271 * (k - 28.0)
            })
        };
        let samples: Vec<NoteSample> = [1u8, 8, 16, 22, 28, 34, 40, 52, 64, 76, 85]
            .iter()
            .map(|&key| NoteSample {
                key,
                f0: key_nominal_hz(key, 440.0),
                b: b_at(key),
                weight: 1.0,
            })
            .collect();
        fit_model(&samples).expect("no model")
    }

    #[test]
    fn ideal_strings_ask_for_no_stretch_at_all() {
        // The claim the whole formulation rests on, tested where it lives. With
        // no stiffness every interval wants exactly its equal-tempered width —
        // not the pure ratio. A beatless formulation would instead have demanded
        // pure fifths and pure thirds, and returned a temperament nobody asked
        // for in place of a stretch.
        for c in CHECKS {
            assert_eq!(
                stretch_between(0.0, 0.0, c.lower_partial, c.upper_partial),
                0.0,
                "{} {}:{} wanted stretch from ideal strings",
                c.family.name(),
                c.lower_partial,
                c.upper_partial
            );
        }
    }

    #[test]
    fn a_nearly_perfect_piano_gets_nearly_equal_temperament() {
        // The same claim end to end. Stiffness is floored at 1e-6 by the model,
        // which is far below anything real and leaves a few hundredths of a cent
        // of stretch across the whole instrument.
        let curve = solve(&flat_model(1e-9), &CurveConfig::default()).expect("no curve");
        for key in 1..=KEYS {
            assert!(
                curve.cents_at(key).abs() < 0.1,
                "key {key} ({}) drifted {:.4} cents from equal temperament",
                key_name(key),
                curve.cents_at(key)
            );
        }
    }

    #[test]
    fn the_anchor_note_stays_where_it_was_put() {
        let curve = solve(&spinet_model(), &CurveConfig::default()).expect("no curve");
        assert!(curve.cents_at(49).abs() < 0.02, "A4 moved {:.4} cents", curve.cents_at(49));
        assert!((curve.hz_at(49) - 440.0).abs() < 0.01, "A4 is {:.4} Hz", curve.hz_at(49));
    }

    #[test]
    fn the_reference_pitch_carries_the_whole_instrument() {
        let model = spinet_model();
        let a = solve(&model, &CurveConfig::default()).unwrap();
        let b = solve(
            &model,
            &CurveConfig {
                a4_hz: 442.0,
                ..CurveConfig::default()
            },
        )
        .unwrap();
        // Same shape, moved bodily.
        for key in 1..=KEYS {
            assert!((a.cents_at(key) - b.cents_at(key)).abs() < 1e-6);
            let ratio = b.hz_at(key) / a.hz_at(key);
            assert!((ratio - 442.0 / 440.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_real_piano_gets_a_stretched_curve() {
        // The shape every piano tuner recognises: treble progressively sharp,
        // bass progressively flat, relative to equal temperament.
        let curve = solve(&spinet_model(), &CurveConfig::default()).expect("no curve");

        assert!(
            curve.cents_at(88) > 8.0,
            "top note only {:.2} cents sharp",
            curve.cents_at(88)
        );
        assert!(
            curve.cents_at(1) < -8.0,
            "bottom note only {:.2} cents flat",
            curve.cents_at(1)
        );
        // Monotone through the middle, where nothing should reverse direction.
        for key in 20..80 {
            assert!(
                curve.cents_at(key + 1) >= curve.cents_at(key) - 0.01,
                "curve dipped between {} and {}",
                key_name(key),
                key_name(key + 1)
            );
        }
    }

    #[test]
    fn a_stiffer_piano_gets_more_stretch() {
        let mild = solve(&flat_model(5e-5), &CurveConfig::default()).unwrap();
        let stiff = solve(&flat_model(5e-4), &CurveConfig::default()).unwrap();
        assert!(
            stiff.cents_at(88) > mild.cents_at(88) * 2.0,
            "stiffer piano stretched {:.2} vs {:.2} cents at the top",
            stiff.cents_at(88),
            mild.cents_at(88)
        );
        assert!(stiff.cents_at(1) < mild.cents_at(1) * 2.0);
    }

    #[test]
    fn the_stretch_control_scales_the_whole_curve() {
        let model = spinet_model();
        let normal = solve(&model, &CurveConfig::default()).unwrap();
        let none = solve(
            &model,
            &CurveConfig {
                stretch: 0.0,
                ..CurveConfig::default()
            },
        )
        .unwrap();
        let more = solve(
            &model,
            &CurveConfig {
                stretch: 1.5,
                ..CurveConfig::default()
            },
        )
        .unwrap();

        for key in 1..=KEYS {
            assert!(none.cents_at(key).abs() < 1e-6, "no stretch means no deviation");
        }
        assert!(more.cents_at(88) > normal.cents_at(88) * 1.4);
    }

    #[test]
    fn the_curve_is_smooth_from_note_to_note() {
        let curve = solve(&spinet_model(), &CurveConfig::default()).expect("no curve");
        for key in 2..KEYS {
            let curvature = curve.cents_at(key + 1) - 2.0 * curve.cents_at(key)
                + curve.cents_at(key - 1);
            assert!(
                curvature.abs() < 0.35,
                "kink of {:.3} cents at {}",
                curvature,
                key_name(key)
            );
        }
    }

    #[test]
    fn thirds_beat_progressively_faster_going_up() {
        // The owner's stated priority, checked the way he would check it: play
        // major thirds up the keyboard and their beat rates should climb
        // steadily, with no reversal.
        let model = spinet_model();
        let curve = solve(&model, &CurveConfig::default()).expect("no curve");
        let third = CHECKS.iter().find(|c| c.family == Family::Third).unwrap();

        let mut previous = 0.0;
        for key in 28..=52u8 {
            let beat = curve.beat_rate(&model, key, *third).expect("no beat rate");
            assert!(
                beat > previous - 0.15,
                "third on {} beats {:.2}/s, slower than the one below at {:.2}/s",
                key_name(key),
                beat,
                previous
            );
            previous = beat;
        }
        // And the progression should actually go somewhere.
        let low = curve.beat_rate(&model, 28, *third).unwrap();
        assert!(previous > low * 2.0, "thirds barely accelerated: {low:.2} to {previous:.2}");
    }

    #[test]
    fn octaves_come_out_wider_than_pure() {
        let model = spinet_model();
        let curve = solve(&model, &CurveConfig::default()).expect("no curve");
        for key in [20u8, 30, 40, 52, 64] {
            let width = curve.cents_at(key + 12) - curve.cents_at(key);
            assert!(
                width > 0.3,
                "octave on {} widened only {:.3} cents",
                key_name(key),
                width
            );
        }
    }

    #[test]
    fn an_override_is_respected_without_tearing_the_curve() {
        let model = spinet_model();
        let plain = solve(&model, &CurveConfig::default()).unwrap();
        let nudged = solve(
            &model,
            &CurveConfig {
                overrides: vec![(60, plain.cents_at(60) + 2.0)],
                ..CurveConfig::default()
            },
        )
        .unwrap();

        // The overruled note goes exactly where it was told.
        let moved = nudged.cents_at(60) - plain.cents_at(60);
        assert!(
            (moved - 2.0).abs() < 1e-9,
            "asked for 2.0 cents, note moved {moved:.6}"
        );
        // Neighbours follow rather than being left behind on the far side of a
        // step, so the curve stays smooth through the correction.
        let step = (nudged.cents_at(60) - nudged.cents_at(59)).abs();
        assert!(step < 1.0, "override left a {step:.2} cent cliff beside it");

        // And the influence is genuinely local: the rest of the instrument,
        // including any note already tuned this session, does not move at all.
        for key in 1..=KEYS {
            let distance = (i32::from(key) - 60).abs() as f64;
            if distance > CurveConfig::default().override_width_keys {
                let shift = (nudged.cents_at(key) - plain.cents_at(key)).abs();
                assert!(
                    shift < 1e-9,
                    "override at key 60 moved {} by {shift:.4} cents",
                    key_name(key)
                );
            }
        }
    }

    #[test]
    fn smoothness_trades_against_satisfying_intervals() {
        let model = spinet_model();
        let loose = solve(
            &model,
            &CurveConfig {
                smoothness: 1.0,
                ..CurveConfig::default()
            },
        )
        .unwrap();
        let tight = solve(
            &model,
            &CurveConfig {
                smoothness: 200.0,
                ..CurveConfig::default()
            },
        )
        .unwrap();

        let curvature = |c: &TuningCurve| {
            (2..KEYS)
                .map(|k| (c.cents_at(k + 1) - 2.0 * c.cents_at(k) + c.cents_at(k - 1)).abs())
                .fold(0.0, f64::max)
        };
        assert!(
            curvature(&tight) < curvature(&loose),
            "more smoothing should mean less curvature: {:.4} vs {:.4}",
            curvature(&tight),
            curvature(&loose)
        );
    }

    #[test]
    fn every_target_can_be_accounted_for() {
        let model = spinet_model();
        let cfg = CurveConfig::default();
        let curve = solve(&model, &cfg).unwrap();
        let results = explain(&model, &curve, &cfg);

        assert!(!results.is_empty());

        // The compromise should be a compromise: small errors spread about,
        // rather than one family satisfied and another abandoned.
        let wsse: f64 = results
            .iter()
            .map(|r| r.weight * r.error_cents * r.error_cents)
            .sum();
        let wsum: f64 = results.iter().map(|r| r.weight).sum();
        let rms = (wsse / wsum).sqrt();
        assert!(rms < 6.0, "weighted error across all intervals is {rms:.2} cents");

        // Judged by weight, not by raw worst case. Some coincidences are
        // physically unsatisfiable — an 8:2 double octave off A0 wants eighty
        // cents, because partial 8 of that string is nearly a semitone sharp —
        // and the right response is to ignore them, which is what their near
        // zero weight means. Only the checks actually being listened to have to
        // come out close.
        let heaviest = results.iter().map(|r| r.weight).fold(0.0, f64::max);
        let worst_heeded = results
            .iter()
            .filter(|r| r.weight > heaviest * 0.1)
            .map(|r| r.error_cents.abs())
            .fold(0.0, f64::max);
        assert!(
            worst_heeded < 20.0,
            "an interval carrying real weight is {worst_heeded:.1} cents out"
        );

        for family in [Family::Octave, Family::Third, Family::Twelfth] {
            assert!(
                results.iter().any(|r| r.check.family == family),
                "{} never appears in the explanation",
                family.name()
            );
        }
    }

    #[test]
    fn solving_twice_gives_the_same_answer() {
        let model = spinet_model();
        let cfg = CurveConfig::default();
        assert_eq!(solve(&model, &cfg), solve(&model, &cfg));
    }

    #[test]
    fn targets_are_real_frequencies() {
        let curve = solve(&spinet_model(), &CurveConfig::default()).unwrap();
        assert_eq!(curve.hz.len(), 88);
        for key in 1..=KEYS {
            let hz = curve.hz_at(key);
            assert!(hz.is_finite() && hz > 20.0 && hz < 4500.0, "key {key} at {hz} Hz");
            if key > 1 {
                assert!(hz > curve.hz_at(key - 1), "key {key} is not above the one below");
            }
        }
    }
}
