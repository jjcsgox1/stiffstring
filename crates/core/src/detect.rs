//! Which key was just struck.
//!
//! # Why this is not a pitch detector
//!
//! A general-purpose tuner asks "what note is this?" of every block of audio,
//! from nothing. On a piano that question is genuinely hard — in the bass the
//! fundamental is absent, every note shares half its partials with the octave
//! below, and other strings ring on for the better part of a minute — and the
//! failures it produces are confident ones, which is the one kind this project
//! cannot afford.
//!
//! Here the question is much smaller. The piano has been measured, so every one
//! of its 88 keys has a known target and a known stiffness, and therefore a known
//! set of partial frequencies. The only question is **which of these 88 known
//! patterns best explains what has just started sounding**. That is a choice
//! among candidates, not an open-ended search.
//!
//! # Only what is new
//!
//! A bass note struck a moment ago is still the loudest thing in the room when
//! the next key goes down. Scoring the raw spectrum would hand the answer to the
//! previous note. So the spectrum just after the blow is compared with the one
//! just before it, and only components that grew are considered. The string
//! that was hit grows; everything else is decaying.
//!
//! # Three ways to name the wrong key, and what stops each
//!
//! - **The octave above** has partials that are exactly our even partials, so
//!   every one of them is present. It loses because it explains only half the
//!   energy: our odd partials are left over.
//! - **The octave below** has every one of our partials among its own, so it
//!   explains all the energy. It loses because half of *its* partials — its odd
//!   ones — are missing. A key is only credited with partials that come without
//!   gaps beneath them.
//! - **The neighbouring key** is the one physics cannot rule out: a string sixty
//!   cents flat sounds exactly like the key below it forty cents sharp, and the
//!   neighbour's stiffness is too close to ours to tell them apart. So a reading
//!   that leans this way is never taken as a reason to leave the key being
//!   tuned. The technician taps the key if they really meant the neighbour.
//!
//! And when two unrelated keys score about the same, nothing is decided at all.
//! Staying on the current note costs a tap; jumping to the wrong one would show
//! a confident number for a string nobody is touching.

use crate::estimate::cents;
use crate::fft::{amplitudes, bin_hz, parabolic_peak, spectrum, window, Window};
use crate::synth::partial_hz;

/// Longest analysis frame. At 48 kHz this is a third of a second and bins under
/// 3 Hz wide — fine enough to keep the partials of a bass note, a fundamental's
/// width apart, from running into each other.
const MAX_FRAME: usize = 16_384;
/// Shortest frame worth deciding anything on.
const MIN_FRAME: usize = 4096;

/// Partials below this are not looked for. The bottom octave's fundamentals are
/// absent from the string as often as from the microphone.
const MIN_HZ: f64 = 70.0;
/// Partials above this are not looked for. Higher than the meter's limit,
/// because naming a key needs only where a partial is, not its phase, and the
/// top octave has too few partials below the meter's limit to corroborate.
const MAX_HZ: f64 = 9000.0;
/// At most this many partials per key are scored, lowest first.
const MAX_PARTIALS: usize = 10;

/// How far from where a key's partial is expected a peak may sit and still be
/// counted as that partial. Under half a semitone, so that a key's window never
/// reaches its neighbour's in-tune position.
const WINDOW_CENTS: f64 = 45.0;
/// How closely a key's matched partials must agree with each other. Loose for
/// the same reason as the meter's: stiffness error grows with partial number.
const AGREE_CENTS: f64 = 15.0;

/// A peak must stand this far above the noise floor to be considered.
const NOISE_MARGIN: f64 = 5.0;
/// ...and this far above the spectrum immediately around it. The hammer and
/// the action make a thump at every blow: a dense spread of small new peaks,
/// low in the spectrum, that a weak treble note can be outvoted by. A real
/// partial stands clear of its neighbourhood; the thump does not.
const PROMINENCE: f64 = 4.0;
/// Bins either side that make up a peak's neighbourhood.
const NEIGHBOURHOOD: usize = 40;
/// Peaks weaker than this share of the strongest new peak are ignored.
const RELATIVE_FLOOR: f64 = 0.01;
/// Share of a peak's amplitude that must be new since just before the blow.
/// At one half, a component must have at least doubled to count as struck.
const NEW_SHARE: f64 = 0.5;

/// A key needs at least this many agreeing partials to be named at all.
const MIN_MATCHED: usize = 2;
/// If the best and second-best keys score within this ratio, nothing is
/// decided.
const AMBIGUOUS: f64 = 0.7;
/// A neighbouring key's reading leaning this far back towards the current key
/// is taken as the current string being far out, not as the neighbour struck.
const LEAN_CENTS: f64 = 25.0;

/// The winner must account for its partials this completely...
const MIN_COMPLETENESS: f64 = 0.6;
/// ...and for this share of all the new energy. A string far enough out to sit
/// between two keys fits neither well, and some unrelated key then wins on a
/// scrap of agreement. Better to decide nothing.
const MIN_SHARE: f64 = 0.3;

/// Share of a top-register key's matched energy its fundamental must carry for
/// the key itself to be believed.
///
/// Above about F#6 the strings have no dampers, so striking a top note sets
/// lower strings ringing in sympathy wherever one of their partials coincides
/// with it — the octave below at its second partial, the twelfth below at its
/// third. Such a lower key then matches the struck note perfectly, plus a
/// whisper at its own fundamental. On real recordings A7 was handed to D6 and C8
/// to F6 this way. A genuinely struck top note is mostly fundamental.
const FUNDAMENTAL_SHARE: f64 = 0.3;
/// Lowest key the rule above applies to. Dampers stop somewhere around E6 to
/// F#6 depending on the piano, and lower down a struck note's second partial
/// is often stronger than its first — A3 on these recordings — so the rule
/// would be wrong there.
const UNDAMPED_FROM: u8 = 60;
/// Distances, in keys, at which a lower string has a partial on a higher
/// key's fundamental: octave, twelfth, two octaves, two octaves and a third,
/// two octaves and a fifth, three octaves.
const SHARED: [u8; 6] = [12, 19, 24, 28, 31, 36];

/// What the detector concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing new started sounding. Stay where you are.
    Nothing,
    /// The key already being tuned was struck.
    Same,
    /// A different key was struck: switch to it.
    Switch,
    /// A neighbour of the current key fits, but leans back towards the current
    /// one by enough that it is more likely the current string, far out. Stay.
    StayNeighbour,
    /// Two keys fit about equally well. Stay.
    Unsure,
}

/// How well one key explains the new sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub key: u8,
    /// How far the struck string sits from this key's *target*, in cents,
    /// sharp positive.
    pub cents: f64,
    /// Agreeing partials found.
    pub matched: usize,
    /// Partials expected up to the highest one found.
    pub expected: usize,
    pub score: f64,
    /// Amplitude of the agreeing partials.
    pub energy: f64,
    /// The part of `energy` at the fundamental, when the fundamental was found.
    pub first_energy: f64,
}

/// The answer, with enough of the reasoning to grade it by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub decision: Decision,
    /// The best-fitting key, when any key fitted at all.
    pub best: Option<Candidate>,
    /// The best key unrelated to it, for judging how clear the answer was.
    pub second: Option<Candidate>,
}

impl Detection {
    fn nothing() -> Self {
        Detection { decision: Decision::Nothing, best: None, second: None }
    }

    /// The key to tune now, given the one that was being tuned.
    pub fn key_to_tune(&self, current: Option<u8>) -> Option<u8> {
        match self.decision {
            Decision::Switch => self.best.map(|c| c.key),
            _ => current,
        }
    }
}

/// What the piano is known to be, key by key.
///
/// `targets_hz` and `b` come from the solved curve. `expect_cents` is where each
/// string is believed to sit right now relative to its target: zero for a note
/// already tuned, its measured offset for one measured but not yet tuned, and
/// the piano's overall offset for the rest. It centres each key's search, so a
/// piano forty cents flat throughout is matched as readily as one in tune.
#[derive(Clone, Copy, Debug)]
pub struct Keyboard<'a> {
    pub targets_hz: &'a [f64],
    pub b: &'a [f64],
    pub expect_cents: &'a [f64],
}

/// A component of the spectrum that grew when the key went down.
#[derive(Clone, Copy, Debug)]
struct Peak {
    hz: f64,
    /// The part of its amplitude that is new.
    amplitude: f64,
}

#[inline]
fn floor_pow2(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1usize << (usize::BITS - 1 - n.leading_zeros()) as usize
    }
}

/// Every key's score, best first, for diagnosing a wrong answer.
pub fn explain(samples: &[f32], split: usize, sample_rate: f64, piano: Keyboard) -> (Vec<(f64, f64)>, Vec<Candidate>) {
    let keys = piano.targets_hz.len().min(piano.b.len()).min(piano.expect_cents.len());
    let after = &samples[split.min(samples.len())..];
    let n = floor_pow2(after.len()).min(MAX_FRAME);
    if n < MIN_FRAME {
        return (Vec::new(), Vec::new());
    }
    let h = heard(&samples[..split], &after[..n], sample_rate);
    let mut scored: Vec<Candidate> = (0..keys).filter_map(|i| score_key(&h, piano, i)).collect();
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    (h.peaks.iter().map(|p| (p.hz, p.amplitude)).collect(), scored)
}

/// Name the key struck at `split`.
///
/// `samples[..split]` is the audio just before the blow and `samples[split..]`
/// the audio from the blow on; a third of a second of each is ideal, and less
/// before it is fine. `current` is the key being tuned, if any.
pub fn identify(
    samples: &[f32],
    split: usize,
    sample_rate: f64,
    piano: Keyboard,
    current: Option<u8>,
) -> Detection {
    let keys = piano.targets_hz.len().min(piano.b.len()).min(piano.expect_cents.len());
    if keys == 0 || split > samples.len() || sample_rate.is_nan() || sample_rate <= 0.0 {
        return Detection::nothing();
    }
    let after = &samples[split..];
    let n = floor_pow2(after.len()).min(MAX_FRAME);
    if n < MIN_FRAME {
        return Detection::nothing();
    }

    let h = heard(&samples[..split], &after[..n], sample_rate);
    let peaks = &h.peaks;
    if peaks.len() < MIN_MATCHED {
        return Detection::nothing();
    }

    let mut scored: Vec<Candidate> = (0..keys)
        .filter_map(|i| score_key(&h, piano, i))
        .filter(|c| c.matched >= MIN_MATCHED)
        .collect();
    if scored.is_empty() {
        return Detection::nothing();
    }
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut best = scored[0];

    // A top-register key with a silent fundamental is a string ringing in
    // sympathy with a higher one. Take the higher one if it fits; otherwise
    // decide nothing.
    let sounds_itself = |c: &Candidate| c.first_energy >= c.energy * FUNDAMENTAL_SHARE;
    if best.key >= UNDAMPED_FROM && !sounds_itself(&best) {
        let lower = best.key;
        let up = scored
            .iter()
            .filter(|c| c.key > lower && SHARED.contains(&(c.key - lower)))
            .filter(|c| c.matched >= MIN_MATCHED && completeness(c) >= 0.75 && sounds_itself(c))
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .copied();
        match up {
            Some(u) => best = u,
            None => return Detection { decision: Decision::Nothing, best: Some(best), second: None },
        }
    }

    // Nothing fits well enough to act on: decide nothing at all.
    let total: f64 = peaks.iter().map(|p| p.amplitude).sum();
    if completeness(&best) < MIN_COMPLETENESS || best.energy < total * MIN_SHARE {
        return Detection { decision: Decision::Nothing, best: Some(best), second: None };
    }

    // The runner-up that matters is a key that is *not* an octave, twelfth or
    // double octave of the best one. Those always score something, because they
    // share partials by construction, and the scoring above is what separates
    // them; their score says nothing about whether the answer is clear.
    let related = |a: u8, b: u8| matches!(a.abs_diff(b), 12 | 19 | 24 | 28 | 31 | 36);
    let second = scored
        .iter()
        .find(|c| c.key != best.key && !related(c.key, best.key))
        .copied();

    let mut out = Detection { decision: Decision::Switch, best: Some(best), second };
    if let Some(s) = second {
        if s.score >= best.score * AMBIGUOUS {
            out.decision = Decision::Unsure;
            return out;
        }
    }

    let Some(cur) = current else { return out };
    if best.key == cur {
        out.decision = Decision::Same;
        return out;
    }
    // A neighbour, or a neighbour's octave: a string far out between two keys
    // fits the octaves of the key beside it about as well as that key itself,
    // since they share its partials.
    let step = (i32::from(best.key) - i32::from(cur)).rem_euclid(12);
    if step == 1 || step == 11 {
        // How far the reading leans back towards the key being tuned, relative
        // to where that string was believed to be. Step 11 is a key whose pitch
        // class sits just below the current one; leaning sharp leans back up.
        let i = usize::from(best.key - 1);
        let rel = best.cents - piano.expect_cents[i];
        let lean = if step == 11 { rel } else { -rel };
        if lean >= LEAN_CENTS {
            out.decision = Decision::StayNeighbour;
        }
    }
    out
}

fn completeness(c: &Candidate) -> f64 {
    c.matched as f64 / c.expected.max(1) as f64
}

/// What the spectrum after the blow holds: the peaks that grew, and the
/// frequencies of those that were already sounding before it.
///
/// The second list matters. A partial of the struck note that coincides with
/// one of a note still ringing cannot be told apart from it, so it is neither
/// found nor missing — it is hidden. Counting it missing handed D#2, struck over
/// a ringing C2 whose sixth partial sits on D#2's fifth, to the octave above.
struct Heard {
    peaks: Vec<Peak>,
    hidden: Vec<f64>,
}

/// Peaks of the spectrum after the blow that grew since just before it, and
/// what was already sounding.
fn heard(before: &[f32], after: &[f32], sample_rate: f64) -> Heard {
    let n = after.len();
    let w = window(Window::BlackmanHarris, n);
    let amps = amplitudes(&spectrum(after, &w), &w);
    // Whatever was there before. With less than a frame of history the gap is
    // silence, which is what it was as far as anyone can tell.
    let prior: Vec<f64> = if before.len() >= n {
        amplitudes(&spectrum(&before[before.len() - n..], &w), &w)
    } else {
        let mut padded = vec![0.0f32; n - before.len()];
        padded.extend_from_slice(before);
        amplitudes(&spectrum(&padded, &w), &w)
    };

    let mut sorted = amps.clone();
    sorted.sort_by(f64::total_cmp);
    let floor = sorted[sorted.len() / 2].max(1e-12);
    let bin = bin_hz(sample_rate, n);
    let top = (MAX_HZ.min(sample_rate * 0.45) / bin) as usize;
    let lo = ((MIN_HZ * 0.97 / bin) as usize).max(1);

    let mut peaks = Vec::new();
    let mut hidden = Vec::new();
    for i in lo..top.min(amps.len().saturating_sub(1)) {
        let a = amps[i];
        if a <= floor * NOISE_MARGIN || a <= amps[i - 1] || a < amps[i + 1] {
            continue;
        }
        // The prior frame's level here, generously: its own peak may sit a bin
        // either side after a small change of pitch.
        let was = prior[i - 1].max(prior[i]).max(prior[i + 1]);
        let fresh = a - was;
        if fresh < a * NEW_SHARE {
            hidden.push(i as f64 * bin);
            continue;
        }
        // Standing clear of its own neighbourhood, not merely of the room.
        let from = i.saturating_sub(NEIGHBOURHOOD).max(1);
        let to = (i + NEIGHBOURHOOD).min(amps.len() - 1);
        let mut local: Vec<f64> = amps[from..to].to_vec();
        local.sort_by(f64::total_cmp);
        if a < local[local.len() / 2] * PROMINENCE {
            continue;
        }
        let (offset, amplitude) = parabolic_peak(amps[i - 1], a, amps[i + 1]);
        peaks.push(Peak {
            hz: (i as f64 + offset) * bin,
            amplitude: amplitude * fresh / a,
        });
    }
    let strongest = peaks.iter().fold(0.0f64, |m, p| m.max(p.amplitude));
    peaks.retain(|p| p.amplitude >= strongest * RELATIVE_FLOOR);
    Heard { peaks, hidden }
}

/// How well key `i` (zero-based) explains the new peaks.
fn score_key(h: &Heard, piano: Keyboard, i: usize) -> Option<Candidate> {
    let peaks = &h.peaks;
    let target = piano.targets_hz[i];
    let b = piano.b[i];
    if target.is_nan() || target <= 0.0 || !b.is_finite() || b < 0.0 {
        return None;
    }
    let expect = piano.expect_cents[i];
    let centre_f0 = target * 2f64.powf(expect / 1200.0);

    // (partial number, offset from expected in cents, amplitude) for each
    // expected partial that has a peak inside its window.
    let mut expected: Vec<u32> = Vec::new();
    let mut hidden: Vec<u32> = Vec::new();
    let mut found: Vec<(u32, f64, f64)> = Vec::new();
    let mut partial = 1u32;
    while expected.len() < MAX_PARTIALS && partial <= 40 {
        let want = partial_hz(centre_f0, b, partial);
        if want > MAX_HZ {
            break;
        }
        if want >= MIN_HZ {
            if let Some((hz, amp)) = nearest(peaks, want, WINDOW_CENTS) {
                expected.push(partial);
                found.push((partial, cents(want, hz), amp));
            } else if hidden_near(&h.hidden, want, WINDOW_CENTS) {
                hidden.push(partial);
            } else {
                expected.push(partial);
            }
        }
        partial += 1;
    }
    if found.is_empty() {
        return None;
    }

    // The offset most of the found partials agree on, weighted by strength. A
    // mis-pairing's partials scatter; a real note's line up.
    let consensus = found
        .iter()
        .map(|&(_, c, _)| {
            let weight: f64 = found
                .iter()
                .filter(|&&(_, o, _)| (o - c).abs() <= AGREE_CENTS)
                .map(|&(_, _, a)| a)
                .sum();
            (c, weight)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)?;
    let agreed: Vec<&(u32, f64, f64)> = found
        .iter()
        .filter(|&&(_, o, _)| (o - consensus).abs() <= AGREE_CENTS)
        .collect();

    let matched = agreed.len();
    let highest = agreed.iter().map(|&&(p, _, _)| p).max()?;
    // Credit only for partials that come without gaps beneath them: the
    // octave below has all of ours among its own, and would otherwise win.
    let gaps_counted = expected.iter().filter(|&&p| p <= highest).count().max(1);
    // Hidden partials are not held against a key, but a key that is mostly
    // hidden has not been heard at all: with a top note ringing, some unrelated
    // key whose partials sit under it was seen to look complete on two matches.
    let hidden_below = hidden.iter().filter(|&&p| p <= highest).count();
    if hidden_below * 3 > gaps_counted + hidden_below {
        return None;
    }
    let energy: f64 = agreed.iter().map(|&&(_, _, a)| a).sum();
    let first_energy: f64 = agreed.iter().filter(|&&&(p, _, _)| p == 1).map(|&&(_, _, a)| a).sum();
    let completeness = matched as f64 / gaps_counted as f64;
    let offset = agreed.iter().map(|&&(_, o, a)| o * a).sum::<f64>() / energy.max(1e-300);

    Some(Candidate {
        key: (i + 1) as u8,
        cents: expect + offset,
        matched,
        expected: gaps_counted,
        score: energy * completeness * completeness,
        energy,
        first_energy,
    })
}

/// Whether something already sounding before the blow sits within `span_cents`
/// of `hz`.
fn hidden_near(hidden: &[f64], hz: f64, span_cents: f64) -> bool {
    let span = 2f64.powf(span_cents / 1200.0);
    let start = hidden.partition_point(|&f| f < hz / span);
    hidden.get(start).is_some_and(|&f| f <= hz * span)
}

/// The strongest peak within `span_cents` of `hz`, as `(hz, amplitude)`.
fn nearest(peaks: &[Peak], hz: f64, span_cents: f64) -> Option<(f64, f64)> {
    let span = 2f64.powf(span_cents / 1200.0);
    let (lo, hi) = (hz / span, hz * span);
    let start = peaks.partition_point(|p| p.hz < lo);
    peaks[start..]
        .iter()
        .take_while(|p| p.hz <= hi)
        .max_by(|a, b| a.amplitude.total_cmp(&b.amplitude))
        .map(|p| (p.hz, p.amplitude))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::piano::key_nominal_hz;
    use crate::synth::{render, ToneSpec};

    const SR: f64 = 48_000.0;

    /// A plausible stiffness across the keyboard: high in the bass, lowest in
    /// the tenor, climbing steeply into the treble.
    fn b_of(key: u8) -> f64 {
        let k = f64::from(key);
        let log = if k <= 30.0 { -3.1 - 0.015 * (k - 1.0) } else { -3.55 + 0.032 * (k - 30.0) };
        10f64.powf(log)
    }

    struct Fixture {
        hz: Vec<f64>,
        b: Vec<f64>,
        expect: Vec<f64>,
    }

    fn piano() -> Fixture {
        Fixture {
            hz: (1..=88).map(|k| key_nominal_hz(k, 440.0)).collect(),
            b: (1..=88).map(b_of).collect(),
            expect: vec![0.0; 88],
        }
    }

    impl Fixture {
        fn keyboard(&self) -> Keyboard<'_> {
            Keyboard { targets_hz: &self.hz, b: &self.b, expect_cents: &self.expect }
        }
    }

    /// A struck three-string note `off` cents from key `key`'s target.
    fn strike(key: u8, off: f64, seconds: f64) -> Vec<f32> {
        let f0 = key_nominal_hz(key, 440.0) * 2f64.powf(off / 1200.0);
        let mut spec = ToneSpec::unison(f0, b_of(key), seconds, 3, 0.4);
        spec.noise_dbfs = Some(-70.0);
        render(&spec)
    }

    /// Silence, then a blow.
    fn after_silence(note: Vec<f32>) -> (Vec<f32>, usize) {
        let pad = (SR * 0.3) as usize;
        let mut v = vec![0.0f32; pad];
        v.extend(note);
        (v, pad)
    }

    #[test]
    fn names_every_key_struck_from_silence() {
        let p = piano();
        let mut wrong = Vec::new();
        // C8 has a single partial under 9 kHz worth the name; it is left to a tap.
        for key in 1..=87u8 {
            let (audio, split) = after_silence(strike(key, 0.0, 0.5));
            let d = identify(&audio, split, SR, p.keyboard(), None);
            if d.key_to_tune(None) != Some(key) {
                wrong.push((key, d.decision, d.best.map(|c| c.key)));
            }
        }
        assert!(wrong.is_empty(), "misnamed: {wrong:?}");
    }

    #[test]
    fn neither_octave_steals_the_answer() {
        let p = piano();
        for key in [16u8, 28, 40, 52, 64] {
            let (audio, split) = after_silence(strike(key, 0.0, 0.5));
            let d = identify(&audio, split, SR, p.keyboard(), Some(key + 12));
            assert_eq!(d.decision, Decision::Switch, "key {key}: {d:?}");
            assert_eq!(d.best.unwrap().key, key);
            let d = identify(&audio, split, SR, p.keyboard(), Some(key - 12));
            assert_eq!(d.best.unwrap().key, key, "key {key} from below: {d:?}");
        }
    }

    #[test]
    fn a_ringing_bass_note_does_not_win_over_the_new_one() {
        let p = piano();
        // E1 struck and ringing; G#1 (a major third up, the known trap) struck
        // more quietly on top of it.
        let old = strike(8, 0.0, 1.6);
        let new = strike(12, 0.0, 0.8);
        let at = (SR * 0.8) as usize;
        let mut mix: Vec<f32> = old.clone();
        for (i, s) in new.iter().enumerate() {
            if at + i < mix.len() {
                mix[at + i] += s * 0.5;
            }
        }
        let d = identify(&mix, at, SR, p.keyboard(), Some(8));
        assert_eq!(d.decision, Decision::Switch, "{d:?}");
        assert_eq!(d.best.unwrap().key, 12);
    }

    #[test]
    fn a_far_flat_string_is_not_taken_for_its_neighbour() {
        let p = piano();
        // Tuning key 40 while it sits 60 cents flat — it looks like key 39, 40
        // cents sharp. Stay on 40.
        let (audio, split) = after_silence(strike(40, -60.0, 0.5));
        let d = identify(&audio, split, SR, p.keyboard(), Some(40));
        assert_ne!(d.key_to_tune(Some(40)), Some(39), "{d:?}");
        assert_eq!(d.key_to_tune(Some(40)), Some(40));
    }

    #[test]
    fn the_neighbour_struck_in_tune_is_followed() {
        let p = piano();
        let (audio, split) = after_silence(strike(41, 3.0, 0.5));
        let d = identify(&audio, split, SR, p.keyboard(), Some(40));
        assert_eq!(d.key_to_tune(Some(40)), Some(41), "{d:?}");
    }

    #[test]
    fn a_piano_far_flat_throughout_is_matched_against_where_it_is() {
        let mut p = piano();
        p.expect = vec![-40.0; 88];
        let (audio, split) = after_silence(strike(30, -42.0, 0.5));
        let d = identify(&audio, split, SR, p.keyboard(), Some(29));
        assert_eq!(d.key_to_tune(Some(29)), Some(30), "{d:?}");
        assert!((d.best.unwrap().cents + 42.0).abs() < 3.0, "{d:?}");
    }

    #[test]
    fn silence_decides_nothing() {
        let p = piano();
        let audio = vec![0.0f32; (SR * 0.8) as usize];
        let d = identify(&audio, (SR * 0.3) as usize, SR, p.keyboard(), Some(40));
        assert_eq!(d.decision, Decision::Nothing);
        assert_eq!(d.key_to_tune(Some(40)), Some(40));
    }
}
