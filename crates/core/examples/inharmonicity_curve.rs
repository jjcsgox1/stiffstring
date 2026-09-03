//! The fitted inharmonicity curve against a known one, key by key.
//!
//! ```text
//! cargo run -p stiffstring-core --example inharmonicity_curve --release
//! ```
//!
//! The reference is a plausible small piano: stiffness climbing steeply into
//! the bass, stepping at the wound-to-plain break, climbing again through the
//! treble, plus a smooth wobble no two-line model can represent. That wobble is
//! the point — a reference that matched the model exactly would only prove the
//! arithmetic works.

use stiffstring_core::piano::{
    anchor_keys, fit_model, key_name, key_nominal_hz, suggest_next_key, InharmonicityModel,
    NoteSample, KEYS,
};

const TRUE_BREAK: f64 = 28.0;

fn reference_b(key: u8) -> f64 {
    let k = f64::from(key);
    let log10b = if k < TRUE_BREAK {
        -2.70 - 0.0415 * (k - 1.0)
    } else {
        -3.70 + 0.0271 * (k - TRUE_BREAK)
    };
    10f64.powf(log10b + 0.05 * (k / 9.0).sin())
}

fn sample_at(key: u8) -> NoteSample {
    NoteSample {
        key,
        f0: key_nominal_hz(key, 440.0),
        b: reference_b(key),
        weight: 1.0,
    }
}

fn report(label: &str, model: &InharmonicityModel, sampled: &[u8]) {
    println!("\n{label}");
    println!(
        "  break: {}   samples: {}   trend residual: {:.4} log10",
        model
            .break_name()
            .unwrap_or_else(|| "none found".to_string()),
        model.samples,
        model.rms_log10
    );
    println!(
        "  {:>4} {:>5} {:>11} {:>11} {:>9}  sampled",
        "key", "note", "true B", "model B", "error"
    );

    let mut worst = 0.0f64;
    let mut worst_key = 0u8;
    let mut all: Vec<f64> = Vec::new();
    for key in 1..=KEYS {
        let truth = reference_b(key);
        let got = model.b_at(key);
        let err = (got - truth) / truth;
        all.push(err.abs());
        if err.abs() > worst {
            worst = err.abs();
            worst_key = key;
        }
        // Print every third key, plus every sampled one, to keep this readable.
        if key % 3 == 1 || sampled.contains(&key) {
            println!(
                "  {key:>4} {:>5} {truth:>11.3e} {got:>11.3e} {:>8.1}%  {}",
                key_name(key),
                err * 100.0,
                if sampled.contains(&key) { "*" } else { "" }
            );
        }
    }
    all.sort_by(f64::total_cmp);
    println!(
        "  worst {:.1}% at key {worst_key} ({}), median {:.1}%",
        worst * 100.0,
        key_name(worst_key),
        all[all.len() / 2] * 100.0
    );
}

fn main() {
    let anchors = anchor_keys();
    let samples: Vec<NoteSample> = anchors.iter().copied().map(sample_at).collect();
    let model = fit_model(&samples).expect("no model from anchors");
    report("FROM THE ANCHOR NOTES", &model, &anchors);

    // Then let the model ask for more.
    let mut chosen = samples.clone();
    let mut keys = anchors.clone();
    for _ in 0..4 {
        let m = fit_model(&chosen).expect("no model");
        let Some(next) = suggest_next_key(&chosen, &m) else {
            break;
        };
        println!("\n  requested: key {next} ({})", key_name(next));
        chosen.push(sample_at(next));
        keys.push(next);
    }
    keys.sort_unstable();
    let refined = fit_model(&chosen).expect("no model");
    report("AFTER THE NOTES IT ASKED FOR", &refined, &keys);

    // A false-beating string reading three times too stiff. It must be spotted
    // and dropped, not absorbed into the curve.
    let mut spoiled: Vec<NoteSample> = anchors.iter().copied().map(sample_at).collect();
    let bad_key = spoiled[5].key;
    spoiled[5].b *= 3.0;
    let model = fit_model(&spoiled).expect("no model");
    println!(
        "\n  planted a bad reading on key {bad_key} ({}): {} samples went in, {} were kept",
        key_name(bad_key),
        spoiled.len(),
        model.samples
    );
    for s in &spoiled {
        let trend = model.trend_log10_b_at(s.key);
        println!(
            "    key {:>2} ({:>4})  measured {:.3e}  trend {:.3e}  residual {:+.3} log10",
            s.key,
            key_name(s.key),
            s.b,
            10f64.powf(trend),
            s.b.log10() - trend
        );
    }
    report("WITH ONE BAD STRING", &model, &anchors);
}
