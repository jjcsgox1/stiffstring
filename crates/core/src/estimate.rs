//! Precision frequency estimation.
//!
//! # Why not a pitch detector
//!
//! YIN, autocorrelation and every general-purpose pitch detector work by finding
//! a single repeating period, which assumes partials sit at exact integer
//! multiples of the fundamental. That assumption is precisely what is false
//! about a stiff piano string: they return a biased answer *and* throw away the
//! partial structure this project is built on. Nothing here estimates a period.
//!
//! # Where the precision comes from
//!
//! We need roughly 0.1 cent, about 0.025 Hz at A4. FFT bins are 1.46 Hz. No
//! window function closes a gap that wide.
//!
//! The resolution limit does not apply, because we are not *resolving* two
//! nearby tones — we are *estimating* the frequency of one well-isolated
//! sinusoid, and piano partials sit far apart in their own neighbourhoods.
//! Precision then comes from observing for a long time, not from a bigger
//! transform.
//!
//! Concretely: a sinusoid's phase advances at a constant rate. Measure that
//! phase at several instants spread across a second, and the *slope* of the
//! line through those measurements is the frequency, to a precision limited by
//! how well each phase is measured rather than by bin width. The FFT's only job
//! is to say roughly where to look and to isolate one partial from its
//! neighbours.
//!
//! Two properties make this work on real piano notes. A decaying amplitude does
//! not disturb phase, so the note dying away is harmless. And a partial sitting
//! between two bins adds a constant phase offset, identical in every frame,
//! which shifts the line's intercept and leaves its slope alone.
//!
//! # Bad data announces itself
//!
//! If the phase does not advance linearly, something is wrong: two strings
//! beating, a false beat on one string, or noise swamping the partial. The
//! residual from the straight-line fit therefore doubles as a quality measure,
//! which matters on the neglected pianos this tool is aimed at.

use crate::fft::{amplitudes, bin_hz, parabolic_peak, spectrum, window, Window};
use std::f64::consts::{PI, TAU};

/// A partial located roughly, by looking at the magnitude spectrum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub hz: f64,
    /// Linear amplitude, where 1.0 is a full-scale sinusoid.
    pub amplitude: f64,
}

/// A partial pinned down precisely, by watching its phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Refined {
    pub hz: f64,
    pub amplitude: f64,
    /// RMS deviation from a straight-line phase fit, in radians. Small means one
    /// clean sinusoid; large means beating, a false beat, or noise.
    pub phase_residual: f64,
    /// `phase_residual` mapped to 0..1 for convenience. Heuristic.
    pub confidence: f64,
}

/// How to spend the available signal when refining.
#[derive(Clone, Copy, Debug)]
pub struct RefineConfig {
    /// Frame length, a power of two. Longer isolates neighbouring partials
    /// better; shorter leaves more room for frames to spread out in time.
    pub frame_len: usize,
    /// How many phase measurements to fit the line through.
    pub frames: usize,
    pub window: Window,
}

impl Default for RefineConfig {
    fn default() -> Self {
        Self {
            frame_len: 8192,
            frames: 8,
            window: Window::BlackmanHarris,
        }
    }
}

/// Wrap an angle into (-pi, pi].
#[inline]
fn wrap(mut a: f64) -> f64 {
    while a > PI {
        a -= TAU;
    }
    while a <= -PI {
        a += TAU;
    }
    a
}

/// Largest power of two not exceeding `n`.
#[inline]
fn floor_pow2(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1usize << (usize::BITS - 1 - n.leading_zeros()) as usize
    }
}

/// Locate partials from the magnitude spectrum, strongest first.
///
/// Sub-bin position comes from fitting a parabola through the peak and its two
/// neighbours in the log-magnitude domain. That is good to a few hundredths of a
/// hertz — nowhere near enough on its own, but ample as a starting point for
/// [`refine`], which needs only to be told which bin to watch.
pub fn coarse_peaks(
    samples: &[f32],
    sample_rate: f64,
    min_hz: f64,
    max_peaks: usize,
) -> Vec<Peak> {
    let n = floor_pow2(samples.len());
    if n < 64 || max_peaks == 0 {
        return Vec::new();
    }
    let w = window(Window::BlackmanHarris, n);
    let amps = amplitudes(&spectrum(&samples[..n], &w), &w);
    let bin = bin_hz(sample_rate, n);

    let first = ((min_hz / bin).ceil() as usize).max(2);
    let mut peaks: Vec<Peak> = Vec::new();

    for i in first..amps.len().saturating_sub(1) {
        if !(amps[i] > amps[i - 1] && amps[i] >= amps[i + 1]) {
            continue;
        }
        let (offset, amplitude) = parabolic_peak(amps[i - 1], amps[i], amps[i + 1]);
        peaks.push(Peak {
            hz: (i as f64 + offset) * bin,
            amplitude,
        });
    }

    peaks.sort_by(|a, b| b.amplitude.total_cmp(&a.amplitude));
    peaks.truncate(max_peaks);
    peaks
}

/// A robust estimate of the noise floor, as a per-bin amplitude.
///
/// The median works because the overwhelming majority of bins in any real
/// spectrum hold nothing but noise — a piano note occupies a few dozen bins out
/// of thousands — so the middle of the distribution is the floor, unmoved by how
/// loud the note itself is.
///
/// Callers use it to decide what is worth calling a partial. Without a threshold
/// like this, a peak finder will happily return the loudest bumps in the noise
/// and a fitter will then have to reject them, which is a slower and less honest
/// way of reaching the same conclusion.
pub fn noise_floor(samples: &[f32]) -> f64 {
    let n = floor_pow2(samples.len());
    if n < 64 {
        return 0.0;
    }
    let w = window(Window::BlackmanHarris, n);
    let mut amps = amplitudes(&spectrum(&samples[..n], &w), &w);
    amps.sort_by(f64::total_cmp);
    amps[amps.len() / 2]
}

/// Pin down one partial near `approx_hz` by fitting a line to its phase.
///
/// `approx_hz` must be within half the wrap limit of the truth — see
/// [`wrap_limit_hz`]. A [`coarse_peaks`] estimate is comfortably inside it.
///
/// Returns `None` if the signal is too short for the configuration, or the
/// requested frequency falls outside the spectrum.
pub fn refine(
    samples: &[f32],
    sample_rate: f64,
    approx_hz: f64,
    cfg: RefineConfig,
) -> Option<Refined> {
    let n = cfg.frame_len;
    if !n.is_power_of_two() || cfg.frames < 2 || samples.len() < n {
        return None;
    }
    let bin = bin_hz(sample_rate, n);
    let k = (approx_hz / bin).round() as usize;
    if k < 1 || k >= n / 2 {
        return None;
    }

    let hop = (samples.len() - n) / (cfg.frames - 1);
    if hop == 0 {
        return None;
    }
    let w = window(cfg.window, n);
    let scale = 2.0 / (n as f64 * crate::fft::coherent_gain(&w));

    // Measure phase at each frame, then unwrap against where a sinusoid of
    // exactly `approx_hz` would have been. What remains is a straight line whose
    // slope is the frequency error.
    let mut times = Vec::with_capacity(cfg.frames);
    let mut residuals = Vec::with_capacity(cfg.frames);
    let mut frame_amps = Vec::with_capacity(cfg.frames);
    let mut amplitude_sum = 0.0;
    let mut previous = 0.0;

    for j in 0..cfg.frames {
        let start = j * hop;
        let frame = &samples[start..start + n];
        let spec = spectrum(frame, &w);
        let c = spec[k];

        // Interpolate across the peak rather than trusting one bin: a partial
        // between bins under-reads by up to 0.8 dB otherwise, making a partial's
        // apparent strength depend on where it happened to fall.
        let amp = if k >= 1 && k + 1 < n / 2 {
            parabolic_peak(
                spec[k - 1].abs() * scale,
                c.abs() * scale,
                spec[k + 1].abs() * scale,
            )
            .1
        } else {
            c.abs() * scale
        };
        amplitude_sum += amp;
        frame_amps.push(amp);

        let t = start as f64 / sample_rate;
        let expected = TAU * approx_hz * t;
        let measured = c.arg();

        let r = if j == 0 {
            wrap(measured - expected)
        } else {
            // Continue the line rather than wrapping independently, so a steady
            // drift accumulates instead of folding back on itself.
            previous + wrap(measured - expected - previous)
        };
        previous = r;
        times.push(t);
        residuals.push(r);
    }

    // Least squares through (t, phase residual), weighted by each frame's
    // amplitude squared.
    //
    // This matters most in the treble, where partials die within half a second:
    // the later frames are then reading phase out of noise, and weighting every
    // frame alike would let those measurements degrade a fit the early frames
    // had already settled. Phase variance goes as the inverse square of
    // amplitude, so amplitude squared is the statistically right weight, and it
    // shortens the effective observation window exactly as far as the note's own
    // decay demands — no register-specific special casing needed.
    let weights: Vec<f64> = frame_amps.iter().map(|a| a * a).collect();
    let wsum: f64 = weights.iter().sum();
    if wsum <= 0.0 {
        return None;
    }
    let mean_t = times.iter().zip(&weights).map(|(t, w)| t * w).sum::<f64>() / wsum;
    let mean_r = residuals.iter().zip(&weights).map(|(r, w)| r * w).sum::<f64>() / wsum;

    let mut sxy = 0.0;
    let mut sxx = 0.0;
    for ((t, r), w) in times.iter().zip(&residuals).zip(&weights) {
        sxy += w * (t - mean_t) * (r - mean_r);
        sxx += w * (t - mean_t) * (t - mean_t);
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let intercept = mean_r - slope * mean_t;

    let mut sse = 0.0;
    for ((t, r), w) in times.iter().zip(&residuals).zip(&weights) {
        let e = r - (intercept + slope * t);
        sse += w * e * e;
    }
    let phase_residual = (sse / wsum).sqrt();

    Some(Refined {
        hz: approx_hz + slope / TAU,
        amplitude: amplitude_sum / cfg.frames as f64,
        phase_residual,
        // Heuristic: a clean sinusoid sits near zero; a quarter radian of wobble
        // means something else is going on in that bin.
        confidence: (1.0 - phase_residual / 0.25).clamp(0.0, 1.0),
        })
}

/// How far `approx_hz` may be from the truth before phase unwrapping becomes
/// ambiguous, for a given configuration and signal length.
pub fn wrap_limit_hz(sample_rate: f64, samples: usize, cfg: RefineConfig) -> f64 {
    if cfg.frames < 2 || samples < cfg.frame_len {
        return 0.0;
    }
    let hop = (samples - cfg.frame_len) / (cfg.frames - 1);
    if hop == 0 {
        return 0.0;
    }
    sample_rate / (2.0 * hop as f64)
}

/// Difference between two frequencies in cents.
#[inline]
pub fn cents(from_hz: f64, to_hz: f64) -> f64 {
    1200.0 * (to_hz / from_hz).log2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{render, rms, StringSpec, ToneSpec};

    const SR: f64 = 48_000.0;

    /// A sinusoid that does not decay, for measuring the estimator's precision
    /// floor without a moving target.
    fn steady(hz: f64, amp: f64) -> StringSpec {
        StringSpec {
            f0: hz,
            b: 0.0,
            amp,
            partials: 1,
            rolloff: 0.0,
            t60: 1e9,
            decay_exp: 0.0,
            phase: 0.4,
        }
    }

    fn tone(strings: Vec<StringSpec>, secs: f64, noise_dbfs: Option<f64>) -> Vec<f32> {
        let spec = ToneSpec {
            strings,
            sample_rate: SR,
            duration: secs,
            noise_dbfs,
            seed: 0x51FF_5717,
            clip: None,
        };
        render(&spec)
    }

    /// Noise level giving the requested SNR against a sinusoid of amplitude `amp`.
    fn noise_for_snr(amp: f64, snr_db: f64) -> f64 {
        let signal_rms = amp / 2f64.sqrt();
        20.0 * signal_rms.log10() - snr_db
    }

    #[test]
    fn recovers_a_clean_tone_to_far_better_than_a_hundredth_of_a_cent() {
        // The phase-2 gate: better than 0.02 cents on clean synthetic signal.
        // Deliberately awkward frequencies, none of them near a bin centre.
        for &hz in &[27.53, 110.31, 220.61, 441.93, 1329.87, 3517.4] {
            let x = tone(vec![steady(hz, 0.5)], 1.0, None);
            let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
            let r = refine(&x, SR, coarse.hz, RefineConfig::default()).unwrap();

            let err = cents(hz, r.hz).abs();
            assert!(
                err < 0.02,
                "{hz} Hz: refined to {:.6} Hz, {err:.5} cents off (coarse was {:.4})",
                r.hz,
                cents(hz, coarse.hz).abs()
            );
        }
    }

    #[test]
    fn the_fft_alone_is_nowhere_near_good_enough() {
        // Establishes the problem the phase step exists to solve: the coarse
        // parabolic estimate is far too crude, and refining it is what closes
        // the gap. If this ever stops being true, the phase step is redundant.
        let hz = 441.93;
        let x = tone(vec![steady(hz, 0.5)], 1.0, None);
        let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
        let refined = refine(&x, SR, coarse.hz, RefineConfig::default()).unwrap();

        let coarse_err = cents(hz, coarse.hz).abs();
        let refined_err = cents(hz, refined.hz).abs();
        assert!(
            refined_err * 20.0 < coarse_err,
            "refining should improve on the FFT by more than 20x: \
             coarse {coarse_err:.5} cents, refined {refined_err:.5} cents"
        );
    }

    #[test]
    fn holds_up_at_twenty_decibels_of_noise() {
        // The phase-2 gate under realistic conditions: better than 0.1 cents.
        // A 20 dB broadband SNR is far better than it sounds inside one bin —
        // the transform concentrates the tone and spreads the noise.
        let amp = 0.5;
        for &hz in &[110.31, 441.93, 1329.87] {
            let x = tone(
                vec![steady(hz, amp)],
                1.0,
                Some(noise_for_snr(amp, 20.0)),
            );
            let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
            let r = refine(&x, SR, coarse.hz, RefineConfig::default()).unwrap();

            let err = cents(hz, r.hz).abs();
            assert!(err < 0.1, "{hz} Hz at 20 dB SNR: {err:.4} cents off");
        }
    }

    #[test]
    fn survives_a_punishing_noise_floor() {
        // Not a gate, just a record of where it starts to break down.
        let amp = 0.5;
        let hz = 441.93;
        let x = tone(vec![steady(hz, amp)], 1.0, Some(noise_for_snr(amp, 0.0)));
        let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
        let r = refine(&x, SR, coarse.hz, RefineConfig::default()).unwrap();
        assert!(
            cents(hz, r.hz).abs() < 1.0,
            "at 0 dB SNR: {:.4} cents off",
            cents(hz, r.hz).abs()
        );
    }

    #[test]
    fn a_decaying_note_is_still_measured_accurately() {
        // Amplitude falls away throughout, which must not disturb the phase
        // slope. This is the real working case.
        let hz = 220.61;
        let s = StringSpec {
            f0: hz,
            b: 0.0,
            amp: 0.5,
            partials: 1,
            rolloff: 0.0,
            t60: 3.0,
            decay_exp: 0.0,
            phase: 0.9,
        };
        let x = tone(vec![s], 1.5, None);
        let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
        let r = refine(&x, SR, coarse.hz, RefineConfig::default()).unwrap();
        assert!(
            cents(hz, r.hz).abs() < 0.05,
            "decaying note: {:.4} cents off",
            cents(hz, r.hz).abs()
        );
    }

    #[test]
    fn finds_every_partial_of_a_synthetic_piano_note() {
        // A3 as measured on the real piano in phase 0a.
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(8).with_amp(0.4)],
            1.5,
            Some(-60.0),
        );
        let peaks = coarse_peaks(&x, SR, 60.0, 10);

        for n in 1..=6u32 {
            let want = crate::synth::partial_hz(f0, b, n);
            let found = peaks
                .iter()
                .min_by(|a, c| {
                    (a.hz - want)
                        .abs()
                        .total_cmp(&(c.hz - want).abs())
                })
                .copied()
                .expect("no peaks at all");
            assert!(
                (found.hz - want).abs() < 2.0,
                "partial {n} expected near {want:.2} Hz, closest peak {:.2}",
                found.hz
            );

            let r = refine(&x, SR, found.hz, RefineConfig::default()).unwrap();
            let err = cents(want, r.hz).abs();
            assert!(err < 0.2, "partial {n} refined {err:.4} cents off {want:.3} Hz");
        }
    }

    #[test]
    fn inharmonicity_is_recoverable_from_refined_partials() {
        // The payoff: measure partials precisely enough that B falls out of them.
        let (f0, b) = (220.63, 3.04e-4);
        let x = tone(
            vec![StringSpec::new(f0, b).with_partials(8).with_amp(0.4)],
            1.5,
            None,
        );

        // Linearised fit: (f_n/n)^2 against n^2 is a straight line whose slope
        // over intercept is B. Phase 3 will do this properly, with weighting and
        // outlier rejection; this only shows the measurements support it.
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for n in 1..=6u32 {
            let want = crate::synth::partial_hz(f0, b, n);
            let r = refine(&x, SR, want, RefineConfig::default()).unwrap();
            let per = r.hz / f64::from(n);
            xs.push(f64::from(n * n));
            ys.push(per * per);
        }
        let m = xs.len() as f64;
        let mx = xs.iter().sum::<f64>() / m;
        let my = ys.iter().sum::<f64>() / m;
        let sxy: f64 = xs.iter().zip(&ys).map(|(a, c)| (a - mx) * (c - my)).sum();
        let sxx: f64 = xs.iter().map(|a| (a - mx) * (a - mx)).sum();
        let slope = sxy / sxx;
        let intercept = my - slope * mx;

        let got_b = slope / intercept;
        let got_f0 = intercept.sqrt();
        assert!(
            (got_b - b).abs() / b < 0.02,
            "B recovered as {got_b:.3e}, want {b:.3e}"
        );
        assert!(
            cents(f0, got_f0).abs() < 0.1,
            "f0 recovered as {got_f0:.4}, want {f0}"
        );
    }

    #[test]
    fn a_clean_partial_reports_high_confidence() {
        let x = tone(vec![steady(441.93, 0.5)], 1.0, None);
        let r = refine(&x, SR, 441.93, RefineConfig::default()).unwrap();
        assert!(
            r.confidence > 0.95,
            "clean tone should be trusted: confidence {:.3}, residual {:.4} rad",
            r.confidence,
            r.phase_residual
        );
    }

    #[test]
    fn a_beating_pair_reports_low_confidence() {
        // Two strings a few cents apart. The phase stops advancing linearly, and
        // the fit residual is what tells us so. This is the mechanism behind
        // false-beat and rough-unison detection on neglected pianos.
        let a = steady(220.61, 0.5);
        let b = StringSpec {
            f0: 220.61 * 2f64.powf(6.0 / 1200.0),
            ..a.clone()
        };
        let x = tone(vec![a, b], 2.0, None);
        let r = refine(&x, SR, 220.61, RefineConfig::default()).unwrap();
        assert!(
            r.phase_residual > 0.1,
            "beating pair should not fit a straight line: residual {:.4} rad",
            r.phase_residual
        );
        assert!(
            r.confidence < 0.7,
            "beating pair should not be trusted: confidence {:.3}",
            r.confidence
        );
    }

    #[test]
    fn the_coarse_estimate_stays_inside_the_unwrapping_limit() {
        // refine() can only unwrap phase if it is told roughly the right answer.
        // This checks the margin is generous rather than lucky.
        let cfg = RefineConfig::default();
        let limit = wrap_limit_hz(SR, 48_000, cfg);
        assert!(limit > 4.0, "unwrapping limit unexpectedly tight: {limit} Hz");

        for &hz in &[110.31, 441.93, 1329.87] {
            let x = tone(vec![steady(hz, 0.5)], 1.0, None);
            let coarse = coarse_peaks(&x, SR, 20.0, 1).first().copied().unwrap();
            assert!(
                (coarse.hz - hz).abs() < limit * 0.25,
                "coarse estimate for {hz} was {:.3} Hz, uncomfortably close to the \
                 {limit:.2} Hz unwrapping limit",
                coarse.hz
            );
        }
    }

    #[test]
    fn rejects_configurations_it_cannot_honour() {
        let x = tone(vec![steady(440.0, 0.5)], 0.1, None);
        // Frame longer than the signal.
        assert!(refine(&x, SR, 440.0, RefineConfig::default()).is_none());
        // Not a power of two.
        let bad = RefineConfig {
            frame_len: 1000,
            ..RefineConfig::default()
        };
        assert!(refine(&x, SR, 440.0, bad).is_none());
    }

    #[test]
    fn amplitude_is_reported_faithfully() {
        let x = tone(vec![steady(441.93, 0.25)], 1.0, None);
        let r = refine(&x, SR, 441.93, RefineConfig::default()).unwrap();
        assert!(
            (r.amplitude - 0.25).abs() < 0.005,
            "amplitude reported as {:.4}, want 0.25",
            r.amplitude
        );
        // And the signal really is at the level we think.
        assert!((rms(&x) - 0.25 / 2f64.sqrt()).abs() < 0.01);
    }
}
