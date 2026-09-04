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
    /// Trust in this measurement, 0 to 1. Heuristic, and deliberately forgiving
    /// of wobble that [`beat_hz`](Self::beat_hz) accounts for.
    pub confidence: f64,
    /// Rate at which this partial's amplitude rises and falls, when a single
    /// coherent beat explains the wobble.
    ///
    /// On a real piano this is the normal case, not a fault: nearly every note
    /// has two or three strings, and a partial is therefore two or three
    /// closely spaced components rather than one. What it beats at tells us how
    /// far apart they are.
    pub beat_hz: Option<f64>,
    /// How much of the amplitude wobble that one beat accounts for, 0 to 1.
    /// High means two clean strings; low means noise, or something messier.
    pub beat_strength: f64,
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
            // Enough measurements across the note to see the amplitude rise and
            // fall, not merely to fit a line through its phase. Two strings a
            // couple of cents apart beat a few times a second, and a handful of
            // samples cannot show that.
            frames: 32,
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

/// One look at the component near a chosen frequency: its complex amplitude at
/// one instant.
#[derive(Clone, Copy, Debug)]
struct Look {
    t: f64,
    re: f64,
    im: f64,
}

impl Look {
    #[inline]
    fn magnitude(&self) -> f64 {
        self.re.hypot(self.im)
    }
}

/// Follow one component's complex amplitude across the note.
///
/// Multiplying the signal by an oscillator at `hz` and averaging over a window
/// leaves the amplitude and phase of whatever sits near that frequency: the same
/// quantity one FFT bin carries, obtained without computing the several thousand
/// bins we would discard. That is cheaper by more than an order of magnitude,
/// which is what makes it affordable to take dozens of looks across a note
/// rather than a handful — and dozens are needed to watch an amplitude rise and
/// fall, which is how beating strings betray themselves.
///
/// It also removes scalloping loss. A bin measures the frequency it is centred
/// on; this measures the frequency we ask for, so a partial between bins is no
/// longer under-read.
fn trajectory(
    samples: &[f32],
    sample_rate: f64,
    hz: f64,
    frame_len: usize,
    hop: usize,
    frames: usize,
    kind: Window,
) -> Vec<Look> {
    let w = window(kind, frame_len);
    // A real signal splits its energy between positive and negative frequency,
    // so half the window sum recovers the true amplitude.
    let norm = w.iter().sum::<f64>() / 2.0;
    if norm <= 0.0 {
        return Vec::new();
    }

    // The oscillator's samples within a frame are the same for every frame, so
    // they are built once. Only the frame's starting phase differs, and that is
    // one rotation applied afterwards.
    let step = TAU * hz / sample_rate;
    let table: Vec<(f64, f64)> = (0..frame_len)
        .map(|i| {
            let (sin, cos) = (-step * i as f64).sin_cos();
            (cos, sin)
        })
        .collect();

    let mut out = Vec::with_capacity(frames);
    for j in 0..frames {
        let start = j * hop;
        if start + frame_len > samples.len() {
            break;
        }
        let mut re = 0.0;
        let mut im = 0.0;
        for (i, &(cos, sin)) in table.iter().enumerate() {
            let x = f64::from(samples[start + i]) * w[i];
            re += x * cos;
            im += x * sin;
        }
        // Refer the phase to absolute time rather than to the frame's start, so
        // successive looks share one clock and their phases can be compared.
        let (sin, cos) = (-step * start as f64).sin_cos();
        out.push(Look {
            t: start as f64 / sample_rate,
            re: (re * cos - im * sin) / norm,
            im: (re * sin + im * cos) / norm,
        });
    }
    out
}

/// Slowest and fastest beat worth looking for, in Hz.
///
/// Below the slow end a beat cannot be told from the note simply decaying within
/// the window; above the fast end it stops being a beat and becomes roughness.
const BEAT_MIN_HZ: f64 = 0.4;
const BEAT_MAX_HZ: f64 = 12.0;

/// How many cycles of a beat must fit inside the window before it is believed.
const MIN_BEAT_CYCLES: f64 = 1.5;

/// Smallest modulation, in natural-log amplitude units, that counts as beating.
/// About twelve percent — far beyond what a noise floor produces, and far below
/// what two strings do.
const MIN_BEAT_DEPTH: f64 = 0.12;

/// Phase wobble, in radians, at which a partial is worth nothing.
///
/// Calibrated against real notes rather than synthetic ones. A lone synthetic
/// string holds phase to a few hundredths of a radian, and an earlier threshold
/// of a quarter radian was set from exactly that — which marked almost every
/// note of a real piano untrustworthy, because real notes have two or three
/// strings and genuinely do wobble. The measurements were fine; the yardstick
/// was not.
const UNUSABLE_PHASE_WOBBLE: f64 = 0.6;

/// Look for a single coherent rise and fall in a component's amplitude.
///
/// Returns the rate and how much of the wobble it accounts for.
///
/// Works on the logarithm of the amplitude, where the note's decay is a straight
/// line and can be removed by subtraction. Deep nulls — which is what two evenly
/// matched strings produce — are clipped rather than allowed to run to negative
/// infinity and dominate everything.
fn detect_beat(looks: &[Look], sample_rate_of_looks: f64) -> (Option<f64>, f64) {
    if looks.len() < 10 {
        return (None, 0.0);
    }
    let peak = looks.iter().map(Look::magnitude).fold(0.0, f64::max);
    if peak <= 0.0 {
        return (None, 0.0);
    }

    let floor = peak * 0.02;
    let logs: Vec<f64> = looks.iter().map(|l| l.magnitude().max(floor).ln()).collect();
    let times: Vec<f64> = looks.iter().map(|l| l.t).collect();

    // Remove the decay, which in this domain is a straight line.
    let n = logs.len() as f64;
    let mean_t = times.iter().sum::<f64>() / n;
    let mean_l = logs.iter().sum::<f64>() / n;
    let sxx: f64 = times.iter().map(|t| (t - mean_t) * (t - mean_t)).sum();
    if sxx <= 0.0 {
        return (None, 0.0);
    }
    let sxy: f64 = times
        .iter()
        .zip(&logs)
        .map(|(t, l)| (t - mean_t) * (l - mean_l))
        .sum();
    let slope = sxy / sxx;
    let residual: Vec<f64> = times
        .iter()
        .zip(&logs)
        .map(|(t, l)| l - (mean_l + slope * (t - mean_t)))
        .collect();

    let energy: f64 = residual.iter().map(|r| r * r).sum();
    if energy <= 1e-12 {
        return (None, 0.0); // perfectly steady: one string, or one that sounds like it
    }

    // Nothing above half the rate we are sampling the amplitude at can be
    // believed, so do not look there.
    let ceiling = BEAT_MAX_HZ.min(sample_rate_of_looks / 2.0);

    // Nor below what the window can actually show. Claiming a beat from less
    // than about one and a half cycles is indistinguishable from claiming that
    // the note got quieter and then slightly less quiet, so the slow end is set
    // by how long we watched rather than by preference. The cost is real: a
    // unison tight enough to beat slower than this goes unreported — but a
    // unison that tight is not one the technician needs telling about.
    let span = times[times.len() - 1] - times[0];
    if span <= 0.0 {
        return (None, 0.0);
    }
    let floor_hz = (MIN_BEAT_CYCLES / span).max(BEAT_MIN_HZ);
    if floor_hz >= ceiling {
        return (None, 0.0);
    }

    let steps = 240;
    let mut best = (0.0f64, 0.0f64); // (amplitude, hz)
    for i in 0..=steps {
        let f = floor_hz + (ceiling - floor_hz) * i as f64 / f64::from(steps);
        let (mut re, mut im) = (0.0, 0.0);
        for (t, r) in times.iter().zip(&residual) {
            let (sin, cos) = (TAU * f * t).sin_cos();
            re += r * cos;
            im -= r * sin;
        }
        // A sinusoid of amplitude A correlates to A*n/2 against its own frequency.
        let amplitude = 2.0 * re.hypot(im) / n;
        if amplitude > best.0 {
            best = (amplitude, f);
        }
    }

    // Share of the wobble that one rate explains.
    //
    // The thresholds are set well above what noise reaches by luck. Scanning
    // hundreds of candidate rates and keeping the best will always explain some
    // of any residual — with a few dozen looks that alone clears a third of the
    // variance often enough to matter — so a beat has to be both coherent and
    // deep before it is believed. Two strings produce something unmistakable.
    let explained = ((n * best.0 * best.0 / 2.0) / energy).min(1.0);
    if explained > 0.5 && best.0 > MIN_BEAT_DEPTH {
        (Some(best.1), explained)
    } else {
        (None, explained)
    }
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
    if approx_hz.is_nan() || approx_hz <= 0.0 || approx_hz >= sample_rate / 2.0 {
        return None;
    }

    let hop = (samples.len() - n) / (cfg.frames - 1);
    if hop == 0 {
        return None;
    }

    let looks = trajectory(samples, sample_rate, approx_hz, n, hop, cfg.frames, cfg.window);
    if looks.len() < 2 {
        return None;
    }

    // Phase, unwrapped against an oscillator running at exactly `approx_hz`.
    // What is left is a straight line whose slope is the frequency error.
    let mut times = Vec::with_capacity(looks.len());
    let mut residuals = Vec::with_capacity(looks.len());
    let mut frame_amps = Vec::with_capacity(looks.len());
    let mut amplitude_sum = 0.0;
    let mut previous = 0.0;

    for (j, look) in looks.iter().enumerate() {
        let amp = look.magnitude();
        amplitude_sum += amp;
        frame_amps.push(amp);

        let measured = look.im.atan2(look.re);
        let r = if j == 0 {
            measured
        } else {
            // Continue the line rather than wrapping independently, so a steady
            // drift accumulates instead of folding back on itself.
            previous + wrap(measured - previous)
        };
        previous = r;
        times.push(look.t);
        residuals.push(r);
    }

    // Least squares through (t, phase), weighted by each look's amplitude
    // squared.
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

    let (beat_hz, beat_strength) = detect_beat(&looks, sample_rate / hop as f64);

    // A partial that wobbles because two strings are beating is not an unreliable
    // measurement — it is a correct measurement of a note with two strings, which
    // is nearly every note on a piano. Penalising that as heavily as incoherent
    // noise would mark almost the whole instrument untrustworthy, which is
    // exactly what happened on the first real recordings.
    //
    // So wobble the beat accounts for is forgiven, and wobble it does not is not.
    let unexplained = phase_residual * (1.0 - 0.7 * beat_strength.clamp(0.0, 1.0));

    Some(Refined {
        hz: approx_hz + slope / TAU,
        amplitude: amplitude_sum / looks.len() as f64,
        phase_residual,
        confidence: (1.0 - unexplained / UNUSABLE_PHASE_WOBBLE).clamp(0.0, 1.0),
        beat_hz,
        beat_strength,
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
    fn a_beating_pair_is_recognised_as_a_beat() {
        // Two strings six cents apart near 220 Hz beat about 0.76 times a
        // second. The amplitude rising and falling at a steady rate is the
        // signature, and reporting the rate is worth far more than merely
        // noting that the phase misbehaved — it is the unison spread.
        let a = steady(220.61, 0.5);
        let b = StringSpec {
            f0: 220.61 * 2f64.powf(6.0 / 1200.0),
            ..a.clone()
        };
        let x = tone(vec![a, b], 4.0, None);
        let r = refine(&x, SR, 220.61, RefineConfig::default()).unwrap();

        let beat = r.beat_hz.expect("two beating strings reported no beat");
        assert!(
            (beat - 0.766).abs() < 0.2,
            "expected about 0.77 beats a second, measured {beat:.3}"
        );
        assert!(
            r.beat_strength > 0.5,
            "the beat should account for most of the wobble: {:.2}",
            r.beat_strength
        );
        assert!(
            r.phase_residual > 0.05,
            "a beating pair does not fit a straight line: residual {:.4} rad",
            r.phase_residual
        );
    }

    #[test]
    fn a_lone_string_reports_no_beat() {
        let x = tone(vec![steady(220.61, 0.5)], 4.0, Some(-70.0));
        let r = refine(&x, SR, 220.61, RefineConfig::default()).unwrap();
        assert!(
            r.beat_hz.is_none(),
            "a single string was reported as beating at {:?} Hz",
            r.beat_hz
        );
        assert!(r.confidence > 0.9, "confidence {:.3}", r.confidence);
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
