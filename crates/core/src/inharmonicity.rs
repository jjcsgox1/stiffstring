//! Measuring one note: which partials are present, where they are, and what
//! stiffness coefficient explains them.
//!
//! # The assignment problem
//!
//! [`crate::estimate`] can pin a partial down once told roughly where to look.
//! It cannot say *which* partial it found. That matters because the stiffness
//! coefficient is recovered from how partial frequencies grow with partial
//! number, so a misnumbered partial does not merely add noise — it produces a
//! confident, wrong answer.
//!
//! Assignment would be easy if partials sat at exact multiples of the
//! fundamental. They do not, and the deviation grows as the square of the
//! partial number: by the tenth partial of a stiff bass string it can exceed
//! half the spacing between neighbouring partials, which is precisely the
//! margin assignment depends on.
//!
//! The way out is that we already know roughly which note was struck, because
//! the technician is working through a sequence. That hint plus a search over
//! plausible stiffness values pins the numbering down, and once numbered, the
//! partials determine the fundamental far more precisely than the hint did.
//!
//! # The bass, where this earns its place
//!
//! Below about C2 the fundamental is inaudible to a phone — weak on the
//! instrument and filtered by the microphone. Phase 0a confirmed it on a real
//! A0: the strongest thing there was the *fifth* partial, with the fundamental
//! and second partial absent entirely.
//!
//! So in the bass the fundamental is not measured at all. It is inferred from
//! partials four and above through the stiffness model, which is why
//! inharmonicity is a prerequisite for bass pitch detection rather than a
//! feature layered on top of it.

use crate::estimate::{cents, coarse_peaks, refine, Peak, RefineConfig};
use crate::synth::partial_hz;

/// One partial, as measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeasuredPartial {
    /// Partial number, 1-based.
    pub n: u32,
    pub hz: f64,
    pub amplitude: f64,
    /// From the phase fit: low means beating, a false beat, or noise.
    pub confidence: f64,
    /// Cents from where the fitted model says this partial should be.
    pub residual_cents: f64,
    /// False if the fit rejected it as an outlier.
    pub used: bool,
}

/// Something worth telling the technician about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Concern {
    /// The fundamental was not detectable. Normal in the bass, and handled;
    /// unexpected higher up.
    FundamentalMissing,
    /// Too few partials for the fit to be well determined.
    FewPartials,
    /// Partials do not lie on any stiff-string curve. Usually a badly false
    /// string, or the wrong note.
    PoorFit,
    /// Partials wobble instead of holding still: a beating unison, or a false
    /// beat on one string.
    UnstablePartials,
    /// One or more partials were discarded as outliers.
    PartialsRejected,
}

/// Everything learned from one struck note.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteMeasurement {
    /// Fundamental in Hz — inferred from upper partials, not necessarily heard.
    pub f0: f64,
    /// Inharmonicity coefficient.
    pub b: f64,
    pub partials: Vec<MeasuredPartial>,
    /// RMS of the fit residuals, in cents, over the partials actually used.
    pub rms_cents: f64,
    pub concerns: Vec<Concern>,
}

impl NoteMeasurement {
    /// Partials the fit accepted.
    pub fn used(&self) -> impl Iterator<Item = &MeasuredPartial> {
        self.partials.iter().filter(|p| p.used)
    }

    pub fn used_count(&self) -> usize {
        self.used().count()
    }

    /// Where this note's `n`th partial sits, according to the fitted model.
    pub fn predicted_partial(&self, n: u32) -> f64 {
        partial_hz(self.f0, self.b, n)
    }

    pub fn has(&self, c: Concern) -> bool {
        self.concerns.contains(&c)
    }
}

/// Tuning knobs for [`measure_note`].
#[derive(Clone, Copy, Debug)]
pub struct MeasureConfig {
    /// Highest partial number to look for.
    pub max_partial: u32,
    /// How many spectral peaks to consider.
    pub max_peaks: usize,
    /// How far the caller's `f0_hint` might be wrong. A piano can sit a long
    /// way flat, so this is generous by default.
    pub hint_tolerance_cents: f64,
    /// Fewer accepted partials than this and we decline to answer.
    pub min_partials: usize,
    /// Residual beyond which a partial is treated as an outlier.
    pub outlier_cents: f64,
    /// How far above the noise floor a peak must sit to count as a partial.
    /// Six is about 16 dB — comfortably clear of the noise without discarding
    /// the faint upper partials the bass depends on.
    pub noise_margin: f64,
    pub refine: RefineConfig,
}

impl Default for MeasureConfig {
    fn default() -> Self {
        Self {
            max_partial: 16,
            max_peaks: 40,
            hint_tolerance_cents: 150.0,
            min_partials: 3,
            outlier_cents: 12.0,
            noise_margin: 6.0,
            refine: RefineConfig::default(),
        }
    }
}

/// One partial's contribution to a fit.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub n: u32,
    pub hz: f64,
    /// Relative trust. Zero excludes the observation entirely.
    pub weight: f64,
}

/// The result of fitting the stiff-string model to a set of partials.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub f0: f64,
    pub b: f64,
    pub rms_cents: f64,
    /// Residual per input observation, in the order given.
    pub residuals_cents: Vec<f64>,
}

const CENTS_PER_LN: f64 = 1200.0 / std::f64::consts::LN_2;

/// Fit `f_n = n * f0 * sqrt(1 + B n^2)` to measured partials.
///
/// Two stages. A linearised solve gives a starting point: dividing each partial
/// by its number and squaring turns the model into a straight line, whose
/// intercept is the fundamental squared and whose slope over intercept is the
/// stiffness. That is quick and needs no initial guess, but it weights high
/// partials disproportionately.
///
/// Gauss-Newton then minimises the residuals in *cents*, which is the unit the
/// error actually matters in, reweighting as it goes so that one wild partial —
/// a false beat, or a misidentified peak — cannot drag the answer with it.
///
/// Returns `None` if there are fewer than three usable observations, or they
/// share a single partial number and so cannot determine two parameters.
pub fn fit_inharmonicity(obs: &[Observation]) -> Option<Fit> {
    let pts: Vec<&Observation> = obs
        .iter()
        .filter(|o| o.weight > 0.0 && o.hz > 0.0 && o.n >= 1)
        .collect();
    if pts.len() < 3 {
        return None;
    }

    // Linearised start: (f_n/n)^2 against n^2 is a straight line.
    let (mut f0, mut b) = {
        let xs: Vec<f64> = pts.iter().map(|o| f64::from(o.n * o.n)).collect();
        let ys: Vec<f64> = pts
            .iter()
            .map(|o| {
                let per = o.hz / f64::from(o.n);
                per * per
            })
            .collect();
        let m = xs.len() as f64;
        let mx = xs.iter().sum::<f64>() / m;
        let my = ys.iter().sum::<f64>() / m;
        let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
        if sxx <= 0.0 {
            return None; // every observation is the same partial number
        }
        let sxy: f64 = xs
            .iter()
            .zip(&ys)
            .map(|(x, y)| (x - mx) * (y - my))
            .sum();
        let slope = sxy / sxx;
        let intercept = my - slope * mx;
        if intercept <= 0.0 {
            return None;
        }
        // A negative slope means stiffness below zero, which is unphysical; a
        // noisy set of low partials can produce one. Start from zero instead.
        (intercept.sqrt(), (slope / intercept).max(0.0))
    };

    // Gauss-Newton on cents residuals, with Huber reweighting for robustness.
    let mut residuals = vec![0.0; pts.len()];
    for iteration in 0..12 {
        for (r, o) in residuals.iter_mut().zip(&pts) {
            *r = cents(partial_hz(f0, b, o.n), o.hz);
        }

        // Scale for the robust weights, from the median absolute residual.
        let mut abs: Vec<f64> = residuals.iter().map(|r| r.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let mad = abs[abs.len() / 2].max(1e-6);
        let scale = 1.4826 * mad;

        let (mut a11, mut a12, mut a22) = (0.0, 0.0, 0.0);
        let (mut g1, mut g2) = (0.0, 0.0);
        for (r, o) in residuals.iter().zip(&pts) {
            let nn = f64::from(o.n * o.n);
            // d(residual)/d(ln f0) and d(residual)/dB, both negative because a
            // larger prediction means a smaller residual.
            let j1 = -CENTS_PER_LN;
            let j2 = -CENTS_PER_LN * 0.5 * nn / (1.0 + b * nn);

            // Huber: quadratic near zero, linear in the tails, so a single wild
            // partial pulls with bounded force instead of dominating.
            let u = (r / scale).abs();
            let robust = if u <= 1.5 { 1.0 } else { 1.5 / u };
            let w = o.weight * robust;

            a11 += w * j1 * j1;
            a12 += w * j1 * j2;
            a22 += w * j2 * j2;
            g1 -= w * j1 * r;
            g2 -= w * j2 * r;
        }

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-12 {
            break;
        }
        let d_ln_f0 = (g1 * a22 - g2 * a12) / det;
        let d_b = (a11 * g2 - a12 * g1) / det;

        f0 *= d_ln_f0.exp();
        b = (b + d_b).max(0.0);

        if d_ln_f0.abs() < 1e-12 && d_b.abs() < 1e-12 {
            break;
        }
        let _ = iteration;
    }

    // Final residuals, reported against every observation the caller gave us.
    let residuals_cents: Vec<f64> = obs
        .iter()
        .map(|o| {
            if o.hz > 0.0 && o.n >= 1 {
                cents(partial_hz(f0, b, o.n), o.hz)
            } else {
                f64::NAN
            }
        })
        .collect();

    let mut sse = 0.0;
    let mut wsum = 0.0;
    for (o, r) in obs.iter().zip(&residuals_cents) {
        if o.weight > 0.0 && r.is_finite() {
            sse += o.weight * r * r;
            wsum += o.weight;
        }
    }

    Some(Fit {
        f0,
        b,
        rms_cents: if wsum > 0.0 { (sse / wsum).sqrt() } else { 0.0 },
        residuals_cents,
    })
}

/// Peak nearest `target_hz`, within `tolerance_hz`, preferring louder peaks when
/// several are close.
fn nearest_peak(peaks: &[Peak], target_hz: f64, tolerance_hz: f64) -> Option<&Peak> {
    peaks
        .iter()
        .filter(|p| (p.hz - target_hz).abs() <= tolerance_hz)
        .max_by(|a, c| {
            // Score louder and closer peaks above quieter, further ones.
            let score = |p: &Peak| p.amplitude * (1.0 - (p.hz - target_hz).abs() / tolerance_hz);
            score(a).total_cmp(&score(c))
        })
}

/// Match peaks to partial numbers, given a fundamental and stiffness.
fn assign(
    peaks: &[Peak],
    f0: f64,
    b: f64,
    up_to: u32,
    tolerance_hz: f64,
) -> Vec<(u32, Peak)> {
    let mut out = Vec::new();
    for n in 1..=up_to {
        let target = partial_hz(f0, b, n);
        if let Some(p) = nearest_peak(peaks, target, tolerance_hz) {
            out.push((n, *p));
        }
    }
    out
}

/// How good an assignment is: loud partials matched closely are worth most.
fn assignment_score(matches: &[(u32, Peak)], f0: f64, b: f64, tolerance_hz: f64) -> f64 {
    matches
        .iter()
        .map(|(n, p)| {
            let target = partial_hz(f0, b, *n);
            let closeness = 1.0 - (p.hz - target).abs() / tolerance_hz;
            p.amplitude * closeness.max(0.0)
        })
        .sum()
}

/// Measure one struck note.
///
/// `f0_hint` is where the fundamental is expected — from the key the technician
/// selected, or the tuning sequence. It may be off by
/// [`MeasureConfig::hint_tolerance_cents`]; the returned `f0` is determined by
/// the partials themselves and is far more accurate than the hint.
///
/// Returns `None` if too few partials could be found to say anything honest.
pub fn measure_note(
    samples: &[f32],
    sample_rate: f64,
    f0_hint: f64,
    cfg: MeasureConfig,
) -> Option<NoteMeasurement> {
    if f0_hint <= 0.0 {
        return None;
    }
    // Look below the fundamental too, so its absence is a finding rather than an
    // artefact of where we started looking.
    let min_hz = (f0_hint * 0.55).max(18.0);
    let all_peaks = coarse_peaks(samples, sample_rate, min_hz, cfg.max_peaks);
    if all_peaks.is_empty() {
        return None;
    }

    // Discard anything indistinguishable from noise before assignment. A note
    // rarely has as many partials as we look for, and without this the matcher
    // pairs the missing ones with whatever noise sits nearest — leaving the
    // fitter to reject them, which reaches the same answer more slowly and then
    // complains about partials that were never there.
    let threshold = crate::estimate::noise_floor(samples) * cfg.noise_margin;
    let peaks: Vec<Peak> = all_peaks
        .into_iter()
        .filter(|p| p.amplitude > threshold)
        .collect();
    if peaks.is_empty() {
        return None;
    }

    // Half the spacing between neighbouring partials is the most we can allow
    // before an assignment becomes ambiguous.
    let tolerance_hz = 0.4 * f0_hint;

    // Search plausible fundamentals and stiffnesses together. Only the low
    // partials take part: they are where a wrong stiffness moves predictions
    // least, so the search cannot talk itself into a wrong numbering.
    let anchor_partials = 6.min(cfg.max_partial);
    let mut best: Option<(f64, f64, f64)> = None; // (score, f0, b)
    let steps = (cfg.hint_tolerance_cents / 6.0).ceil() as i32;
    for i in -steps..=steps {
        let f0 = f0_hint * 2f64.powf(f64::from(i) * 6.0 / 1200.0);
        for j in 0..48 {
            // Log grid from 1e-5 to 3e-3, spanning grands to spinets, plus zero.
            let b = if j == 0 {
                0.0
            } else {
                1e-5 * (300f64).powf(f64::from(j - 1) / 46.0)
            };
            let matches = assign(&peaks, f0, b, anchor_partials, tolerance_hz);
            if matches.len() < 2 {
                continue;
            }
            let score = assignment_score(&matches, f0, b, tolerance_hz);
            if best.is_none_or(|(s, _, _)| score > s) {
                best = Some((score, f0, b));
            }
        }
    }
    let (_, mut f0, mut b) = best?;

    // Sharpen the anchor before extending upward: a better fundamental and
    // stiffness make the high partial predictions trustworthy enough to match.
    let anchor = assign(&peaks, f0, b, anchor_partials, tolerance_hz);
    let anchor_obs: Vec<Observation> = anchor
        .iter()
        .map(|(n, p)| Observation {
            n: *n,
            hz: p.hz,
            weight: p.amplitude,
        })
        .collect();
    if let Some(fit) = fit_inharmonicity(&anchor_obs) {
        f0 = fit.f0;
        b = fit.b;
    }

    // Now take every partial we can reach, and measure each one properly.
    let mut partials: Vec<MeasuredPartial> = Vec::new();
    for (n, peak) in assign(&peaks, f0, b, cfg.max_partial, tolerance_hz) {
        let (hz, amplitude, confidence) =
            match refine(samples, sample_rate, peak.hz, cfg.refine) {
                Some(r) => (r.hz, r.amplitude, r.confidence),
                // Too short to refine: the coarse position still beats nothing.
                None => (peak.hz, peak.amplitude, 0.3),
            };
        partials.push(MeasuredPartial {
            n,
            hz,
            amplitude,
            confidence,
            residual_cents: 0.0,
            used: true,
        });
    }
    if partials.len() < cfg.min_partials {
        return None;
    }

    // Weight by confidence: a partial whose phase would not hold still says so,
    // and should not be allowed to set the answer for the ones that did.
    let observations: Vec<Observation> = partials
        .iter()
        .map(|p| Observation {
            n: p.n,
            hz: p.hz,
            weight: (0.05 + p.confidence).min(1.0),
        })
        .collect();
    let fit = fit_inharmonicity(&observations)?;

    for (p, r) in partials.iter_mut().zip(&fit.residuals_cents) {
        p.residual_cents = *r;
        p.used = r.abs() <= cfg.outlier_cents;
    }

    // Refit without the outliers, so they do not colour the final answer.
    let rejected = partials.iter().filter(|p| !p.used).count();
    let final_fit = if rejected > 0 {
        let kept: Vec<Observation> = partials
            .iter()
            .filter(|p| p.used)
            .map(|p| Observation {
                n: p.n,
                hz: p.hz,
                weight: (0.05 + p.confidence).min(1.0),
            })
            .collect();
        match fit_inharmonicity(&kept) {
            Some(refit) => {
                for p in partials.iter_mut() {
                    p.residual_cents = cents(partial_hz(refit.f0, refit.b, p.n), p.hz);
                }
                refit
            }
            // Not enough left to refit; the first answer stands.
            None => fit,
        }
    } else {
        fit
    };

    let used: Vec<&MeasuredPartial> = partials.iter().filter(|p| p.used).collect();
    if used.len() < cfg.min_partials {
        return None;
    }

    let mut concerns = Vec::new();
    if !partials.iter().any(|p| p.n == 1) {
        concerns.push(Concern::FundamentalMissing);
    }
    if used.len() < 4 {
        concerns.push(Concern::FewPartials);
    }
    if final_fit.rms_cents > 3.0 {
        concerns.push(Concern::PoorFit);
    }
    if used.iter().filter(|p| p.confidence < 0.6).count() * 2 >= used.len() {
        concerns.push(Concern::UnstablePartials);
    }
    if rejected > 0 {
        concerns.push(Concern::PartialsRejected);
    }

    Some(NoteMeasurement {
        f0: final_fit.f0,
        b: final_fit.b,
        partials,
        rms_cents: final_fit.rms_cents,
        concerns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{render, StringSpec, ToneSpec};

    const SR: f64 = 48_000.0;

    fn tone(strings: Vec<StringSpec>, secs: f64, noise_dbfs: Option<f64>) -> Vec<f32> {
        render(&ToneSpec {
            strings,
            sample_rate: SR,
            duration: secs,
            noise_dbfs,
            seed: 0x1A_2B_3C_4D,
            clip: None,
        })
    }

    fn exact_observations(f0: f64, b: f64, ns: &[u32]) -> Vec<Observation> {
        ns.iter()
            .map(|&n| Observation {
                n,
                hz: partial_hz(f0, b, n),
                weight: 1.0,
            })
            .collect()
    }

    #[test]
    fn fit_recovers_exactly_what_generated_the_partials() {
        for &(f0, b) in &[
            (27.5, 1.2e-3),   // A0 on a spinet
            (110.0, 8.0e-5),  // A2
            (220.63, 3.04e-4),
            (441.92, 7.33e-4),
            (3520.0, 4.0e-3), // top of the treble
        ] {
            let obs = exact_observations(f0, b, &[1, 2, 3, 4, 5, 6, 7, 8]);
            let fit = fit_inharmonicity(&obs).expect("fit failed");
            assert!(
                cents(f0, fit.f0).abs() < 1e-4,
                "f0 {f0}: got {}, {:.6} cents off",
                fit.f0,
                cents(f0, fit.f0)
            );
            assert!(
                (fit.b - b).abs() / b < 1e-4,
                "B {b:.3e}: got {:.6e}",
                fit.b
            );
            assert!(fit.rms_cents < 1e-4, "rms {:.6} cents", fit.rms_cents);
        }
    }

    #[test]
    fn fit_works_without_the_low_partials() {
        // The bass case: the fundamental and second partial are simply absent,
        // as phase 0a confirmed on the real A0. The fundamental has to be
        // inferred from partials four and up.
        let (f0, b) = (27.5, 1.2e-3);
        let obs = exact_observations(f0, b, &[3, 4, 5, 6, 7, 8, 9, 10]);
        let fit = fit_inharmonicity(&obs).expect("fit failed");
        assert!(
            cents(f0, fit.f0).abs() < 0.01,
            "inferred f0 {} is {:.4} cents off {f0}",
            fit.f0,
            cents(f0, fit.f0)
        );
        assert!((fit.b - b).abs() / b < 0.001);
    }

    #[test]
    fn one_wild_partial_does_not_drag_the_fit() {
        // A false beat, or a peak that belongs to something else entirely.
        let (f0, b) = (220.63, 3.04e-4);
        let mut obs = exact_observations(f0, b, &[1, 2, 3, 4, 5, 6, 7, 8]);
        obs[4].hz *= 1.01; // 17 cents out

        let fit = fit_inharmonicity(&obs).expect("fit failed");
        assert!(
            cents(f0, fit.f0).abs() < 0.5,
            "one bad partial moved f0 by {:.4} cents",
            cents(f0, fit.f0)
        );
        assert!(
            (fit.b - b).abs() / b < 0.05,
            "one bad partial moved B by {:.1}%",
            100.0 * (fit.b - b) / b
        );
        // And it should stand out plainly in the residuals.
        assert!(
            fit.residuals_cents[4].abs() > 10.0,
            "the bad partial should be conspicuous, residual was {:.2}",
            fit.residuals_cents[4]
        );
    }

    #[test]
    fn fit_declines_when_it_cannot_answer() {
        assert!(fit_inharmonicity(&[]).is_none());
        assert!(fit_inharmonicity(&exact_observations(220.0, 1e-4, &[1, 2])).is_none());
        // Three readings of the same partial cannot determine two parameters.
        let same = exact_observations(220.0, 1e-4, &[3, 3, 3]);
        assert!(fit_inharmonicity(&same).is_none());
    }

    #[test]
    fn measures_a_synthetic_note_end_to_end() {
        // A3 as measured on the owner's real piano.
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(10).with_amp(0.4)],
            1.5,
            Some(-60.0),
        );
        let m = measure_note(&x, SR, f0, MeasureConfig::default()).expect("no measurement");

        assert!(
            cents(f0, m.f0).abs() < 0.05,
            "f0 {:.4} is {:.4} cents off",
            m.f0,
            cents(f0, m.f0)
        );
        assert!(
            (m.b - b).abs() / b < 0.02,
            "B {:.4e} vs {b:.4e}, {:.2}% off",
            m.b,
            100.0 * (m.b - b) / b
        );
        assert!(m.used_count() >= 6, "only {} partials used", m.used_count());
        assert!(m.rms_cents < 0.5, "rms {:.4} cents", m.rms_cents);
    }

    #[test]
    fn a_wrong_hint_is_corrected_by_the_partials() {
        // The piano sits a long way flat, so the key pressed is not where the
        // note actually is. The hint only has to get us to the right partials.
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(10).with_amp(0.4)],
            1.5,
            Some(-60.0),
        );
        for hint_error_cents in [-120.0, -60.0, -20.0, 20.0, 60.0, 120.0] {
            let hint = f0 * 2f64.powf(hint_error_cents / 1200.0);
            let m = measure_note(&x, SR, hint, MeasureConfig::default())
                .unwrap_or_else(|| panic!("no measurement with hint {hint_error_cents} cents off"));
            assert!(
                cents(f0, m.f0).abs() < 0.1,
                "hint {hint_error_cents} cents off gave f0 {:.4}, {:.4} cents out",
                m.f0,
                cents(f0, m.f0)
            );
        }
    }

    #[test]
    fn measures_a_bass_note_whose_fundamental_is_missing() {
        // Reproduces what the phone actually saw on A0: no fundamental, no
        // second partial, the fifth the strongest thing present.
        let (f0, b) = (27.5, 1.1e-3);
        let mut s = StringSpec::new(f0, b);
        s.partials = 14;
        s.amp = 0.5;
        s.rolloff = -1.2; // upper partials louder than the fundamental
        s.t60 = 12.0;
        let full = tone(vec![s], 2.0, Some(-70.0));

        // Roll off everything below 70 Hz the way a phone microphone does, by
        // subtracting the low partials back out.
        let mut x = full;
        for n in 1..=2u32 {
            let hz = partial_hz(f0, b, n);
            let mut low = StringSpec::new(f0, b);
            low.partials = 14;
            low.amp = 0.5;
            low.rolloff = -1.2;
            low.t60 = 12.0;
            let only_this = {
                let mut one = low.clone();
                one.f0 = hz;
                one.b = 0.0;
                one.partials = 1;
                one.rolloff = 0.0;
                one.amp = 0.5 * f64::from(n).powf(1.2);
                tone(vec![one], 2.0, None)
            };
            for (a, b2) in x.iter_mut().zip(&only_this) {
                *a -= *b2;
            }
        }

        let m = measure_note(&x, SR, f0, MeasureConfig::default()).expect("no measurement");
        assert!(
            cents(f0, m.f0).abs() < 1.0,
            "inferred f0 {:.4} is {:.3} cents off {f0}",
            m.f0,
            cents(f0, m.f0)
        );
        assert!(
            (m.b - b).abs() / b < 0.10,
            "B {:.4e} vs {b:.4e}",
            m.b
        );
    }

    #[test]
    fn a_beating_unison_is_flagged_rather_than_trusted_silently() {
        // Two strings six cents apart, which is a rough unison but not a rare
        // one on a neglected piano.
        let (f0, b) = (220.63, 3.04e-4);
        let a = StringSpec::new(f0, b).with_partials(8).with_amp(0.35);
        let second = a.detuned(6.0);
        let x = tone(vec![a, second], 2.0, Some(-65.0));

        let m = measure_note(&x, SR, f0, MeasureConfig::default()).expect("no measurement");
        let shaky = m.used().filter(|p| p.confidence < 0.8).count();
        assert!(
            shaky > 0,
            "a beating unison should shake at least one partial's confidence"
        );
    }

    #[test]
    fn a_clean_note_raises_no_concerns() {
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(10).with_amp(0.4)],
            1.5,
            Some(-70.0),
        );
        let m = measure_note(&x, SR, f0, MeasureConfig::default()).unwrap();
        assert!(
            m.concerns.is_empty(),
            "clean note raised concerns: {:?}",
            m.concerns
        );
    }

    #[test]
    fn the_treble_manages_on_very_few_partials() {
        // High notes give three or four usable partials at best, and they die
        // fast. The measurement must still be honest about what it got.
        let (f0, b) = (2093.0, 2.5e-3); // C7
        let mut s = StringSpec::new(f0, b);
        s.partials = 4;
        s.amp = 0.4;
        s.t60 = 1.5;
        let x = tone(vec![s], 1.2, Some(-70.0));

        let m = measure_note(&x, SR, f0, MeasureConfig::default()).expect("no measurement");
        assert!(
            cents(f0, m.f0).abs() < 0.5,
            "treble f0 {:.3} is {:.3} cents off",
            m.f0,
            cents(f0, m.f0)
        );
        assert!(m.used_count() >= 3);
    }

    #[test]
    fn predicted_partials_match_what_was_measured() {
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(8).with_amp(0.4)],
            1.5,
            None,
        );
        let m = measure_note(&x, SR, f0, MeasureConfig::default()).unwrap();
        for p in m.used() {
            let predicted = m.predicted_partial(p.n);
            assert!(
                cents(predicted, p.hz).abs() < 1.0,
                "partial {} predicted {predicted:.3}, measured {:.3}",
                p.n,
                p.hz
            );
        }
    }
}
