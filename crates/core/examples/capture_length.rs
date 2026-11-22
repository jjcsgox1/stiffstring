//! How short can a capture be? Measure every recorded note from only its first
//! part and compare with the whole recording.
//!
//! ```text
//! cargo run -p stiffstring-core --example capture_length --release -- corpus/<folder>
//! ```
//!
//! Each recording is cut to start just before its loudest blow, as the app's
//! captures do, so its first N seconds are what the app would have heard in a
//! capture N seconds long. A shorter capture is a faster measuring pass; this says, by
//! register, how much is lost for it.

use std::fs;
use std::path::Path;

use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::piano::{key_name, key_nominal_hz, KEYS};

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

fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

/// The stiffness to lend a note too high to show its own: a straight line
/// through log B of the measured notes just below it, as the app does.
fn lend_b(below: &[(u8, f64)], key: u8) -> Option<f64> {
    let pts: Vec<(f64, f64)> = below
        .iter()
        .filter(|(k, b)| *k < key && *k + 30 >= key && *b > 0.0)
        .map(|(k, b)| (f64::from(*k), b.ln()))
        .collect();
    if pts.len() < 3 {
        return below.iter().rev().find(|(k, _)| *k < key).map(|(_, b)| *b);
    }
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    Some((my + slope * (f64::from(key) - mx)).exp())
}

const LENGTHS: [f64; 6] = [0.6, 0.9, 1.2, 1.6, 2.0, 2.6];

fn main() {
    let folder = std::env::args().nth(1).expect("usage: capture_length <folder>");
    let mut files: Vec<(u8, std::path::PathBuf)> = fs::read_dir(&folder)
        .expect("cannot read folder")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
        .filter_map(|p| Some((key_from_name(p.file_name()?.to_str()?)?, p.clone())))
        .collect();
    files.sort_by_key(|(k, _)| *k);

    print!("  {:>4} {:>5} {:>6} {:>11}", "key", "note", "file s", "full B");
    for l in LENGTHS {
        print!(" {:>15}", format!("{l:.1}s cents/B%"));
    }
    println!();
    let mut measured: Vec<(u8, f64)> = Vec::new();
    for (key, path) in &files {
        let Ok((whole, sr)) = read_wav(path) else { continue };
        // Start where the app's capture would: just before the blow. Some
        // recordings hold a quieter strike or a second or two of room first.
        let peak = whole.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let onset = whole.iter().position(|x| x.abs() > peak * 0.2).unwrap_or(0);
        let start = onset.saturating_sub((0.15 * sr) as usize);
        let audio = &whole[start..];
        let hint = key_nominal_hz(*key, 440.0);
        let mut cfg = MeasureConfig::default();
        let mut full = measure_note(audio, sr, hint, cfg);
        let mut lent = false;
        if full.is_none() {
            if let Some(b) = lend_b(&measured, *key) {
                cfg.b_hint = Some(b);
                full = measure_note(audio, sr, hint, cfg);
                lent = full.is_some();
            }
        }
        print!("  {key:>4} {:>5} {:>6.2}", key_name(*key), audio.len() as f64 / sr);
        let Some(full) = full else {
            println!(" {:>11}", "FAILED");
            continue;
        };
        measured.push((*key, full.b));
        print!(" {:>10.3e}{}", full.b, if lent { "*" } else { " " });
        for l in LENGTHS {
            let n = ((l * sr) as usize).min(audio.len());
            let cell = match measure_note(&audio[..n], sr, hint, cfg) {
                None => "fail".to_string(),
                Some(m) => {
                    let c = 1200.0 * (m.f0 / full.f0).log2();
                    let bp = 100.0 * (m.b / full.b - 1.0);
                    format!("{c:+.2} {bp:+.0}%")
                }
            };
            print!(" {cell:>15}");
        }
        println!();
    }
    println!("
  * stiffness lent from the notes below; the note could not show its own");
}
