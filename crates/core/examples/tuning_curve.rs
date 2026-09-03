//! The tuning curve for a simulated small piano, with the beat rates it implies.
//!
//! ```text
//! cargo run -p stiffstring-core --example tuning_curve --release
//! ```
//!
//! Prints what a technician would actually check: the stretch in cents, the beat
//! rate of thirds climbing up the keyboard, octave widths, and which interval
//! families ended up compromised and by how much. The curve is a choice, so this
//! is the evidence for whether it is a defensible one.

use std::collections::BTreeMap;

use stiffstring_core::curve::{explain, solve, CurveConfig, Family, CHECKS};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, InharmonicityModel, NoteSample, KEYS};

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

fn main() {
    let model = spinet_model();
    let cfg = CurveConfig::default();
    let curve = solve(&model, &cfg).expect("no curve");

    println!("TUNING CURVE  (simulated spinet, A4 = {:.1} Hz)", cfg.a4_hz);
    println!("  {:>4} {:>5} {:>10} {:>9} {:>11}", "key", "note", "target Hz", "stretch", "B");
    for key in (1..=KEYS).step_by(3) {
        println!(
            "  {key:>4} {:>5} {:>10.3} {:>8.2}\u{a2} {:>11.3e}",
            key_name(key),
            curve.hz_at(key),
            curve.cents_at(key),
            model.b_at(key)
        );
    }
    println!(
        "\n  span: {:.2} cents at A0 to {:+.2} at C8",
        curve.cents_at(1),
        curve.cents_at(88)
    );

    // Curvature, note to note.
    let mut worst_kink = (0.0f64, 0u8);
    for key in 2..KEYS {
        let k = curve.cents_at(key + 1) - 2.0 * curve.cents_at(key) + curve.cents_at(key - 1);
        if k.abs() > worst_kink.0 {
            worst_kink = (k.abs(), key);
        }
    }
    println!(
        "  worst kink: {:.3} cents at {}",
        worst_kink.0,
        key_name(worst_kink.1)
    );

    // Thirds climbing, which is the owner's stated priority.
    let third = CHECKS.iter().find(|c| c.family == Family::Third).unwrap();
    println!("\nMAJOR THIRDS  (5:4, beat rate should climb steadily)");
    print!("  ");
    for key in (24..=56u8).step_by(4) {
        print!(
            "{} {:.1}/s   ",
            key_name(key),
            curve.beat_rate(&model, key, *third).unwrap_or(f64::NAN)
        );
    }
    println!();

    // Octave widths.
    println!("\nOCTAVES  (width beyond 2:1, and the 4:2 beat rate)");
    let octave42 = CHECKS
        .iter()
        .find(|c| c.family == Family::Octave && c.lower_partial == 4)
        .unwrap();
    print!("  ");
    for key in [8u8, 20, 32, 44, 56, 68] {
        println!(
            "  {:>4}-{:<4} {:+.2} cents wide, 4:2 beats {:.2}/s",
            key_name(key),
            key_name(key + 12),
            curve.cents_at(key + 12) - curve.cents_at(key),
            curve.beat_rate(&model, key, *octave42).unwrap_or(f64::NAN)
        );
    }

    // Where the compromise fell.
    let results = explain(&model, &curve, &cfg);
    let mut by_family: BTreeMap<&str, (f64, f64, f64, usize)> = BTreeMap::new();
    for r in &results {
        let e = by_family.entry(r.check.family.name()).or_insert((0.0, 0.0, 0.0, 0));
        e.0 += r.weight * r.error_cents * r.error_cents;
        e.1 += r.weight;
        e.2 = e.2.max(r.error_cents.abs());
        e.3 += 1;
    }
    println!("\nWHERE THE COMPROMISE FELL");
    println!("  {:>18} {:>10} {:>10} {:>7}", "family", "wtd rms", "worst", "checks");
    for (name, (sse, wsum, worst, n)) in &by_family {
        println!(
            "  {name:>18} {:>9.2}\u{a2} {worst:>9.2}\u{a2} {n:>7}",
            if *wsum > 0.0 { (sse / wsum).sqrt() } else { 0.0 }
        );
    }

    let wsse: f64 = results.iter().map(|r| r.weight * r.error_cents * r.error_cents).sum();
    let wsum: f64 = results.iter().map(|r| r.weight).sum();
    let worst = results.iter().map(|r| r.error_cents.abs()).fold(0.0, f64::max);
    println!(
        "\n  overall weighted rms {:.2} cents, worst single check {:.2} cents",
        (wsse / wsum).sqrt(),
        worst
    );
    if let Some(w) = results
        .iter()
        .max_by(|a, b| a.error_cents.abs().total_cmp(&b.error_cents.abs()))
    {
        println!(
            "  worst is a {} {}:{} on {} — weight {:.3}",
            w.check.family.name(),
            w.check.lower_partial,
            w.check.upper_partial,
            key_name(w.lower_key),
            w.weight
        );
    }
}
