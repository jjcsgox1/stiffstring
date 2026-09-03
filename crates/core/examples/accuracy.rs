//! Measured accuracy of the estimator, against signals whose answer we know.
//!
//! Run with:
//!
//! ```text
//! cargo run -p stiffstring-core --example accuracy --release
//! ```
//!
//! The test suite asserts the gates; this reports where we actually stand, so a
//! change that quietly costs precision without breaking a threshold still shows.

use stiffstring_core::estimate::{cents, coarse_peaks, refine, RefineConfig};
use stiffstring_core::synth::{partial_hz, render, StringSpec, ToneSpec};

const SR: f64 = 48_000.0;

/// A sinusoid that does not decay, for measuring the precision floor.
fn steady(hz: f64, amp: f64, phase: f64) -> StringSpec {
    StringSpec {
        f0: hz,
        b: 0.0,
        amp,
        partials: 1,
        rolloff: 0.0,
        t60: 1e9,
        decay_exp: 0.0,
        phase,
    }
}

fn tone(strings: Vec<StringSpec>, secs: f64, noise_dbfs: Option<f64>, seed: u64) -> Vec<f32> {
    render(&ToneSpec {
        strings,
        sample_rate: SR,
        duration: secs,
        noise_dbfs,
        seed,
        clip: None,
    })
}

fn noise_for_snr(amp: f64, snr_db: f64) -> f64 {
    20.0 * (amp / 2f64.sqrt()).log10() - snr_db
}

/// Error in cents over several starting phases and noise seeds, so a lucky
/// alignment cannot flatter the result.
fn error_cents(hz: f64, snr_db: Option<f64>, trials: usize) -> (f64, f64) {
    let amp = 0.5;
    let mut errors: Vec<f64> = Vec::with_capacity(trials);
    for t in 0..trials {
        let phase = t as f64 * 0.9;
        let noise = snr_db.map(|s| noise_for_snr(amp, s));
        let x = tone(vec![steady(hz, amp, phase)], 1.0, noise, 1000 + t as u64);
        let Some(coarse) = coarse_peaks(&x, SR, 20.0, 1).first().copied() else {
            continue;
        };
        if let Some(r) = refine(&x, SR, coarse.hz, RefineConfig::default()) {
            errors.push(cents(hz, r.hz).abs());
        }
    }
    if errors.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    errors.sort_by(f64::total_cmp);
    (errors[errors.len() / 2], *errors.last().unwrap())
}

fn main() {
    println!("Stiffstring estimator accuracy");
    println!("1 second of signal at 48 kHz, 8192-point frames, 8 phase measurements\n");

    println!("CLEAN SIGNAL                      (gate: better than 0.020 cents)");
    println!("  {:>10}  {:>12}  {:>12}", "Hz", "median", "worst");
    let mut worst_clean: f64 = 0.0;
    for &hz in &[27.53, 55.07, 110.31, 220.61, 441.93, 880.17, 1329.87, 3517.4] {
        let (median, worst) = error_cents(hz, None, 9);
        worst_clean = worst_clean.max(worst);
        println!("  {hz:>10.2}  {median:>9.5}\u{a2}  {worst:>9.5}\u{a2}");
    }

    println!("\nAGAINST NOISE, at 441.93 Hz       (gate: better than 0.100 cents at 20 dB)");
    println!("  {:>10}  {:>12}  {:>12}", "SNR", "median", "worst");
    let mut worst_at_20: f64 = 0.0;
    for &snr in &[40.0, 30.0, 20.0, 10.0, 0.0, -6.0] {
        let (median, worst) = error_cents(441.93, Some(snr), 9);
        if (snr - 20.0).abs() < f64::EPSILON {
            worst_at_20 = worst;
        }
        println!("  {snr:>7.0} dB  {median:>9.5}\u{a2}  {worst:>9.5}\u{a2}");
    }

    // A real note: A3 as measured on the owner's piano, decaying, with a floor.
    let (f0, b) = (220.63, 3.04e-4);
    println!("\nA DECAYING PIANO NOTE  (A3, B = {b:.2e}, 60 dB noise floor)");
    println!(
        "  {:>7}  {:>11}  {:>11}  {:>9}  {:>10}",
        "partial", "true Hz", "measured", "error", "confidence"
    );
    let x = tone(
        vec![StringSpec::new(f0, b).with_partials(8).with_amp(0.4)],
        1.5,
        Some(-60.0),
        7,
    );
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for n in 1..=6u32 {
        let want = partial_hz(f0, b, n);
        let Some(r) = refine(&x, SR, want, RefineConfig::default()) else {
            continue;
        };
        println!(
            "  {n:>7}  {want:>11.4}  {:>11.4}  {:>7.4}\u{a2}  {:>10.3}",
            r.hz,
            cents(want, r.hz),
            r.confidence
        );
        let per = r.hz / f64::from(n);
        xs.push(f64::from(n * n));
        ys.push(per * per);
    }

    // Recover B from those measurements, the way phase 3 will but without the
    // weighting and outlier rejection that belong there.
    let m = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / m;
    let my = ys.iter().sum::<f64>() / m;
    let sxy: f64 = xs.iter().zip(&ys).map(|(a, c)| (a - mx) * (c - my)).sum();
    let sxx: f64 = xs.iter().map(|a| (a - mx) * (a - mx)).sum();
    let slope = sxy / sxx;
    let intercept = my - slope * mx;
    let got_b = slope / intercept;
    let got_f0 = intercept.sqrt();

    println!("\n  inharmonicity recovered: B = {got_b:.4e}  (true {b:.4e}, {:+.2}%)",
        100.0 * (got_b - b) / b);
    println!("  fundamental recovered:   f0 = {got_f0:.4} Hz  (true {f0}, {:+.4} cents)",
        cents(f0, got_f0));

    println!("\nGATES");
    println!(
        "  clean better than 0.020 cents  : {}  (worst {worst_clean:.5})",
        if worst_clean < 0.02 { "PASS" } else { "FAIL" }
    );
    println!(
        "  20 dB SNR better than 0.100    : {}  (worst {worst_at_20:.5})",
        if worst_at_20 < 0.1 { "PASS" } else { "FAIL" }
    );
}
