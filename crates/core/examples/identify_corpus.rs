//! Grade the key detector against real recorded notes.
//!
//! ```text
//! cargo run -p stiffstring-core --example identify_corpus --release -- <folder> [model folder]
//! ```
//!
//! Measures the model folder (the test folder itself by default), solves its
//! curve exactly as the phone does, then asks the detector to name every note in
//! the test folder in the situations it will meet at the piano:
//!
//! - **from silence**, with no key selected;
//! - **moving on**, with the neighbour or the octave either side selected —
//!   the detector must switch to the key actually struck;
//! - **the same key again**, which must not switch anywhere;
//! - **the string being tuned is far out** — the recording pitched 30 or 60
//!   cents away with its own key selected — which must never send the screen to
//!   a neighbour;
//! - **a neighbour struck slightly out**, which should be followed;
//! - **the last note still ringing** underneath, which must not win.
//!
//! A wrong switch is the failure that matters. Staying put when it could have
//! switched costs the technician one tap.

use std::fs;
use std::path::{Path, PathBuf};

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::detect::{identify, Decision, Keyboard};
use stiffstring_core::inharmonicity::{measure_note, MeasureConfig};
use stiffstring_core::piano::{fit_model, key_name, key_nominal_hz, NoteSample, KEYS};
use stiffstring_core::wav;

/// (folder, key) of recordings known to hold a different note from their name.
/// `grand-at-home-2026-09-04` D#3: every partial is C#3's, and the full
/// measurement cannot measure it as D#3 at all.
const MISFILED: &[(&str, u8)] = &[("grand-at-home-2026-09-04", 31)];

fn cents(from: f64, to: f64) -> f64 {
    1200.0 * (to / from).log2()
}

fn key_from_name(name: &str) -> Option<u8> {
    let rest = name.rsplit_once("-key")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|k| (1..=KEYS).contains(k))
}

struct Recording {
    key: u8,
    audio: Vec<f32>,
    sample_rate: f64,
}

fn load(folder: &str) -> Vec<Recording> {
    let mut files: Vec<(u8, PathBuf)> = fs::read_dir(Path::new(folder))
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
    files
        .into_iter()
        .filter_map(|(key, path)| {
            let audio = wav::decode(&fs::read(path).ok()?).ok()?;
            Some(Recording { key, audio: audio.samples, sample_rate: audio.sample_rate })
        })
        .collect()
}

/// Where the blow lands, the way the meter check finds it.
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
        .map_or(0, |i| (i * window).saturating_sub(window))
}

/// The same recording played `c` cents higher, by resampling.
fn repitch(samples: &[f32], c: f64) -> Vec<f32> {
    let r = 2f64.powf(c / 1200.0);
    let len = ((samples.len() as f64 - 2.0) / r) as usize;
    (0..len)
        .map(|i| {
            let x = i as f64 * r;
            let j = x as usize;
            let f = (x - j as f64) as f32;
            samples[j] * (1.0 - f) + samples[j + 1] * f
        })
        .collect()
}

fn main() {
    let Some(folder) = std::env::args().nth(1) else {
        eprintln!("usage: identify_corpus <folder> [model folder]");
        std::process::exit(2);
    };
    let model_folder = std::env::args().nth(2).unwrap_or_else(|| folder.clone());

    // The phone's own path: measure, fit, solve.
    let model_recs = load(&model_folder);
    let mut samples: Vec<NoteSample> = Vec::new();
    let mut measured: Vec<(u8, f64)> = Vec::new();
    for r in &model_recs {
        if let Some(m) = measure_note(&r.audio, r.sample_rate, key_nominal_hz(r.key, 440.0), MeasureConfig::default()) {
            samples.push(NoteSample::from_measurement(r.key, &m));
            measured.push((r.key, m.f0));
        }
    }
    let model = fit_model(&samples).expect("not enough usable notes to fit a model");
    let curve = solve(&model, &CurveConfig::default()).expect("could not solve a curve");
    let hz: Vec<f64> = (1..=KEYS).map(|k| curve.hz_at(k)).collect();
    let b: Vec<f64> = (1..=KEYS).map(|k| model.b_at(k)).collect();

    // Where each string is believed to sit, as the page works it out: its own
    // measured offset where there is one, the piano's median elsewhere.
    let mut offs: Vec<f64> = measured.iter().map(|&(k, f0)| cents(hz[usize::from(k) - 1], f0)).collect();
    offs.sort_by(f64::total_cmp);
    let median = offs[offs.len() / 2];
    let mut expect = vec![median; 88];
    for &(k, f0) in &measured {
        expect[usize::from(k) - 1] = cents(hz[usize::from(k) - 1], f0);
    }
    let piano = Keyboard { targets_hz: &hz, b: &b, expect_cents: &expect };

    let recs = load(&folder);

    // DEBUG=E1,D#3 prints what the detector saw on those notes.
    if let Ok(list) = std::env::var("DEBUG") {
        for r in recs.iter().filter(|r| list.split(',').any(|n| n == key_name(r.key))) {
            let on = onset_index(&r.audio, r.sample_rate);
            let (peaks, scored) = stiffstring_core::detect::explain(&r.audio, on, r.sample_rate, piano);
            println!("{}  onset at {:.2}s of {:.2}s, {} new peaks", key_name(r.key), on as f64 / r.sample_rate, r.audio.len() as f64 / r.sample_rate, peaks.len());
            let mut strongest = peaks.clone();
            strongest.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (hz, a) in strongest.iter().take(14) {
                println!("    peak {hz:8.1} Hz  {a:.5}");
            }
            for c in scored.iter().take(6) {
                println!("    {:>4} {:+6.1}c  {}/{}  score {:.5}", key_name(c.key), c.cents, c.matched, c.expected, c.score);
            }
            println!("    expected partials of {}: {}", key_name(r.key),
                (1..=8).map(|n| format!("{:.1}", stiffstring_core::synth::partial_hz(hz[usize::from(r.key) - 1], b[usize::from(r.key) - 1], n))).collect::<Vec<_>>().join(" "));
        }
        return;
    }
    println!(
        "KEY DETECTOR AGAINST {} REAL NOTES FROM {folder}\n  model from {model_folder}, piano sits {median:+.1} cents from its targets\n",
        recs.len()
    );
    println!(
        "  {:>5}  {:>10} {:>9} {:>6} {:>7} {:>6}  {:>8} {:>8} {:>8}  {:>8} {:>8}  {:>8}",
        "note", "silence", "cents", "parts", "clear", "same", "from-1", "from+1", "octaves", "far-30", "far-60", "ringing"
    );

    #[derive(Default)]
    struct Tally {
        tried: usize,
        right: usize,
        stayed: usize,
        wrong: usize,
    }
    impl Tally {
        fn add(&mut self, got: Option<u8>, want: u8, stay: Option<u8>) -> &'static str {
            self.tried += 1;
            if got == Some(want) {
                self.right += 1;
                "ok"
            } else if got == stay {
                self.stayed += 1;
                "stay"
            } else {
                self.wrong += 1;
                "WRONG"
            }
        }
    }
    let (mut t_silence, mut t_same, mut t_move, mut t_oct, mut t_far, mut t_ring) =
        (Tally::default(), Tally::default(), Tally::default(), Tally::default(), Tally::default(), Tally::default());
    let mut wrongs: Vec<String> = Vec::new();

    for (idx, r) in recs.iter().enumerate() {
        let k = r.key;
        // Recordings saved under the wrong key, found by this very report and
        // confirmed by the full measurement, which cannot measure them as the
        // key their name claims. Grading the detector against a wrong label
        // would only punish it for being right.
        if MISFILED.iter().any(|&(f, key)| folder.contains(f) && key == k) {
            println!("  {:>5}  misfiled — the recording holds a different note; not graded", key_name(k));
            continue;
        }
        let sr = r.sample_rate;
        let on = onset_index(&r.audio, sr);
        let run = |audio: &[f32], split: usize, cur: Option<u8>| identify(audio, split, sr, piano, cur);

        // From silence.
        let d = run(&r.audio, on, None);
        let s_txt = t_silence.add(d.key_to_tune(None), k, None);
        if s_txt == "WRONG" {
            wrongs.push(format!("{} from silence named {:?}", key_name(k), d.best.map(|c| key_name(c.key))));
        }
        let (c_txt, p_txt, clear_txt) = match d.best {
            Some(c) => (
                format!("{:+.1}", c.cents),
                format!("{}/{}", c.matched, c.expected),
                d.second.map_or("-".into(), |s| format!("{:.0}%", 100.0 * s.score / c.score)),
            ),
            None => ("-".into(), "-".into(), "-".into()),
        };

        // Same key again.
        let d = run(&r.audio, on, Some(k));
        let same_txt = t_same.add(d.key_to_tune(Some(k)), k, Some(k));

        // Moving on from a neighbour, and from an octave.
        let mut mv = Vec::new();
        for cur in [k.checked_sub(1), Some(k + 1)] {
            let Some(cur) = cur.filter(|c| (1..=KEYS).contains(c)) else { mv.push("-"); continue };
            let d = run(&r.audio, on, Some(cur));
            let t = t_move.add(d.key_to_tune(Some(cur)), k, Some(cur));
            if t == "WRONG" {
                wrongs.push(format!("{} from {} went to {:?}", key_name(k), key_name(cur), d.best.map(|c| key_name(c.key))));
            }
            mv.push(t);
        }
        let mut oct = "ok";
        for cur in [k.checked_sub(12), Some(k + 12)] {
            let Some(cur) = cur.filter(|c| (1..=KEYS).contains(c)) else { continue };
            let d = run(&r.audio, on, Some(cur));
            let t = t_oct.add(d.key_to_tune(Some(cur)), k, Some(cur));
            if t == "WRONG" {
                oct = "WRONG";
                wrongs.push(format!("{} from {} went to {:?}", key_name(k), key_name(cur), d.best.map(|c| key_name(c.key))));
            } else if t == "stay" && oct == "ok" {
                oct = "stay";
            }
        }

        // The string being tuned is far out: it must stay, or at worst name itself.
        let mut far = Vec::new();
        for amount in [30.0, 60.0] {
            let mut worst = "ok";
            for sign in [-1.0, 1.0] {
                let audio = repitch(&r.audio, sign * amount);
                let on2 = onset_index(&audio, sr);
                let d = run(&audio, on2, Some(k));
                let t = t_far.add(d.key_to_tune(Some(k)), k, Some(k));
                if t == "WRONG" {
                    worst = "WRONG";
                    wrongs.push(format!("{} at {:+.0} cents went to {:?}", key_name(k), sign * amount, d.best.map(|c| key_name(c.key))));
                }
            }
            far.push(worst);
        }

        // The previous recording still ringing when this one is struck, softer.
        let ring = if idx > 0 {
            let p = &recs[idx - 1];
            let pon = onset_index(&p.audio, sr);
            let at = (sr * 0.8) as usize;
            let len = (p.audio.len() - pon).min(at + (r.audio.len() - on));
            let mut mix: Vec<f32> = p.audio[pon..pon + len].to_vec();
            for (i, s) in r.audio[on..].iter().enumerate() {
                if at + i < mix.len() {
                    mix[at + i] += s * 0.5;
                }
            }
            let d = run(&mix, at, Some(p.key));
            if std::env::var("DEBUGRING").is_ok_and(|v| v == key_name(k)) {
                let (peaks, scored) = stiffstring_core::detect::explain(&mix, at, sr, piano);
                let mut strongest = peaks.clone();
                strongest.sort_by(|a, b| b.1.total_cmp(&a.1));
                println!("  RING {} over {}: {:?}", key_name(k), key_name(p.key), d.decision);
                for (hz, a) in strongest.iter().take(12) { println!("    peak {hz:8.1} Hz  {a:.5}"); }
                for c in scored.iter().take(5) {
                    println!("    {:>4} {:+6.1}c  {}/{}  score {:.5} energy {:.5} fundamental {:.5}", key_name(c.key), c.cents, c.matched, c.expected, c.score, c.energy, c.first_energy);
                }
            }
            let t = t_ring.add(d.key_to_tune(Some(p.key)), k, Some(p.key));
            if t == "WRONG" {
                wrongs.push(format!("{} over ringing {} went to {:?}", key_name(k), key_name(p.key), d.best.map(|c| key_name(c.key))));
            }
            t
        } else {
            "-"
        };

        println!(
            "  {:>5}  {:>10} {:>9} {:>6} {:>7} {:>6}  {:>8} {:>8} {:>8}  {:>8} {:>8}  {:>8}",
            key_name(k), s_txt, c_txt, p_txt, clear_txt, same_txt, mv[0], mv[1], oct, far[0], far[1], ring
        );
        let _ = Decision::Nothing;
    }

    let line = |name: &str, t: &Tally| {
        println!(
            "  {name:<34} {:>3} right  {:>3} stayed  {:>3} WRONG   of {}",
            t.right, t.stayed, t.wrong, t.tried
        );
    };
    println!("\nSUMMARY  (a WRONG is a switch to a key that was not struck)");
    line("named from silence", &t_silence);
    line("same key struck again (stay)", &t_same);
    line("neighbour selected, this key struck", &t_move);
    line("octave selected, this key struck", &t_oct);
    line("string being tuned 30-60 cents out", &t_far);
    line("struck over the last note ringing", &t_ring);
    if !wrongs.is_empty() {
        println!("\nWRONG SWITCHES");
        for w in &wrongs {
            println!("  {w}");
        }
    }
}
