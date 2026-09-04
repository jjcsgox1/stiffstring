//! From a handful of measured notes to inharmonicity across all 88 keys.
//!
//! # Why a model rather than a table
//!
//! Measuring every key would take far longer than a technician will give us, so
//! we sample ten to sixteen notes and interpolate. That only works because
//! stiffness is not arbitrary from note to note: within one section of stringing
//! it varies smoothly and almost exponentially with pitch, so the logarithm of
//! the coefficient is close to a straight line in key number.
//!
//! # The break is the whole difficulty
//!
//! Pianos change stringing partway up the bass — wound strings below, plain wire
//! above — and stiffness steps discontinuously there. So the curve is two lines,
//! not one, and where they meet differs from instrument to instrument. Assuming
//! a fixed break would put a corner in the wrong place and mis-model every note
//! between the assumed break and the real one.
//!
//! We therefore search for it: try every plausible break, fit a line either
//! side, and keep the split the measurements actually support.
//!
//! # Small pianos are the hard case
//!
//! On a spinet the bass strings are short and thickly wound, which drives
//! stiffness up sharply toward the bottom and makes the step at the break more
//! violent. Those are also the instruments the owner mostly works on. Naive
//! extrapolation beyond the sampled range can produce numbers that are
//! arithmetically fine and musically absurd, so extrapolation is bounded.

use crate::inharmonicity::{Concern, NoteMeasurement};

/// A piano has 88 keys: key 1 is A0, key 88 is C8.
pub const KEYS: u8 = 88;

/// MIDI note number for a key. Key 1 (A0) is MIDI 21.
#[inline]
pub fn key_midi(key: u8) -> u8 {
    key + 20
}

/// Equal-tempered frequency of a key, for a given reference pitch.
///
/// This is only ever a *starting point*: the whole purpose of the project is
/// that the frequencies a piano should actually be tuned to are not these.
pub fn key_nominal_hz(key: u8, a4_hz: f64) -> f64 {
    // Key 49 is A4.
    a4_hz * 2f64.powf((f64::from(key) - 49.0) / 12.0)
}

/// Name of a key, such as `A0`, `C4`, `F#3`.
pub fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let midi = i32::from(key_midi(key));
    format!("{}{}", NAMES[(midi % 12) as usize], midi / 12 - 1)
}

/// What a beating note means, once we know how it is strung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeatKind {
    /// One string beating against itself — vibrating in two planes at slightly
    /// different rates, from asymmetry at the bridge or bearing points.
    ///
    /// No amount of tuning removes it. It is something to tell the client about,
    /// not something to correct.
    FalseBeat,
    /// Two or three strings out with each other. This is tunable.
    Unison,
}

/// How many strings each note has.
///
/// Pianos are strung in three bands: a handful of single wound strings at the
/// very bottom, then pairs, then triples for most of the instrument. Where the
/// transitions fall differs from piano to piano.
///
/// This is what turns a measurement into a diagnosis. The engine hears amplitude
/// modulation and can say how fast; whether that is a unison wanting attention
/// or a false beat that no tuning will cure depends entirely on how many strings
/// are there, which no amount of listening can establish. Reporting a "unison
/// spread" on a single-strung bass note is confidently wrong — which is exactly
/// what happened on the first real piano measured, where C1 showed the largest
/// beat on the instrument and has only one string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stringing {
    /// Highest key strung with a single string.
    pub single_through: u8,
    /// Highest key strung with two.
    pub double_through: u8,
}

impl Default for Stringing {
    /// A common arrangement, and no more than a starting point — the transitions
    /// vary enough between instruments that this should be corrected per piano
    /// rather than trusted.
    fn default() -> Self {
        Self {
            single_through: 8,   // A0 to E1
            double_through: 16,  // F1 to C2
        }
    }
}

impl Stringing {
    pub fn strings_at(&self, key: u8) -> u8 {
        if key <= self.single_through {
            1
        } else if key <= self.double_through {
            2
        } else {
            3
        }
    }

    /// What beating on this note means.
    pub fn beat_kind(&self, key: u8) -> BeatKind {
        if self.strings_at(key) == 1 {
            BeatKind::FalseBeat
        } else {
            BeatKind::Unison
        }
    }
}

/// One note measured on a real piano, ready to inform the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteSample {
    pub key: u8,
    /// Measured fundamental in Hz.
    pub f0: f64,
    /// Measured inharmonicity coefficient.
    pub b: f64,
    /// Relative trust, from how well the measurement held together.
    pub weight: f64,
}

impl NoteSample {
    /// Build a sample from a note measurement, taking its trustworthiness from
    /// the concerns the measurement raised about itself.
    pub fn from_measurement(key: u8, m: &NoteMeasurement) -> Self {
        let mut weight = m
            .used()
            .map(|p| p.confidence)
            .sum::<f64>()
            .max(0.0)
            / (m.used_count().max(1) as f64);

        // A fit that does not hold together, or partials that would not sit
        // still, means the number is soft — keep it, but let steadier notes
        // outvote it rather than discarding a reading we may need.
        if m.has(Concern::PoorFit) {
            weight *= 0.25;
        }
        if m.has(Concern::UnstablePartials) {
            weight *= 0.5;
        }
        if m.has(Concern::FewPartials) {
            weight *= 0.6;
        }

        Self {
            key,
            f0: m.f0,
            b: m.b,
            weight: weight.clamp(0.01, 1.0),
        }
    }
}

/// A straight line in log10(B) against key number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub intercept: f64,
    pub slope: f64,
}

impl Segment {
    #[inline]
    fn log10_b_at(&self, key: f64) -> f64 {
        self.intercept + self.slope * key
    }
}

/// Inharmonicity across the whole instrument.
///
/// The overall trend is held as one or two straight lines, which is what makes
/// the break findable and gives something intelligible to display. Individual
/// keys, though, are estimated from their neighbours rather than from the trend
/// line — see [`InharmonicityModel::b_at`].
#[derive(Clone, Debug, PartialEq)]
pub struct InharmonicityModel {
    /// First key of the plain-wire section. `None` if the measurements did not
    /// support a two-section split and a single line was used.
    pub break_key: Option<u8>,
    pub bass: Segment,
    pub plain: Segment,
    pub lowest_sampled: u8,
    pub highest_sampled: u8,
    /// RMS residual in log10 units. 0.04 is roughly ten percent in B.
    pub rms_log10: f64,
    pub samples: usize,
    /// Accepted samples as (key, log10 B, weight), sorted by key.
    accepted: Vec<(f64, f64, f64)>,
}

/// Physically plausible bounds. Below this, a string is effectively ideal;
/// above it, no piano string behaves this way and something has gone wrong.
const B_MIN: f64 = 1e-6;
const B_MAX: f64 = 3e-2;

/// How far past the sampled range the trend may be followed before it is held
/// flat. Beyond about an octave, a straight line in log B stops being credible.
const MAX_EXTRAPOLATION_KEYS: f64 = 12.0;

/// Smallest departure from the trend that can get a reading discarded, in log10
/// units. 0.18 is about fifty percent in B — far beyond the few percent that
/// genuine string-to-string variation produces, so nothing legitimate is thrown
/// away, while a false-beating string reading multiples too stiff is caught.
const MIN_REJECT_LOG10: f64 = 0.18;

/// At most this many readings may be discarded. Past that, the problem is not a
/// few bad strings.
const MAX_REJECTIONS: usize = 3;

/// A single line must leave at least this much unexplained before a second one
/// is entertained, in log10 units — roughly three and a half percent in B.
///
/// Two lines always fit at least as well as one, so without a floor here a
/// perfectly straight curve gets a spurious corner wherever rounding error
/// happens to favour one.
const MIN_TREND_RESIDUAL_LOG10: f64 = 0.015;

/// Width, in keys, of the neighbourhood consulted when estimating one key.
///
/// Wide enough that several samples contribute, so measurement noise averages
/// out; narrow enough to follow real curvature instead of straightening it.
const BANDWIDTH_KEYS: f64 = 15.0;

/// Tricube weighting: full weight at the centre, falling smoothly to nothing at
/// the edge of the neighbourhood, with no discontinuity as a sample drops out.
#[inline]
fn tricube(u: f64) -> f64 {
    if u >= 1.0 {
        0.0
    } else {
        let t = 1.0 - u * u * u;
        t * t * t
    }
}

impl InharmonicityModel {
    /// Inharmonicity coefficient for a key.
    ///
    /// Estimated by fitting a line through the *nearby* samples rather than
    /// reading the global trend. Real stiffness curves bend — a straight line in
    /// log B is a good local description and a mediocre global one — and the
    /// samples themselves carry that curvature, so consulting the neighbours
    /// recovers it instead of averaging it away.
    ///
    /// Samples on the far side of the break are excluded: wound and plain
    /// strings are different objects, and letting one speak for the other is
    /// exactly the error the break search exists to prevent.
    pub fn b_at(&self, key: u8) -> f64 {
        // Follow any trend only so far beyond where we actually measured.
        let x = f64::from(key).clamp(
            f64::from(self.lowest_sampled) - MAX_EXTRAPOLATION_KEYS,
            f64::from(self.highest_sampled) + MAX_EXTRAPOLATION_KEYS,
        );

        let estimate = match self.break_key {
            None => self.local_log10_b(x, None),
            Some(b) => {
                let bf = f64::from(b);
                let below = self.nearest_sample_below(bf);
                let above = self.nearest_sample_at_or_above(bf);

                match (below, above) {
                    // The break sits somewhere in the unsampled gap between
                    // these two notes, and nothing tells us where. Committing to
                    // a cliff at one end would make every key on the wrong side
                    // of the guess badly wrong; blending across the gap costs a
                    // little accuracy if the guess was right and avoids a large
                    // error when it was not.
                    (Some(lo), Some(hi)) if x > lo && x < hi => {
                        let t = (x - lo) / (hi - lo);
                        match (
                            self.local_log10_b(x, Some(true)),
                            self.local_log10_b(x, Some(false)),
                        ) {
                            (Some(bass), Some(plain)) => Some(bass * (1.0 - t) + plain * t),
                            (a, c) => a.or(c),
                        }
                    }
                    _ => self.local_log10_b(x, Some(x < bf)),
                }
            }
        };

        match estimate {
            Some(v) => 10f64.powf(v).clamp(B_MIN, B_MAX),
            None => {
                let segment = match self.break_key {
                    Some(b) if x < f64::from(b) => self.bass,
                    _ => self.plain,
                };
                10f64.powf(segment.log10_b_at(x)).clamp(B_MIN, B_MAX)
            }
        }
    }

    fn nearest_sample_below(&self, key: f64) -> Option<f64> {
        self.accepted
            .iter()
            .map(|(k, _, _)| *k)
            .filter(|k| *k < key)
            .fold(None, |acc: Option<f64>, k| Some(acc.map_or(k, |a| a.max(k))))
    }

    fn nearest_sample_at_or_above(&self, key: f64) -> Option<f64> {
        self.accepted
            .iter()
            .map(|(k, _, _)| *k)
            .filter(|k| *k >= key)
            .fold(None, |acc: Option<f64>, k| Some(acc.map_or(k, |a| a.min(k))))
    }

    /// Locally weighted line through the samples, evaluated at `x`.
    ///
    /// `side` restricts which samples may contribute: `Some(true)` for the bass
    /// section, `Some(false)` for plain wire, `None` for all of them.
    fn local_log10_b(&self, x: f64, side: Option<bool>) -> Option<f64> {
        let pool: Vec<&(f64, f64, f64)> = match (side, self.break_key) {
            (Some(bass), Some(b)) => self
                .accepted
                .iter()
                .filter(|(k, _, _)| (*k < f64::from(b)) == bass)
                .collect(),
            _ => self.accepted.iter().collect(),
        };
        if pool.len() < 2 {
            return pool.first().map(|(_, y, _)| *y);
        }

        // Widen the neighbourhood if the nearest samples are sparse, so a thinly
        // sampled register still has something to fit a line through.
        let mut distances: Vec<f64> = pool.iter().map(|(k, _, _)| (k - x).abs()).collect();
        distances.sort_by(f64::total_cmp);
        let third = distances[distances.len().min(3) - 1];
        let h = BANDWIDTH_KEYS.max(third * 1.3).max(1.0);

        let local: Vec<(f64, f64, f64)> = pool
            .iter()
            .map(|(k, y, w)| (*k, *y, w * tricube((k - x).abs() / h)))
            .filter(|(_, _, w)| *w > 0.0)
            .collect();

        match local.len() {
            0 => None,
            // One neighbour cannot define a slope; take its value as it stands.
            1 => Some(local[0].1),
            _ => Some(match fit_line(&local) {
                Some(seg) => seg.log10_b_at(x),
                // Every contributing sample sits on the same key.
                None => {
                    let wsum: f64 = local.iter().map(|(_, _, w)| w).sum();
                    local.iter().map(|(_, y, w)| y * w).sum::<f64>() / wsum
                }
            }),
        }
    }

    /// Inharmonicity for every key, 1 to 88.
    pub fn all(&self) -> Vec<f64> {
        (1..=KEYS).map(|k| self.b_at(k)).collect()
    }

    /// The overall trend at a key, in log10 units, ignoring local detail.
    ///
    /// Judging a sample against [`b_at`](Self::b_at) would be circular: the
    /// local estimate is built from nearby samples and so partly chases the very
    /// point being judged. The trend line does not move to accommodate one
    /// reading, which is what makes a bad one stand out.
    pub fn trend_log10_b_at(&self, key: u8) -> f64 {
        let x = f64::from(key);
        match self.break_key {
            Some(b) if x < f64::from(b) => self.bass.log10_b_at(x),
            _ => self.plain.log10_b_at(x),
        }
    }

    /// Where this model puts the wound-to-plain transition, if it found one.
    pub fn break_name(&self) -> Option<String> {
        self.break_key.map(key_name)
    }
}

/// Weighted least-squares line through (key, log10 B).
fn fit_line(points: &[(f64, f64, f64)]) -> Option<Segment> {
    let wsum: f64 = points.iter().map(|(_, _, w)| w).sum();
    if points.len() < 2 || wsum <= 0.0 {
        return None;
    }
    let mx = points.iter().map(|(x, _, w)| x * w).sum::<f64>() / wsum;
    let my = points.iter().map(|(_, y, w)| y * w).sum::<f64>() / wsum;
    let sxx: f64 = points.iter().map(|(x, _, w)| w * (x - mx) * (x - mx)).sum();
    if sxx <= 1e-12 {
        return None; // every point at the same key
    }
    let sxy: f64 = points
        .iter()
        .map(|(x, y, w)| w * (x - mx) * (y - my))
        .sum();
    let slope = sxy / sxx;
    Some(Segment {
        intercept: my - slope * mx,
        slope,
    })
}

/// Weighted least squares, reweighted a few times so that a wildly wrong point
/// pulls with bounded force.
///
/// Without this a bad string does not merely add error — it tilts the line
/// toward itself until its own residual looks reasonable, which is precisely
/// what defeats outlier detection downstream. A line that refuses to chase the
/// outlier leaves it conspicuous.
fn fit_line_robust(points: &[(f64, f64, f64)]) -> Option<Segment> {
    let mut seg = fit_line(points)?;
    for _ in 0..4 {
        let residuals: Vec<f64> = points
            .iter()
            .map(|(x, y, _)| y - seg.log10_b_at(*x))
            .collect();
        let mut abs: Vec<f64> = residuals.iter().map(|r| r.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let scale = (1.4826 * abs[abs.len() / 2]).max(1e-4);

        let reweighted: Vec<(f64, f64, f64)> = points
            .iter()
            .zip(&residuals)
            .map(|((x, y, w), r)| {
                let u = (r / scale).abs();
                let huber = if u <= 1.5 { 1.0 } else { 1.5 / u };
                (*x, *y, w * huber)
            })
            .collect();
        match fit_line(&reweighted) {
            Some(next) => seg = next,
            None => break,
        }
    }
    Some(seg)
}

fn weighted_sse(points: &[(f64, f64, f64)], seg: &Segment) -> f64 {
    points
        .iter()
        .map(|(x, y, w)| {
            let e = y - seg.log10_b_at(*x);
            w * e * e
        })
        .sum()
}

/// Fit inharmonicity across the keyboard from measured notes.
///
/// Searches for the wound-to-plain break rather than assuming it, then fits a
/// line either side. Falls back to a single line when there are too few samples
/// to support a split, or when splitting does not actually explain the data
/// better.
///
/// One outlier rejection pass runs afterwards: a single misbehaving string
/// should not tilt the curve for the notes around it.
///
/// Returns `None` with fewer than four usable samples.
pub fn fit_model(samples: &[NoteSample]) -> Option<InharmonicityModel> {
    let usable: Vec<&NoteSample> = samples
        .iter()
        .filter(|s| s.weight > 0.0 && s.b > 0.0 && s.key >= 1 && s.key <= KEYS)
        .collect();
    if usable.len() < 4 {
        return None;
    }

    let points: Vec<(f64, f64, f64)> = usable
        .iter()
        .map(|s| (f64::from(s.key), s.b.log10(), s.weight))
        .collect();

    let mut kept = usable;
    let mut model = fit_with_break_search(&points, &kept)?;

    // Now discard readings the trend cannot explain — one at a time, refitting
    // in between.
    //
    // Rejecting them in a single pass does not work, because a bad reading at
    // the edge of a section has leverage: it tilts the line toward itself until
    // its own residual looks tolerable, and in doing so makes a perfectly good
    // neighbour look wrong. Removing only the worst offender and refitting
    // breaks that, and the neighbour's residual collapses once the culprit is
    // gone.
    for _ in 0..MAX_REJECTIONS {
        if kept.len() <= 5 {
            break; // too few left to tell a bad string from a real trend
        }
        let residuals: Vec<f64> = kept
            .iter()
            .map(|s| s.b.log10() - model.trend_log10_b_at(s.key))
            .collect();

        let mut abs: Vec<f64> = residuals.iter().map(|r| r.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let mad = abs[abs.len() / 2].max(0.005);
        let threshold = (4.0 * 1.4826 * mad).max(MIN_REJECT_LOG10);

        let (worst_index, worst) = residuals
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .map(|(i, r)| (i, r.abs()))?;
        if worst <= threshold {
            break;
        }

        kept.remove(worst_index);
        let remaining: Vec<(f64, f64, f64)> = kept
            .iter()
            .map(|s| (f64::from(s.key), s.b.log10(), s.weight))
            .collect();
        match fit_with_break_search(&remaining, &kept) {
            Some(refit) => model = refit,
            None => break,
        }
    }
    Some(model)
}

fn fit_with_break_search(
    points: &[(f64, f64, f64)],
    samples: &[&NoteSample],
) -> Option<InharmonicityModel> {
    let lowest = samples.iter().map(|s| s.key).min()?;
    let highest = samples.iter().map(|s| s.key).max()?;
    let wsum: f64 = points.iter().map(|(_, _, w)| w).sum();

    let single = fit_line_robust(points)?;
    let single_sse = weighted_sse(points, &single);

    // A break anywhere in the lower half of the keyboard. Both sides need at
    // least two points to define a line, and we ask for a real improvement
    // before accepting the extra freedom two lines buy.
    let mut best: Option<(f64, u8, Segment, Segment)> = None;
    for candidate in 10..=48u8 {
        let (below, above): (Vec<_>, Vec<_>) = points
            .iter()
            .partition(|(x, _, _)| *x < f64::from(candidate));
        if below.len() < 2 || above.len() < 2 {
            continue;
        }
        let (Some(bass), Some(plain)) = (fit_line_robust(&below), fit_line_robust(&above))
        else {
            continue;
        };
        let sse = weighted_sse(&below, &bass) + weighted_sse(&above, &plain);
        if best.is_none_or(|(s, _, _, _)| sse < s) {
            best = Some((sse, candidate, bass, plain));
        }
    }

    // Two lines always fit at least as well as one, so accepting a break needs
    // both a clear improvement and something worth improving: an instrument
    // whose curve is already one straight line must not be given a corner.
    let worth_explaining = wsum * MIN_TREND_RESIDUAL_LOG10 * MIN_TREND_RESIDUAL_LOG10;
    let (break_key, bass, plain, sse) = match best {
        Some((sse, key, bass, plain)) if sse < single_sse * 0.6 && single_sse > worth_explaining => {
            (Some(key), bass, plain, sse)
        }
        _ => (None, single, single, single_sse),
    };

    let mut accepted = points.to_vec();
    accepted.sort_by(|a, b| a.0.total_cmp(&b.0));

    Some(InharmonicityModel {
        break_key,
        bass,
        plain,
        lowest_sampled: lowest,
        highest_sampled: highest,
        rms_log10: if wsum > 0.0 { (sse / wsum).sqrt() } else { 0.0 },
        samples: samples.len(),
        accepted,
    })
}

/// The notes to measure first.
///
/// Eleven notes across the compass, weighted toward the bass where the curve
/// bends hardest and where the break has to be found. At roughly ten seconds a
/// note this is under two minutes, inside the budget a technician will tolerate
/// before starting work.
///
/// Both ends are included deliberately. Inharmonicity beyond the outermost
/// sample can only be extrapolated, and a straight line in log B drifts fast:
/// leaving the top octave unsampled cost about twenty percent by C8.
pub fn anchor_keys() -> Vec<u8> {
    vec![1, 8, 16, 22, 28, 34, 40, 52, 64, 76, 85]
}

/// The next note most worth measuring, given what is already known.
///
/// Prefers wide unsampled stretches, regions the current model explains poorly,
/// and above all the neighbourhood of the break, where being wrong costs most
/// and where the curve is least constrained by anything else.
///
/// Returns `None` when nothing would meaningfully improve the model.
pub fn suggest_next_key(samples: &[NoteSample], model: &InharmonicityModel) -> Option<u8> {
    let mut keys: Vec<u8> = samples.iter().map(|s| s.key).collect();
    keys.sort_unstable();
    keys.dedup();
    if keys.len() < 2 {
        return None;
    }

    // Departure from the overall trend, which marks where the curve is bending
    // and so where the samples we have constrain it least.
    let residual_at = |key: u8| -> f64 {
        samples
            .iter()
            .filter(|s| s.key == key)
            .map(|s| (s.b.log10() - model.trend_log10_b_at(s.key)).abs())
            .fold(0.0, f64::max)
    };

    let mut best: Option<(f64, u8)> = None;
    for pair in keys.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        let gap = f64::from(high - low);
        if gap < 4.0 {
            continue; // already close enough together to interpolate through
        }
        let midpoint = low + (high - low) / 2;

        // A gap the model already explains well is less urgent than one where
        // its predictions are visibly off at the ends.
        let mut interest = gap * (1.0 + 8.0 * (residual_at(low) + residual_at(high)));

        // The break dominates: a corner in the wrong place mis-models every note
        // between where we put it and where it really is.
        if let Some(b) = model.break_key {
            if b > low && b < high {
                interest *= 3.0;
            }
        }

        if best.is_none_or(|(i, _)| interest > i) {
            best = Some((interest, midpoint));
        }
    }
    best.map(|(_, key)| key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible small piano: stiffness climbing steeply into the bass,
    /// stepping at the break, then climbing again through the treble.
    ///
    /// The wobble matters. Real strings do not sit exactly on any two-line
    /// curve, and a test whose truth is exactly the model being fitted proves
    /// only that the arithmetic works. This adds a smooth deviation the model
    /// structurally cannot represent, so the numbers below reflect what
    /// interpolation actually costs.
    fn spinet_b(key: u8) -> f64 {
        let k = f64::from(key);
        const BREAK: f64 = 28.0;
        let log10b = if k < BREAK {
            -2.70 - 0.0415 * (k - 1.0)
        } else {
            -3.70 + 0.0271 * (k - BREAK)
        };
        let wobble = 0.05 * (k / 9.0).sin();
        10f64.powf(log10b + wobble)
    }

    fn sample_at(key: u8) -> NoteSample {
        NoteSample {
            key,
            f0: key_nominal_hz(key, 440.0),
            b: spinet_b(key),
            weight: 1.0,
        }
    }

    fn samples_for(keys: &[u8]) -> Vec<NoteSample> {
        keys.iter().copied().map(sample_at).collect()
    }

    /// Worst and median relative error across every key.
    fn errors(model: &InharmonicityModel) -> (f64, f64) {
        let mut all: Vec<f64> = (1..=KEYS)
            .map(|k| (model.b_at(k) - spinet_b(k)).abs() / spinet_b(k))
            .collect();
        all.sort_by(f64::total_cmp);
        (*all.last().unwrap(), all[all.len() / 2])
    }

    #[test]
    fn beating_means_different_things_depending_on_the_stringing() {
        // The distinction that matters in the field. The engine hears a note's
        // amplitude rise and fall and can say how fast; only the stringing says
        // whether that is a unison wanting attention or a false beat that no
        // tuning will cure. Measured on a real baby grand, C1 showed the widest
        // beat on the instrument — and C1 there has one string.
        let s = Stringing::default();
        assert_eq!(s.strings_at(1), 1, "A0 is single strung");
        assert_eq!(s.strings_at(4), 1, "C1 is single strung");
        assert_eq!(s.beat_kind(4), BeatKind::FalseBeat);

        assert_eq!(s.strings_at(12), 2);
        assert_eq!(s.beat_kind(12), BeatKind::Unison);

        assert_eq!(s.strings_at(40), 3, "C4 is triple strung");
        assert_eq!(s.beat_kind(40), BeatKind::Unison);

        // And it is adjustable, because the transitions move between pianos.
        let late = Stringing {
            single_through: 12,
            double_through: 20,
        };
        assert_eq!(late.beat_kind(10), BeatKind::FalseBeat);
        assert_eq!(late.strings_at(21), 3);
    }

    #[test]
    fn keys_map_to_the_right_notes() {
        assert_eq!(key_name(1), "A0");
        assert_eq!(key_name(4), "C1");
        assert_eq!(key_name(40), "C4");
        assert_eq!(key_name(49), "A4");
        assert_eq!(key_name(88), "C8");
        assert_eq!(key_midi(49), 69);
    }

    #[test]
    fn nominal_pitches_are_equal_tempered() {
        assert!((key_nominal_hz(49, 440.0) - 440.0).abs() < 1e-9);
        assert!((key_nominal_hz(1, 440.0) - 27.5).abs() < 1e-9);
        assert!((key_nominal_hz(88, 440.0) - 4186.009).abs() < 0.01);
        // And the reference pitch moves everything together.
        assert!((key_nominal_hz(49, 442.0) - 442.0).abs() < 1e-9);
    }

    #[test]
    fn recovers_the_curve_from_the_anchor_notes_alone() {
        let model = fit_model(&samples_for(&anchor_keys())).expect("no model");
        let (worst, median) = errors(&model);
        // The worst case sits within a key or two of the break, where the curve
        // genuinely steps and nothing in the samples says exactly where. Away
        // from it the model tracks the reference closely, which is what the
        // median records.
        assert!(
            worst < 0.15,
            "worst error {:.1}% across the keyboard from the anchors",
            worst * 100.0
        );
        assert!(median < 0.04, "median error {:.1}%", median * 100.0);
    }

    #[test]
    fn finds_the_break_without_being_told_where_it_is() {
        let model = fit_model(&samples_for(&anchor_keys())).expect("no model");
        let found = model.break_key.expect("no break found");
        assert!(
            (i32::from(found) - 28).abs() <= 6,
            "break found at {found} ({}), truth is key 28 ({})",
            key_name(found),
            key_name(28)
        );
    }

    #[test]
    fn the_notes_it_asks_for_actually_improve_the_model() {
        // The property that matters: each extra note the model requests should
        // buy real accuracy, so the technician's time is not wasted.
        let mut chosen = samples_for(&[1, 16, 28, 40, 64, 76]);
        let before = errors(&fit_model(&chosen).expect("no model")).0;

        for _ in 0..4 {
            let model = fit_model(&chosen).expect("no model");
            let Some(next) = suggest_next_key(&chosen, &model) else {
                break;
            };
            assert!(
                !chosen.iter().any(|s| s.key == next),
                "asked for key {next} twice"
            );
            chosen.push(sample_at(next));
        }
        let after = errors(&fit_model(&chosen).expect("no model")).0;

        assert!(
            after < before * 0.85,
            "four requested notes should help: worst error {:.1}% -> {:.1}%",
            before * 100.0,
            after * 100.0
        );
    }

    #[test]
    fn asks_for_notes_near_the_break_first() {
        let samples = samples_for(&[1, 16, 40, 64, 76]);
        let model = fit_model(&samples).expect("no model");
        let next = suggest_next_key(&samples, &model).expect("no suggestion");
        assert!(
            (16..=40).contains(&next),
            "expected a note bracketing the break, got {next} ({})",
            key_name(next)
        );
    }

    #[test]
    fn one_bad_string_does_not_bend_the_curve() {
        // A false-beating string reading three times too stiff.
        let mut samples = samples_for(&anchor_keys());
        let clean = fit_model(&samples).expect("no model");
        samples[5].b *= 3.0;
        let spoiled = fit_model(&samples).expect("no model");

        let (clean_worst, _) = errors(&clean);
        let (spoiled_worst, _) = errors(&spoiled);
        assert!(
            spoiled_worst < clean_worst + 0.03,
            "one bad string moved the worst error from {:.1}% to {:.1}%",
            clean_worst * 100.0,
            spoiled_worst * 100.0
        );
        // And it should be thrown out, not merely diluted.
        assert!(
            spoiled.samples < clean.samples,
            "the bad reading was kept: {} samples in both fits",
            spoiled.samples
        );
    }

    #[test]
    fn a_low_confidence_reading_is_outvoted_not_obeyed() {
        let mut samples = samples_for(&anchor_keys());
        samples[7].b *= 2.5;
        samples[7].weight = 0.05; // the measurement said so itself
        let model = fit_model(&samples).expect("no model");
        let (worst, _) = errors(&model);
        assert!(worst < 0.25, "worst error {:.1}%", worst * 100.0);
    }

    #[test]
    fn extrapolation_beyond_the_sampled_range_is_bounded() {
        // Nothing above key 50 was measured, so the treble is guesswork. It must
        // stay plausible rather than running away exponentially.
        let model = fit_model(&samples_for(&[1, 10, 20, 28, 36, 44, 50])).expect("no model");
        for key in 51..=KEYS {
            let b = model.b_at(key);
            assert!(
                (B_MIN..=B_MAX).contains(&b),
                "key {key} extrapolated to B = {b:.3e}"
            );
        }
        // The value must stop climbing once it is well past the evidence.
        assert!((model.b_at(75) - model.b_at(88)).abs() < 1e-12);
    }

    #[test]
    fn a_single_section_instrument_is_not_given_a_spurious_corner() {
        // A smooth exponential with no break at all.
        let samples: Vec<NoteSample> = anchor_keys()
            .into_iter()
            .map(|key| NoteSample {
                key,
                f0: key_nominal_hz(key, 440.0),
                b: 10f64.powf(-4.2 + 0.02 * f64::from(key)),
                weight: 1.0,
            })
            .collect();
        let model = fit_model(&samples).expect("no model");
        assert!(
            model.break_key.is_none(),
            "invented a break at {:?} where the curve is one straight line",
            model.break_name()
        );
        for key in 1..=KEYS {
            let want = 10f64.powf(-4.2 + 0.02 * f64::from(key));
            assert!((model.b_at(key) - want).abs() / want < 0.02);
        }
    }

    #[test]
    fn declines_when_there_is_too_little_to_go_on() {
        assert!(fit_model(&[]).is_none());
        assert!(fit_model(&samples_for(&[1, 40, 88])).is_none());
    }

    #[test]
    fn every_key_gets_a_plausible_value() {
        let model = fit_model(&samples_for(&anchor_keys())).expect("no model");
        let all = model.all();
        assert_eq!(all.len(), 88);
        assert!(all.iter().all(|b| (B_MIN..=B_MAX).contains(b)));
    }

    #[test]
    fn sample_weight_follows_the_measurement_concerns() {
        use crate::inharmonicity::MeasuredPartial;
        let partial = |n: u32, confidence: f64| MeasuredPartial {
            n,
            hz: 220.0 * f64::from(n),
            amplitude: 0.1,
            confidence,
            beat_hz: None,
            residual_cents: 0.0,
            used: true,
        };
        let base = NoteMeasurement {
            f0: 220.0,
            b: 3e-4,
            partials: (1..=6).map(|n| partial(n, 0.99)).collect(),
            rms_cents: 0.1,
            beat_spread_cents: None,
            concerns: vec![],
        };
        let clean = NoteSample::from_measurement(40, &base);
        assert!(clean.weight > 0.9, "clean weight {}", clean.weight);

        let shaky = NoteMeasurement {
            concerns: vec![Concern::UnstablePartials, Concern::PoorFit],
            ..base
        };
        let shaky = NoteSample::from_measurement(40, &shaky);
        assert!(
            shaky.weight < clean.weight * 0.2,
            "a troubled measurement should carry far less: {} vs {}",
            shaky.weight,
            clean.weight
        );
        assert!(shaky.weight > 0.0, "but should not be discarded outright");
    }
}
