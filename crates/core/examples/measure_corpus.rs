//! Measure a folder of recorded notes from a real piano.
//!
//! ```text
//! cargo run -p stiffstring-core --example measure_corpus --release -- <folder>
//! ```
//!
//! Reads the WAV files the note recorder produces, measures each one, fits
//! inharmonicity across the keyboard, and solves the tuning curve — the whole
//! chain, on a real instrument rather than a synthetic one.
//!
//! The key number comes from the filename (`A2-key25.wav`), so no JSON parser is
//! needed and the manifest stays purely informational.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, NoteSample, KEYS};

/// Minimal WAV reader: mono, 32-bit float or 16-bit integer.
fn read_wav(path: &Path) -> Result<(Vec<f32>, f64), String> {
    let bytes = fs::read(path).map_err(|e| format!("{e}"))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);

    let (mut format, mut channels, mut sample_rate, mut bits) = (0u16, 0u16, 0f64, 0u16);
    let mut samples = Vec::new();
    let mut pos = 12;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        if body + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = u16_at(body);
            channels = u16_at(body + 2);
            sample_rate = f64::from(u32_at(body + 4));
            bits = u16_at(body + 14);
        } else if id == b"data" {
            match (format, bits) {
                (3, 32) => {
                    samples = (0..size / 4)
                        .map(|i| {
                            let o = body + i * 4;
                            f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
                        })
                        .collect();
                }
                (1, 16) => {
                    samples = (0..size / 2)
                        .map(|i| {
                            let o = body + i * 2;
                            f32::from(i16::from_le_bytes([bytes[o], bytes[o + 1]])) / 32768.0
                        })
                        .collect();
                }
                _ => return Err(format!("unsupported format {format}, {bits} bits")),
            }
        }
        pos = body + size + (size & 1); // chunks are padded to even lengths
    }

    if samples.is_empty() {
        return Err("no audio data".into());
    }
    if channels > 1 {
        // Take the left channel rather than mixing: mixing two microphones can
        // partially cancel a partial and quietly ruin a measurement.
        samples = samples.iter().step_by(channels as usize).copied().collect();
    }
    Ok((samples, sample_rate))
}

/// Key number from a filename such as `A2-key25.wav`.
fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

fn cents(from: f64, to: f64) -> f64 {
    1200.0 * (to / from).log2()
}

fn main() {
    let folder = match std::env::args().nth(1) {
        Some(f) => f,
        None => {
            eprintln!("usage: measure_corpus <folder of recorded notes>");
            std::process::exit(2);
        }
    };
    let dir = Path::new(&folder);

    let mut files: Vec<(u8, std::path::PathBuf)> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?;
                Some((key_from_name(name)?, p.clone()))
            })
            .collect(),
        Err(e) => {
            eprintln!("cannot read {folder}: {e}");
            std::process::exit(1);
        }
    };
    files.sort_by_key(|(k, _)| *k);

    if files.is_empty() {
        eprintln!("no recorded notes found in {folder}");
        std::process::exit(1);
    }

    println!("MEASURING {} NOTES FROM {}\n", files.len(), folder);
    println!(
        "  {:>4} {:>5} {:>11} {:>9} {:>11} {:>6} {:>6} {:>6} {:>7}  concerns",
        "key", "note", "measured Hz", "vs ET", "B", "parts", "conf", "rms", "unison"
    );

    let mut samples_out: Vec<NoteSample> = Vec::new();
    let mut pitch_offsets: Vec<f64> = Vec::new();
    let mut failed: Vec<u8> = Vec::new();

    for (key, path) in &files {
        let (audio, sample_rate) = match read_wav(path) {
            Ok(v) => v,
            Err(e) => {
                println!("  {key:>4} {:>5}  unreadable: {e}", key_name(*key));
                failed.push(*key);
                continue;
            }
        };

        let hint = key_nominal_hz(*key, 440.0);
        let Some(m) = measure_note(&audio, sample_rate, hint, MeasureConfig::default()) else {
            println!("  {key:>4} {:>5}  no measurement", key_name(*key));
            failed.push(*key);
            continue;
        };

        let off = cents(hint, m.f0);
        pitch_offsets.push(off);
        let confidence = m.used().map(|p| p.confidence).sum::<f64>() / m.used_count().max(1) as f64;
        let concerns = if m.concerns.is_empty() {
            "-".to_string()
        } else {
            m.concerns
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(",")
        };

        let unison = m
            .unison_spread_cents
            .map_or_else(|| "-".to_string(), |s| format!("{s:.2}\u{a2}"));

        println!(
            "  {key:>4} {:>5} {:>11.3} {off:>8.1}\u{a2} {:>11.3e} {:>6} {:>6.3} {:>5.2}\u{a2} {unison:>7}  {concerns}",
            key_name(*key),
            m.f0,
            m.b,
            m.used_count(),
            confidence,
            m.rms_cents
        );

        samples_out.push(NoteSample::from_measurement(*key, &m));
    }

    if !failed.is_empty() {
        println!(
            "\n  {} note(s) could not be measured: {}",
            failed.len(),
            failed
                .iter()
                .map(|k| key_name(*k))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Where the instrument is actually sitting.
    if !pitch_offsets.is_empty() {
        let mut sorted = pitch_offsets.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        println!(
            "\nPITCH  median {median:+.1} cents from A4=440, i.e. about A4 = {:.2} Hz",
            440.0 * 2f64.powf(median / 1200.0)
        );
        println!(
            "  range {:+.1} to {:+.1} cents across the compass",
            sorted[0],
            sorted[sorted.len() - 1]
        );
    }

    // Inharmonicity across the keyboard.
    let Some(model) = fit_model(&samples_out) else {
        println!("\nNot enough usable notes to fit a keyboard model.");
        return;
    };
    println!(
        "\nINHARMONICITY MODEL  from {} notes, break {}, residual {:.4} log10",
        model.samples,
        model
            .break_name()
            .unwrap_or_else(|| "none found".to_string()),
        model.rms_log10
    );
    if model.samples < samples_out.len() {
        println!(
            "  {} reading(s) discarded as inconsistent with the rest",
            samples_out.len() - model.samples
        );
    }
    print!("  B:");
    for key in [1u8, 13, 25, 37, 49, 61, 73, 85] {
        print!("  {} {:.2e}", key_name(key), model.b_at(key));
    }
    println!();

    // Targets.
    let cfg = CurveConfig::default();
    let Some(curve) = solve(&model, &cfg) else {
        println!("\nCould not solve a curve.");
        return;
    };
    println!(
        "\nTUNING CURVE  A0 {:+.1} cents, A4 {:+.2}, C8 {:+.1}",
        curve.cents_at(1),
        curve.cents_at(49),
        curve.cents_at(88)
    );

    // How far the piano currently sits from those targets, on the notes we heard.
    let measured: BTreeMap<u8, f64> = samples_out.iter().map(|s| (s.key, s.f0)).collect();
    println!("\nHOW FAR THIS PIANO IS FROM ITS OWN TARGETS");
    println!("  {:>4} {:>5} {:>11} {:>11} {:>9}", "key", "note", "now Hz", "target Hz", "off by");
    let mut worst = (0.0f64, 0u8);
    for (key, now) in &measured {
        let target = curve.hz_at(*key);
        let off = cents(target, *now);
        if off.abs() > worst.0 {
            worst = (off.abs(), *key);
        }
        println!(
            "  {key:>4} {:>5} {now:>11.3} {target:>11.3} {off:>8.1}\u{a2}",
            key_name(*key)
        );
    }
    println!(
        "\n  worst {:.1} cents at {}",
        worst.0,
        key_name(worst.1)
    );
}
