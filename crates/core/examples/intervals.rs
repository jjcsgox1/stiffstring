//! What the intervals of a real tuning actually beat at.
//!
//! ```text
//! cargo run -p stiffstring-core --example intervals --release -- <folder>
//! ```
//!
//! Every other report in this project grades the engine. This one grades the
//! *tuning*: it takes a recording of a piano someone has tuned, measures each
//! note's fundamental and stiffness, and works out how fast the coincident
//! partials of each interval are beating against each other — the thing the
//! technician was listening to while setting the pins.
//!
//! Beside it, the same intervals as the solver's own targets would have laid
//! them. Where the two agree, the solver has reproduced the tuning. Where they
//! differ, the difference is in beats per second rather than in cents, which is
//! the unit the disagreement can actually be argued about.
//!
//! The recorder's note set is spread too widely for thirds outside the bass, so
//! what is available here is octaves and double octaves. That is the right
//! material anyway: outside the temperament, octaves are what carries a tuning.

use std::fs;
use std::path::Path;

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, NoteSample, KEYS};
use stiffstring_core::synth::partial_hz;
use stiffstring_core::wav;

/// A note as the piano actually sounds it.
struct Sounding {
    key: u8,
    f0: f64,
    b: f64,
}

fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

/// Beat rate between partial `m` of the lower note and partial `n` of the upper,
/// signed: positive means the upper note is sharp of where that partial pair
/// would be silent, which is a wide interval.
fn beat(low: &Sounding, m: u32, high: &Sounding, n: u32) -> f64 {
    partial_hz(high.f0, high.b, n) - partial_hz(low.f0, low.b, m)
}

fn main() {
    let Some(folder) = std::env::args().nth(1) else {
        eprintln!("usage: intervals <folder of recorded notes>");
        std::process::exit(2);
    };

    let mut files: Vec<(u8, std::path::PathBuf)> = fs::read_dir(Path::new(&folder))
        .unwrap_or_else(|e| {
            eprintln!("cannot read {folder}: {e}");
            std::process::exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
        .filter_map(|p| Some((key_from_name(p.file_name()?.to_str()?)?, p.clone())))
        .collect();
    files.sort_by_key(|(k, _)| *k);

    let mut heard: Vec<Sounding> = Vec::new();
    let mut samples: Vec<NoteSample> = Vec::new();
    for (key, path) in &files {
        let Ok(bytes) = fs::read(path) else { continue };
        let Ok(a) = wav::decode(&bytes) else { continue };
        let hint = key_nominal_hz(*key, 440.0);
        let Some(m) = measure_note(&a.samples, a.sample_rate, hint, MeasureConfig::default()) else {
            continue;
        };
        samples.push(NoteSample::from_measurement(*key, &m));
        heard.push(Sounding {
            key: *key,
            f0: m.f0,
            b: m.b,
        });
    }
    if heard.len() < 4 {
        eprintln!("too few notes measured in {folder}");
        std::process::exit(1);
    }

    // What the solver would have asked for, at a stated reference pitch.
    //
    // The default is the pitch the piano is sitting at, but it is overridable
    // and needs to be: on a piano only partly tuned, the median is dragged by
    // whichever section was left alone, and every note that *was* tuned then
    // looks uniformly sharp for a reason that has nothing to do with the tuning.
    // Interval beats barely care — a few cents of anchor scales them by a
    // fraction of a percent — but the note-by-note table cares completely.
    let mut offsets: Vec<f64> = heard
        .iter()
        .map(|s| 1200.0 * (s.f0 / key_nominal_hz(s.key, 440.0)).log2())
        .collect();
    offsets.sort_by(f64::total_cmp);
    let a4 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| 440.0 * 2f64.powf(offsets[offsets.len() / 2] / 1200.0));

    let model = fit_model(&samples).expect("a keyboard model");
    let curve = solve(
        &model,
        &CurveConfig {
            a4_hz: a4,
            ..CurveConfig::default()
        },
    )
    .expect("a curve");
    let target = |key: u8| Sounding {
        key,
        f0: curve.hz_at(key),
        b: model.b_at(key),
    };

    println!("INTERVALS IN {folder}");
    println!("  {} notes measured, compared against the solver's own targets at A4 = {a4:.2} Hz\n", heard.len());

    for (span, label, pairs) in [
        (12u8, "OCTAVES", &[(2u32, 1u32), (4, 2), (6, 3)][..]),
        (24, "DOUBLE OCTAVES", &[(4, 1), (8, 2)][..]),
    ] {
        println!("{label}   beats per second, + is wide");
        print!("  {:>9} {:>8}", "interval", "width");
        for (m, n) in pairs {
            print!("  {:>8}", format!("{m}:{n}"));
        }
        print!("   |");
        for (m, n) in pairs {
            print!("  {:>8}", format!("want {m}:{n}"));
        }
        println!();

        for low in &heard {
            let Some(high) = heard.iter().find(|h| h.key == low.key + span) else {
                continue;
            };
            // How far the interval is from a pure frequency ratio, in cents.
            let pure = 100.0 * f64::from(span);
            let width = 1200.0 * (high.f0 / low.f0).log2() - pure;
            print!(
                "  {:>4}{:>5} {width:>+7.2}\u{a2}",
                key_name(low.key),
                key_name(high.key)
            );
            for (m, n) in pairs {
                print!("  {:>+8.2}", beat(low, *m, high, *n));
            }
            print!("   |");
            let (tl, th) = (target(low.key), target(high.key));
            for (m, n) in pairs {
                print!("  {:>+8.2}", beat(&tl, *m, &th, *n));
            }
            println!();
        }
        println!();
    }

    // Where the tuning sits against the targets, note by note, so the interval
    // table above can be read alongside the pitches that produced it.
    println!("EACH NOTE AGAINST ITS TARGET");
    println!("  {:>5} {:>10} {:>10} {:>9}", "note", "now Hz", "target Hz", "off by");
    let mut worst = (0.0f64, 0u8);
    for s in &heard {
        let t = curve.hz_at(s.key);
        let off = 1200.0 * (s.f0 / t).log2();
        if off.abs() > worst.0 {
            worst = (off.abs(), s.key);
        }
        println!(
            "  {:>5} {:>10.3} {t:>10.3} {off:>+8.2}\u{a2}",
            key_name(s.key),
            s.f0
        );
    }
    println!("\n  worst {:.2} cents at {}", worst.0, key_name(worst.1));
}
