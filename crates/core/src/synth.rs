//! Synthetic piano tones with known ground truth.
//!
//! Phase 0b. Every claim the measurement engine makes is graded against signals
//! generated here, where `f0` and `B` are *chosen* rather than estimated. Without
//! this we would be tuning algorithms by intuition.
//!
//! The model is the standard stiff-string relation
//!
//! ```text
//! f_n = n * f0 * sqrt(1 + B * n^2)
//! ```
//!
//! which is an excellent description of a real piano string and the right place
//! to start. It is not the whole truth — wound bass strings deviate — so the
//! generator also produces the things that break naive estimators: differential
//! partial decay, beating unisons, a noise floor, and clipping.

use std::f64::consts::TAU;

/// Frequency of the `n`th partial of a stiff string. `n` is 1-based, so `n == 1`
/// is the fundamental.
#[inline]
pub fn partial_hz(f0: f64, b: f64, n: u32) -> f64 {
    debug_assert!(n >= 1, "partials are 1-based");
    let n = f64::from(n);
    n * f0 * (1.0 + b * n * n).sqrt()
}

/// Recover the fundamental from one measured partial, given `B`.
///
/// This is the operation the bass depends on: below about C2 the fundamental is
/// too weak to measure directly, so it is inferred from upper partials instead.
#[inline]
pub fn f0_from_partial(measured_hz: f64, b: f64, n: u32) -> f64 {
    debug_assert!(n >= 1, "partials are 1-based");
    let n = f64::from(n);
    measured_hz / (n * (1.0 + b * n * n).sqrt())
}

/// How many cents sharp partial `n` sits relative to an exact harmonic multiple.
///
/// This is the quantity that makes octaves, twelfths and thirds disagree with
/// one another, and therefore the reason a tuning curve is a choice rather than
/// a calculation.
#[inline]
pub fn partial_stretch_cents(b: f64, n: u32) -> f64 {
    let n = f64::from(n);
    1200.0 * (1.0 + b * n * n).sqrt().log2()
}

/// Ratio between two frequencies expressed in cents.
#[inline]
pub fn cents_between(from_hz: f64, to_hz: f64) -> f64 {
    1200.0 * (to_hz / from_hz).log2()
}

/// A deterministic PRNG, so every test is reproducible and the crate stays
/// dependency-free. xorshift64*.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Any nonzero state will do; force it so a seed of 0 is still valid.
        Rng(seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F491_4F6CDD1D)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal, via Box-Muller.
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-12);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// One vibrating string.
#[derive(Clone, Debug)]
pub struct StringSpec {
    /// Fundamental in Hz — the value an estimator is expected to recover.
    pub f0: f64,
    /// Inharmonicity coefficient.
    pub b: f64,
    /// Peak amplitude of the fundamental, linear.
    pub amp: f64,
    /// How many partials to generate.
    pub partials: usize,
    /// Partial `n` starts at `amp * n^(-rolloff)`.
    pub rolloff: f64,
    /// Seconds for the fundamental to fall 60 dB.
    pub t60: f64,
    /// Partial `n` decays `n^decay_exp` times faster than the fundamental.
    /// This is why treble measurement windows have to be short: the upper
    /// partials are gone long before the fundamental is.
    pub decay_exp: f64,
    /// Starting phase, radians.
    pub phase: f64,
}

impl StringSpec {
    /// A plausible mid-range piano string.
    pub fn new(f0: f64, b: f64) -> Self {
        Self {
            f0,
            b,
            amp: 0.25,
            partials: 12,
            rolloff: 1.0,
            t60: 6.0,
            decay_exp: 0.7,
            phase: 0.0,
        }
    }

    /// The same string moved by `cents`, for building an out-of-tune unison.
    pub fn detuned(&self, cents: f64) -> Self {
        let mut s = self.clone();
        s.f0 = self.f0 * 2f64.powf(cents / 1200.0);
        s
    }

    pub fn with_amp(mut self, amp: f64) -> Self {
        self.amp = amp;
        self
    }

    pub fn with_partials(mut self, partials: usize) -> Self {
        self.partials = partials;
        self
    }

    pub fn with_phase(mut self, phase: f64) -> Self {
        self.phase = phase;
        self
    }
}

/// A struck note: one to three strings, plus whatever the room adds.
#[derive(Clone, Debug)]
pub struct ToneSpec {
    pub strings: Vec<StringSpec>,
    pub sample_rate: f64,
    pub duration: f64,
    /// Noise floor in dBFS. `None` for a silent background.
    pub noise_dbfs: Option<f64>,
    pub seed: u64,
    /// Hard-clip at this magnitude, to reproduce an overloaded microphone.
    pub clip: Option<f64>,
}

impl ToneSpec {
    /// A single clean string, 48 kHz, no noise.
    pub fn single(f0: f64, b: f64, duration: f64) -> Self {
        Self {
            strings: vec![StringSpec::new(f0, b)],
            sample_rate: 48_000.0,
            duration,
            noise_dbfs: None,
            seed: 0x5717_F517,
            clip: None,
        }
    }

    /// A unison of `count` strings spread evenly across `spread_cents`.
    pub fn unison(f0: f64, b: f64, duration: f64, count: usize, spread_cents: f64) -> Self {
        let base = StringSpec::new(f0, b);
        let strings = (0..count)
            .map(|i| {
                let offset = if count <= 1 {
                    0.0
                } else {
                    spread_cents * (i as f64 / (count - 1) as f64 - 0.5)
                };
                // Real strings are not struck in phase with one another.
                base.detuned(offset).with_phase(0.7 * i as f64)
            })
            .collect();
        Self {
            strings,
            ..Self::single(f0, b, duration)
        }
    }

    pub fn with_noise(mut self, dbfs: f64) -> Self {
        self.noise_dbfs = Some(dbfs);
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_clip(mut self, clip: f64) -> Self {
        self.clip = Some(clip);
        self
    }

    pub fn sample_count(&self) -> usize {
        (self.sample_rate * self.duration).round() as usize
    }
}

/// Render a tone to mono samples in [-1, 1].
pub fn render(spec: &ToneSpec) -> Vec<f32> {
    let n_samples = spec.sample_count();
    let mut out = vec![0f64; n_samples];
    let ln1000 = 1000f64.ln();
    let nyquist = spec.sample_rate / 2.0;

    for s in &spec.strings {
        for k in 1..=(s.partials as u32) {
            let hz = partial_hz(s.f0, s.b, k);
            if hz >= nyquist {
                break; // no aliasing: partials above Nyquist simply do not exist
            }
            let amp = s.amp * f64::from(k).powf(-s.rolloff);
            let t60 = s.t60 / f64::from(k).powf(s.decay_exp);
            let lambda = ln1000 / t60;
            let w = TAU * hz / spec.sample_rate;

            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f64 / spec.sample_rate;
                *o += amp * (-lambda * t).exp() * (w * i as f64 + s.phase).sin();
            }
        }
    }

    if let Some(db) = spec.noise_dbfs {
        let sigma = 10f64.powf(db / 20.0);
        let mut rng = Rng::new(spec.seed);
        for o in out.iter_mut() {
            *o += sigma * rng.normal();
        }
    }

    out.into_iter()
        .map(|v| {
            let v = match spec.clip {
                Some(c) => v.clamp(-c, c),
                None => v,
            };
            v as f32
        })
        .collect()
}

/// Root-mean-square level of a block of samples.
pub fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let sum: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / x.len() as f64).sqrt()
}

/// Linear amplitude as dB relative to full scale.
pub fn dbfs(amplitude: f64) -> f64 {
    if amplitude > 0.0 {
        20.0 * amplitude.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// Magnitude at a single frequency, by Goertzel.
///
/// Used by tests to ask "is there energy exactly here?" without needing a whole
/// FFT. Returned as amplitude, comparable to the `amp` fields above.
pub fn goertzel(x: &[f32], sample_rate: f64, hz: f64) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let w = TAU * hz / sample_rate;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0f64, 0f64);
    for &v in x {
        let s0 = f64::from(v) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * w.cos();
    let imag = s2 * w.sin();
    2.0 * (real * real + imag * imag).sqrt() / x.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn assert_close(got: f64, want: f64, tol: f64, what: &str) {
        assert!(
            (got - want).abs() <= tol,
            "{what}: got {got}, want {want} (tolerance {tol})"
        );
    }

    #[test]
    fn zero_inharmonicity_gives_exact_harmonics() {
        for n in 1..=16 {
            assert_close(
                partial_hz(220.0, 0.0, n),
                220.0 * f64::from(n),
                1e-9,
                "harmonic",
            );
        }
    }

    #[test]
    fn stiffness_sharpens_partials_progressively() {
        let b = 3e-4;
        let f0 = 220.0;
        let mut previous = 0.0;
        for n in 1..=10 {
            // Each partial divided by its number should climb steadily. This is
            // the signature we saw on the real piano in phase 0a.
            let per_partial = partial_hz(f0, b, n) / f64::from(n);
            assert!(
                per_partial > previous,
                "partial {n} did not sharpen: {per_partial} <= {previous}"
            );
            previous = per_partial;
        }
        // Sanity against a hand-computed value.
        assert_close(partial_hz(f0, b, 4) / 4.0, 220.528, 1e-3, "4th partial");
    }

    #[test]
    fn f0_round_trips_from_any_partial() {
        let (f0, b) = (27.5, 1.2e-3); // A0 on a small piano
        for n in 1..=12 {
            let recovered = f0_from_partial(partial_hz(f0, b, n), b, n);
            assert_close(recovered, f0, 1e-9, "round trip");
        }
    }

    #[test]
    fn stretch_in_cents_matches_the_frequency_ratio() {
        let b = 5e-4;
        for n in 1..=8 {
            let want = cents_between(220.0 * f64::from(n), partial_hz(220.0, b, n));
            assert_close(partial_stretch_cents(b, n), want, 1e-9, "stretch");
        }
    }

    #[test]
    fn render_produces_the_requested_length() {
        let spec = ToneSpec::single(440.0, 0.0, 0.5);
        assert_eq!(render(&spec).len(), 24_000);
    }

    #[test]
    fn a_single_partial_lands_where_the_model_says() {
        let (f0, b) = (220.0, 3e-4);
        let spec = ToneSpec {
            strings: vec![StringSpec::new(f0, b).with_partials(6)],
            ..ToneSpec::single(f0, b, 1.0)
        };
        let x = render(&spec);

        for n in 1..=6u32 {
            let hz = partial_hz(f0, b, n);
            let on = goertzel(&x, SR, hz);
            // A quarter-tone away there should be far less energy.
            let off = goertzel(&x, SR, hz * 2f64.powf(50.0 / 1200.0));
            assert!(
                on > off * 8.0,
                "partial {n} at {hz:.2} Hz: on-peak {on:.5} vs off-peak {off:.5}"
            );
        }
    }

    #[test]
    fn partials_above_nyquist_are_not_generated() {
        // 40 partials of 3 kHz would run past 120 kHz. Anything above Nyquist
        // must be dropped rather than folded back down into the audible range,
        // where it would masquerade as a partial that isn't there.
        //
        // Tested by identity rather than by probing the spectrum: an unwindowed
        // Goertzel leaks enough energy from a strong partial to swamp any
        // threshold loose enough to be meaningful.
        let (f0, b) = (3000.0, 1e-4);
        let fits = (1..=40u32)
            .take_while(|&n| partial_hz(f0, b, n) < SR / 2.0)
            .count();
        assert!(
            fits < 40,
            "test proves nothing unless some partials are excluded"
        );

        let requested = ToneSpec {
            strings: vec![StringSpec::new(f0, b).with_partials(40)],
            ..ToneSpec::single(f0, b, 0.25)
        };
        let audible = ToneSpec {
            strings: vec![StringSpec::new(f0, b).with_partials(fits)],
            ..ToneSpec::single(f0, b, 0.25)
        };
        assert_eq!(
            render(&requested),
            render(&audible),
            "partials above Nyquist changed the output"
        );
    }

    #[test]
    fn amplitude_falls_sixty_db_over_t60() {
        let mut s = StringSpec::new(440.0, 0.0);
        s.partials = 1;
        s.t60 = 2.0;
        s.rolloff = 0.0;
        let spec = ToneSpec {
            strings: vec![s],
            ..ToneSpec::single(440.0, 0.0, 2.2)
        };
        let x = render(&spec);

        let head = rms(&x[0..4800]); // first 0.1 s
        let at_t60 = rms(&x[(2.0 * SR) as usize..(2.1 * SR) as usize]);
        let drop = dbfs(at_t60) - dbfs(head);
        // Not exactly -60 because each window spans some decay of its own.
        assert_close(drop, -60.0, 1.5, "decay over T60");
    }

    #[test]
    fn higher_partials_die_first() {
        let (f0, b) = (110.0, 8e-5);
        let spec = ToneSpec {
            strings: vec![StringSpec::new(f0, b).with_partials(8)],
            ..ToneSpec::single(f0, b, 4.0)
        };
        let x = render(&spec);
        let early = &x[0..(0.2 * SR) as usize];
        let late = &x[(3.0 * SR) as usize..(3.2 * SR) as usize];

        let ratio = |w: &[f32], n: u32| goertzel(w, SR, partial_hz(f0, b, n));
        let early_ratio = ratio(early, 8) / ratio(early, 1);
        let late_ratio = ratio(late, 8) / ratio(late, 1);
        assert!(
            late_ratio < early_ratio * 0.5,
            "8th partial should fade relative to the fundamental: \
             early {early_ratio:.4}, late {late_ratio:.4}"
        );
    }

    #[test]
    fn a_detuned_unison_beats_at_the_difference_frequency() {
        // Two strings 4 cents apart near 220 Hz differ by about 0.51 Hz, so the
        // envelope should dip and recover roughly every two seconds. This is the
        // effect we saw on the real piano, and the basis of the unison tools.
        let (f0, b) = (220.0, 0.0);
        let a = StringSpec {
            partials: 1,
            rolloff: 0.0,
            t60: 1e6, // no decay, so only beating moves the envelope
            ..StringSpec::new(f0, b)
        };
        let beat_hz = {
            let other = a.detuned(4.0);
            other.f0 - a.f0
        };
        let spec = ToneSpec {
            strings: vec![a.clone(), a.detuned(4.0)],
            ..ToneSpec::single(f0, b, 8.0)
        };
        let x = render(&spec);

        // Envelope in 25 ms blocks; count how many times it crosses its own mean.
        let block = (0.025 * SR) as usize;
        let env: Vec<f64> = x.chunks(block).map(rms).collect();
        let mean = env.iter().sum::<f64>() / env.len() as f64;
        let crossings = env
            .windows(2)
            .filter(|w| (w[0] - mean).signum() != (w[1] - mean).signum())
            .count();

        // Two crossings per beat cycle.
        let observed_beat_hz = crossings as f64 / 2.0 / spec.duration;
        assert_close(observed_beat_hz, beat_hz, 0.1, "beat rate");
        assert_close(beat_hz, 0.509, 0.01, "expected beat rate for 4 cents");
    }

    #[test]
    fn noise_lands_at_the_requested_level() {
        let spec = ToneSpec {
            strings: vec![],
            ..ToneSpec::single(440.0, 0.0, 1.0)
        }
        .with_noise(-60.0);
        let x = render(&spec);
        assert_close(dbfs(rms(&x)), -60.0, 0.5, "noise floor");
    }

    #[test]
    fn rendering_is_reproducible() {
        let spec = ToneSpec::single(440.0, 1e-4, 0.3).with_noise(-50.0);
        assert_eq!(render(&spec), render(&spec));
    }

    #[test]
    fn a_different_seed_gives_different_noise() {
        let a = ToneSpec::single(440.0, 1e-4, 0.3).with_noise(-50.0).with_seed(1);
        let b = ToneSpec::single(440.0, 1e-4, 0.3).with_noise(-50.0).with_seed(2);
        assert_ne!(render(&a), render(&b));
    }

    #[test]
    fn clipping_bounds_the_output() {
        let spec = ToneSpec {
            strings: vec![StringSpec::new(220.0, 0.0).with_amp(2.0)],
            ..ToneSpec::single(220.0, 0.0, 0.2)
        }
        .with_clip(0.5);
        let x = render(&spec);
        assert!(x.iter().all(|&v| v.abs() <= 0.5 + 1e-6), "clip not applied");
        assert!(x.iter().any(|&v| v.abs() > 0.49), "signal never reached the ceiling");
    }

    #[test]
    fn the_model_describes_a_real_piano() {
        // A4 as measured on the owner's piano in phase 0a. Fitting the stiff
        // string model to those four partials by least squares (linearised as
        // (f_n/n)^2 against n^2) gives f0 = 441.918 Hz, B = 7.33e-4 — a high
        // coefficient, consistent with the short strings of a small piano.
        //
        // This asserts that the model is *adequate*: that some (f0, B) describes
        // the real instrument to within the phone's own resolution. It is not a
        // test of our ability to estimate B, which needs a fitter and arrives in
        // phase 3.
        //
        // Tolerance is 0.5 Hz, roughly a third of the 1.465 Hz FFT bin the phone
        // measured through. The largest residual at the best fit is 0.32 Hz, so
        // anything tighter would be asserting precision the data never had.
        let (f0, b) = (441.918, 7.33e-4);
        let observed = [442.1, 885.2, 1329.8, 1778.2];
        for (i, &want) in observed.iter().enumerate() {
            let n = i as u32 + 1;
            assert_close(
                partial_hz(f0, b, n),
                want,
                0.5,
                &format!("partial {n} of the real A4"),
            );
        }
    }

    #[test]
    fn inharmonicity_rises_with_pitch_as_measured() {
        // Two notes from the same real piano: A3 fitted to B = 3.04e-4, A4 to
        // 7.33e-4. Roughly a doubling per octave is textbook plain-wire
        // behaviour, and the two readings were captured independently — so this
        // records a consistency check on the instrument, not on our arithmetic.
        let (a3_b, a4_b) = (3.04e-4, 7.33e-4);
        let ratio = a4_b / a3_b;
        assert!(
            (2.0..3.0).contains(&ratio),
            "expected B to roughly double across the octave, got {ratio:.2}x"
        );

        // The consequence that matters: at the 4th partial A4 is stretched
        // noticeably further sharp than A3, which is why one stretch curve
        // cannot serve the whole keyboard.
        assert!(partial_stretch_cents(a4_b, 4) > partial_stretch_cents(a3_b, 4));
    }
}
