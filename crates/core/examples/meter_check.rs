//! Grade the live meter against real recorded notes.
//!
//! ```text
//! cargo run -p stiffstring-core --example meter_check --release -- <folder>
//! ```
//!
//! Measures a folder of recorded notes the ordinary way, solves the tuning
//! curve, and then asks the meter — which sees one partial and a quarter of a
//! second — how far each note is from its own target. The full measurement,
//! which sees every partial and the whole note, has already answered that.
//!
//! Two numbers matter, and they are different questions:
//!
//! - **Agreement.** The meter's reading against the full measurement's. This is
//!   the meter's accuracy, and it is where a wrong partial choice or a wrong
//!   stiffness would show up as a steady offset.
//! - **Steadiness.** The spread of the meter's own readings across successive
//!   overlapping blocks of the same held note. A meter that is accurate on
//!   average but wanders by a cent is useless at the pin.
//!
//! Real recordings, not synthetic ones: every note here has two or three strings
//! beating against each other, room noise, and a decay that a synthetic tone
//! flatters.

use std::fs;
use std::path::Path;
use std::time::Instant;

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::meter::{lock_note, settle, track};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, NoteSample, KEYS};
use stiffstring_core::wav;

/// Length of audio the meter is given per update, and how far it steps between
/// them. A third of a second is long enough for the phase fit to be precise and
/// short enough that the reading follows the string.
const BLOCK_SECONDS: f64 = 0.35;
const HOP_SECONDS: f64 = 0.05;

/// How many consecutive readings the display holds a median over.
const DISPLAY_READINGS: usize = 8;

/// Overridable from the command line, for sweeping these choices rather than
/// arguing about them: `meter_check <folder> [block] [display] [min conf]`.
fn arg(n: usize, fallback: f64) -> f64 {
    std::env::args()
        .nth(n)
        .and_then(|s| s.parse().ok())
        .unwrap_or(fallback)
}

fn cents(from: f64, to: f64) -> f64 {
    1200.0 * (to / from).log2()
}

/// Key number from a filename such as `A2-key25.wav`.
fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

fn main() {
    let Some(folder) = std::env::args().nth(1) else {
        eprintln!("usage: meter_check <folder of recorded notes>");
        std::process::exit(2);
    };

    let block_seconds = arg(2, BLOCK_SECONDS);
    let display_readings = arg(3, DISPLAY_READINGS as f64) as usize;
    let min_confidence = arg(4, 0.0);
    println!(
        "block {block_seconds}s, median of {display_readings}, confidence over {min_confidence}"
    );

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

    // Pass one: the slow, thorough measurement, and the curve it supports.
    struct Note {
        key: u8,
        audio: Vec<f32>,
        sample_rate: f64,
        f0: f64,
        beat_spread: Option<f64>,
        /// Each partial's own measured frequency, by partial number.
        partial_hz: Vec<(u32, f64)>,
    }
    let mut notes: Vec<Note> = Vec::new();
    let mut samples: Vec<NoteSample> = Vec::new();
    for (key, path) in &files {
        let Ok(bytes) = fs::read(path) else { continue };
        let Ok(audio) = wav::decode(&bytes) else {
            continue;
        };
        let hint = key_nominal_hz(*key, 440.0);
        let Some(m) = measure_note(
            &audio.samples,
            audio.sample_rate,
            hint,
            MeasureConfig::default(),
        ) else {
            continue;
        };
        samples.push(NoteSample::from_measurement(*key, &m));
        notes.push(Note {
            key: *key,
            audio: audio.samples,
            sample_rate: audio.sample_rate,
            f0: m.f0,
            beat_spread: m.beat_spread_cents,
            partial_hz: m.partials.iter().map(|p| (p.n, p.hz)).collect(),
        });
    }

    let Some(model) = fit_model(&samples) else {
        eprintln!("not enough usable notes in {folder}");
        std::process::exit(1);
    };
    let Some(curve) = solve(&model, &CurveConfig::default()) else {
        eprintln!("could not solve a curve");
        std::process::exit(1);
    };

    println!("METER AGAINST {} REAL NOTES FROM {folder}\n", notes.len());
    println!(
        "  {:>4} {:>5} {:>7} {:>9} {:>9} {:>9} {:>9} {:>7} {:>7} {:>7} {:>7}",
        "key",
        "note",
        "partial",
        "meter",
        "partial",
        "whole",
        "vs part",
        "raw",
        "shown",
        "drift",
        "beat"
    );

    let mut worst_agreement = (0.0f64, 0u8);
    let mut worst_steadiness = (0.0f64, 0u8);
    let mut agreements: Vec<f64> = Vec::new();
    let mut whole_note_gaps: Vec<f64> = Vec::new();
    let mut steadiness: Vec<f64> = Vec::new();
    let mut drifts: Vec<f64> = Vec::new();
    let mut slowest_ms = 0.0f64;
    let mut unlocked: Vec<u8> = Vec::new();

    for note in &notes {
        let key = note.key;
        let target = curve.hz_at(key);
        let b = model.b_at(key);
        let sample_rate = note.sample_rate;
        let audio = &note.audio;
        // What the full measurement says about the whole note.
        let whole = cents(target, note.f0);

        let block = (sample_rate * block_seconds) as usize;
        let hop = (sample_rate * HOP_SECONDS) as usize;
        if audio.len() < block {
            continue;
        }

        // Start where the note starts, not where the file does: these
        // recordings carry up to a second of room before the strike.
        let onset = onset_index(audio, sample_rate);

        let Some(lock) = lock_note(
            &audio[onset..(onset + block).min(audio.len())],
            sample_rate,
            target,
            b,
        ) else {
            println!("  {key:>4} {:>5}   did not lock", key_name(key));
            unlocked.push(key);
            continue;
        };

        let mut readings = Vec::new();
        let mut previous = lock.hz;
        let mut start = onset;
        let began = Instant::now();
        // Only the first second and a bit of the note. The live meter never sees
        // more than that: the technician strikes again long before a note's tail
        // has faded, so grading against a tail nobody meters would be grading
        // the wrong thing.
        let last = (onset + (sample_rate * 1.2) as usize).min(audio.len());
        while start + block <= last {
            if let Some(r) = track(
                &audio[start..start + block],
                sample_rate,
                target,
                b,
                lock.partial,
                previous,
            ) {
                previous = r.hz;
                if r.confidence >= min_confidence {
                    readings.push(r);
                }
            }
            start += hop;
        }
        if readings.is_empty() {
            println!(
                "  {key:>4} {:>5}   locked but tracked nothing",
                key_name(key)
            );
            unlocked.push(key);
            continue;
        }
        let per_read_ms = began.elapsed().as_secs_f64() * 1000.0 / readings.len() as f64;
        slowest_ms = slowest_ms.max(per_read_ms);

        let raw: Vec<f64> = readings.iter().map(|r| r.cents).collect();
        let mean = raw.iter().sum::<f64>() / raw.len() as f64;
        let raw_spread = raw.iter().cloned().fold(f64::MIN, f64::max)
            - raw.iter().cloned().fold(f64::MAX, f64::min);

        // What the technician would actually see: the page holds a rolling
        // window of readings and shows what `settle` makes of them.
        let shown: Vec<f64> = readings
            .windows(display_readings.max(1))
            .filter_map(|w| settle(w).map(|s| s.cents))
            .collect();
        let spread = if shown.is_empty() {
            raw_spread
        } else {
            shown.iter().cloned().fold(f64::MIN, f64::max)
                - shown.iter().cloned().fold(f64::MAX, f64::min)
        };

        // The apples-to-apples comparison: the full measurement's own reading of
        // the very partial the meter chose to follow. Anything left over is the
        // meter's error; the gap between that and `whole` is what the stiffness
        // model costs when it converts one partial into a note.
        let same_partial = note
            .partial_hz
            .iter()
            .find(|(n, _)| *n == lock.partial)
            .map(|(_, hz)| cents(lock.target_hz, *hz));

        if let Some(part) = same_partial {
            agreements.push((mean - part).abs());
            whole_note_gaps.push((part - whole).abs());
            if (mean - part).abs() > worst_agreement.0 {
                worst_agreement = ((mean - part).abs(), key);
            }
        }
        steadiness.push(spread);
        if spread > worst_steadiness.0 {
            worst_steadiness = (spread, key);
        }

        // Does the note itself move as it decays, or is the spread just wobble?
        let drift = if shown.len() >= 2 {
            shown[shown.len() - 1] - shown[0]
        } else {
            0.0
        };
        drifts.push(drift);
        let beat_txt = note
            .beat_spread
            .map_or_else(|| "-".to_string(), |b| format!("{b:.2}c"));
        let part_txt = same_partial.map_or("      -".to_string(), |c| format!("{c:8.2}\u{a2}"));
        let gap_txt =
            same_partial.map_or("      -".to_string(), |c| format!("{:8.2}\u{a2}", mean - c));
        println!(
            "  {key:>4} {:>5} {:>7} {mean:>8.2}\u{a2} {part_txt} {whole:>8.2}\u{a2} {gap_txt} {raw_spread:>7.2}\u{a2}{spread:>7.2}\u{a2}{drift:>7.2}\u{a2} {beat_txt:>7}",
            key_name(key),
            lock.partial
        );
        let _ = per_read_ms;
    }

    let median = |mut v: Vec<f64>| {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };

    println!(
        "\nMETER AGAINST THE SAME PARTIAL, MEASURED PROPERLY  (this is the meter's own error)"
    );
    println!("  median {:.2} cents", median(agreements.clone()));
    println!(
        "  worst  {:.2} cents at {}",
        worst_agreement.0,
        key_name(worst_agreement.1)
    );
    println!("\nCOST OF TURNING ONE PARTIAL INTO A NOTE  (the stiffness model, not the meter)");
    println!("  median {:.2} cents", median(whole_note_gaps.clone()));
    println!("\nSTEADINESS ON A HELD NOTE  (target: under 0.20 cents)");
    println!("  median spread {:.2} cents", median(steadiness.clone()));
    println!(
        "  worst  spread {:.2} cents at {}",
        worst_steadiness.0,
        key_name(worst_steadiness.1)
    );
    // Almost always negative, and worth knowing: a piano string is sharper
    // while it is loud and falls as it decays. The technician's ear hears this
    // too, which is why pitch is judged at a consistent moment after the strike.
    println!("\nDRIFT OVER THE FIRST SECOND  (the string, not the meter)");
    println!("  median {:+.2} cents", median(drifts.clone()));
    println!("\nSPEED  slowest {slowest_ms:.1} ms per update (native; the phone is slower)");
    if !unlocked.is_empty() {
        println!(
            "\n  {} note(s) the meter would not read: {}",
            unlocked.len(),
            unlocked
                .iter()
                .map(|k| key_name(*k))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Where the note starts, so the meter is not handed a block of empty room.
///
/// The recorder captures a pre-roll before the strike; the phone's live meter
/// never sees that, so neither should this.
fn onset_index(samples: &[f32], sample_rate: f64) -> usize {
    let window = (sample_rate * 0.01) as usize;
    if window == 0 || samples.len() < window * 4 {
        return 0;
    }
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let threshold = peak * 0.25;
    samples
        .chunks(window)
        .position(|c| c.iter().any(|s| s.abs() >= threshold))
        .map_or(0, |i| i * window)
}
