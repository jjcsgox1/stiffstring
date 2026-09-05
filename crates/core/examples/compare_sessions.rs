//! Compare two recordings of the same piano.
//!
//! ```text
//! cargo run -p stiffstring-core --example compare_sessions --release -- <folder A> <folder B>
//! ```
//!
//! This is the phase 4 gate. Measuring a piano accurately is worth nothing if
//! measuring it again tomorrow gives a different answer, and no reference
//! equipment is needed to find out — only the same instrument, twice.
//!
//! The number that decides it is the last one printed: how far apart the two
//! sessions' *tuning targets* end up. Everything above that is diagnosis for
//! when they disagree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, NoteSample, KEYS};
use stiffstring_core::wav;

struct Session {
    label: String,
    /// Measured fundamental and stiffness, by key.
    notes: BTreeMap<u8, (f64, f64)>,
    samples: Vec<NoteSample>,
}

fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

fn cents(from: f64, to: f64) -> f64 {
    1200.0 * (to / from).log2()
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn measure_folder(folder: &str) -> Session {
    let mut files: Vec<(u8, PathBuf)> = fs::read_dir(Path::new(folder))
        .unwrap_or_else(|e| {
            eprintln!("cannot read {folder}: {e}");
            std::process::exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            Some((key_from_name(name)?, p.clone()))
        })
        .collect();
    files.sort_by_key(|(k, _)| *k);

    let read = |path: &PathBuf| fs::read(path).ok().and_then(|b| wav::decode(&b).ok());

    // First pass: every note that can be measured unaided.
    let mut notes = BTreeMap::new();
    let mut samples: Vec<NoteSample> = Vec::new();
    let mut unmeasured: Vec<(u8, PathBuf)> = Vec::new();
    for (key, path) in &files {
        let Some(audio) = read(path) else { continue };
        let hint = key_nominal_hz(*key, 440.0);
        match measure_note(
            &audio.samples,
            audio.sample_rate,
            hint,
            MeasureConfig::default(),
        ) {
            Some(m) => {
                notes.insert(*key, (m.f0, m.b));
                samples.push(NoteSample::from_measurement(*key, &m));
            }
            None => unmeasured.push((*key, path.clone())),
        }
    }

    // Second pass: the notes that offered too few partials to determine both a
    // fundamental and a stiffness — the top octave, almost always. The notes
    // already measured say what the stiffness is up there, so only the
    // fundamental is left to find.
    //
    // Leaving them out is not neutral: the keyboard model then has to
    // extrapolate past its last measurement, and two sessions that stopped at
    // different notes extrapolate differently.
    if !unmeasured.is_empty() {
        if let Some(model) = fit_model(&samples) {
            for (key, path) in &unmeasured {
                let Some(audio) = read(path) else { continue };
                let cfg = MeasureConfig {
                    b_hint: Some(model.b_at(*key)),
                    ..MeasureConfig::default()
                };
                if let Some(m) = measure_note(
                    &audio.samples,
                    audio.sample_rate,
                    key_nominal_hz(*key, 440.0),
                    cfg,
                ) {
                    notes.insert(*key, (m.f0, m.b));
                    samples.push(NoteSample::from_measurement(*key, &m));
                }
            }
            samples.sort_by_key(|s| s.key);
        }
    }

    let label = Path::new(folder)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(folder)
        .to_string();
    Session {
        label,
        notes,
        samples,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).take(2).collect();
    if args.len() < 2 {
        eprintln!("usage: compare_sessions <folder A> <folder B>");
        std::process::exit(2);
    }

    let a = measure_folder(&args[0]);
    let b = measure_folder(&args[1]);
    println!("REPEATABILITY\n  A: {}\n  B: {}\n", a.label, b.label);

    // --- notes measured in both sessions ---
    println!("PER-NOTE MEASUREMENTS");
    println!(
        "  {:>4} {:>5} {:>11} {:>11} {:>8} {:>11} {:>11} {:>8}",
        "key", "note", "B in A", "B in B", "B diff", "f0 in A", "f0 in B", "f0 diff"
    );

    let mut b_diffs = Vec::new();
    let mut f0_diffs = Vec::new();
    for (key, (f0a, ba)) in &a.notes {
        let Some((f0b, bb)) = b.notes.get(key) else {
            continue;
        };
        let b_pct = 100.0 * (bb - ba) / ba;
        let f0_cents = cents(*f0a, *f0b);
        b_diffs.push(b_pct.abs());
        f0_diffs.push(f0_cents.abs());
        println!(
            "  {key:>4} {:>5} {ba:>11.3e} {bb:>11.3e} {b_pct:>7.1}% {f0a:>11.3} {f0b:>11.3} {f0_cents:>7.2}\u{a2}",
            key_name(*key)
        );
    }

    let only_a: Vec<u8> = a.notes.keys().filter(|k| !b.notes.contains_key(k)).copied().collect();
    let only_b: Vec<u8> = b.notes.keys().filter(|k| !a.notes.contains_key(k)).copied().collect();
    for (label, keys) in [("A only", &only_a), ("B only", &only_b)] {
        if !keys.is_empty() {
            println!(
                "  measured in {label}: {}",
                keys.iter().map(|k| key_name(*k)).collect::<Vec<_>>().join(", ")
            );
        }
    }

    println!(
        "\n  stiffness: median {:.1}%, worst {:.1}%",
        median(&mut b_diffs.clone()),
        b_diffs.iter().cloned().fold(0.0, f64::max)
    );
    println!(
        "  pitch:     median {:.2} cents, worst {:.2} cents  (the piano itself drifts, so this is not error)",
        median(&mut f0_diffs.clone()),
        f0_diffs.iter().cloned().fold(0.0, f64::max)
    );

    // --- the keyboard models ---
    let (Some(model_a), Some(model_b)) = (fit_model(&a.samples), fit_model(&b.samples)) else {
        println!("\nOne of the sessions did not yield a keyboard model.");
        return;
    };
    println!(
        "\nKEYBOARD MODEL\n  break:    A {}   B {}",
        model_a.break_name().unwrap_or_else(|| "none".into()),
        model_b.break_name().unwrap_or_else(|| "none".into())
    );
    println!(
        "  residual: A {:.4}   B {:.4}  log10",
        model_a.rms_log10, model_b.rms_log10
    );

    let model_diffs: Vec<f64> = (1..=KEYS)
        .map(|k| 100.0 * (model_b.b_at(k) - model_a.b_at(k)).abs() / model_a.b_at(k))
        .collect();
    println!(
        "  stiffness across all 88 keys: median {:.1}%, worst {:.1}%",
        median(&mut model_diffs.clone()),
        model_diffs.iter().cloned().fold(0.0, f64::max)
    );

    // --- what actually matters ---
    let cfg = CurveConfig::default();
    let (Some(curve_a), Some(curve_b)) = (solve(&model_a, &cfg), solve(&model_b, &cfg)) else {
        println!("\nCould not solve both curves.");
        return;
    };

    println!("\nTUNING TARGETS  — the number that decides the gate");
    println!("  {:>4} {:>5} {:>10} {:>10} {:>9}", "key", "note", "A cents", "B cents", "apart");
    let mut target_diffs = Vec::new();
    let mut worst = (0.0f64, 0u8);
    for key in 1..=KEYS {
        let d = curve_b.cents_at(key) - curve_a.cents_at(key);
        target_diffs.push(d.abs());
        if d.abs() > worst.0 {
            worst = (d.abs(), key);
        }
        if key % 6 == 1 {
            println!(
                "  {key:>4} {:>5} {:>9.2}\u{a2} {:>9.2}\u{a2} {d:>8.2}\u{a2}",
                key_name(key),
                curve_a.cents_at(key),
                curve_b.cents_at(key)
            );
        }
    }

    let med = median(&mut target_diffs.clone());
    println!(
        "\n  median {med:.2} cents, worst {:.2} cents at {}",
        worst.0,
        key_name(worst.1)
    );

    // A tuner's own repeatability sets the floor here: no algorithm can be asked
    // to agree with itself more closely than the ear it is meant to satisfy.
    let verdict = if worst.0 < 1.0 {
        "the same tuning twice"
    } else if worst.0 < 2.0 {
        "close, within a cent or two at the extremes"
    } else {
        "not yet repeatable"
    };
    println!("  verdict: {verdict}");
}
