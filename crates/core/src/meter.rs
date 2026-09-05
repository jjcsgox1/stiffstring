//! The live meter: how far a sounding note is from a target we already know.
//!
//! # A tracker, not a detector
//!
//! Everything else in this engine answers "what is this note, and what is it
//! doing?" — a hard question, which is why measuring one note costs hundreds of
//! milliseconds and searches over partial numberings before it will commit to an
//! answer.
//!
//! By the time the technician is setting a pin, that question is already
//! settled. The note is known, its stiffness is known, and the target is known,
//! so every partial's wanted frequency is arithmetic. The only question left is
//! *how far is this one component from where I want it*, which is a far smaller
//! question and can therefore be answered many times a second without ever
//! flinching.
//!
//! That is the whole reason for the split. A general-purpose tuner re-derives
//! the note from scratch on every update, and its reading dances because each
//! update is an independent guess. Here consecutive updates ask the same narrow
//! question of overlapping audio, so the answer moves only when the string does.
//!
//! # Why the lowest usable partial
//!
//! The target for partial *n* is `n·f0·√(1 + B·n²)`, so the meter's zero depends
//! on `B` — and `B` is measured, or worse, interpolated. That error enters as
//! `600·log2(1 + B·n²)`, which grows as the square of the partial number. On a
//! note whose stiffness we know only to 20%, following the fundamental puts the
//! zero out by under two tenths of a cent; following partial 4 puts it out by
//! nearly three cents, and partial 8 by ten.
//!
//! So [`lock_note`] takes the lowest partial that is genuinely there, rather
//! than the loudest. In the treble that is the fundamental. In the bass, where
//! the fundamental does not exist on the instrument and would not survive the
//! microphone if it did, it is whichever low partial does exist — and the
//! resulting sensitivity to `B` is a real limit of bass metering rather than an
//! oversight.
//!
//! # Two ways to be confidently wrong, and the guards against them
//!
//! Both were found by testing rather than by reasoning, which is the pattern
//! throughout this engine: an estimator asked to find something will find it.
//!
//! A note two semitones from its target is not silently ignored — its partials
//! are numerous enough that some *other* one of them lands inside the band we
//! are searching, and the meter reports a steady, confident reading of the wrong
//! pairing. The guard is that a partial's reading must be **corroborated by
//! another partial** before it is believed. A real note's partials all say the
//! same thing; a mis-pairing's do not.
//!
//! And the note an octave below shares every one of our partials, so
//! corroboration alone cannot see it. What it does *not* share is a strong
//! component at half our fundamental, so that is checked for directly.

use crate::estimate::{cents, noise_floor, refine, RefineConfig};
use crate::fft::{amplitudes, bin_hz, parabolic_peak, spectrum, window, Window};
use crate::synth::partial_hz;

/// Highest partial the meter will consider following.
///
/// Past this the stiffness sensitivity above outweighs anything gained, and on a
/// piano a partial this high is as likely to belong to somebody else's string.
pub const MAX_TRACK_PARTIAL: u32 = 8;

/// Below this the microphone and the instrument between them deliver too little
/// to steer by. A0's fundamental at 27.5 Hz is not merely quiet on a phone; it
/// is frequently absent from the string as well.
const MIN_TRACK_HZ: f64 = 90.0;

/// Above this, partials die away too fast to hold a reading between blows.
const MAX_TRACK_HZ: f64 = 5000.0;

/// How far either side of the target [`lock_note`] will look for the note.
///
/// Deliberately wider than a semitone. A string this far out is still the string
/// the technician means, and refusing to show a number for it would hide exactly
/// the case where a number is most wanted. The cost is that a neighbouring key
/// can be picked up if the wrong one is struck — which shows on the meter as
/// most of a semitone of error, and reads as the mistake it is.
const SEARCH_CENTS: f64 = 150.0;

/// How far a partial must stand above the noise floor to be believed. Generous,
/// because a partial the meter can barely see produces a reading that wanders,
/// and a wandering meter is worse than one that says nothing at all.
const NOISE_MARGIN: f64 = 5.0;

/// A partial carrying less than this share of the strongest one's amplitude is
/// passed over even when it is lower. Preferring low partials is worth a great
/// deal, but not worth steering by something barely present.
const RELATIVE_STRENGTH: f64 = 0.22;

/// How closely two partials must agree, in cents, to corroborate each other.
///
/// Loose, because the stiffness the targets are computed from is itself only
/// approximate and that error grows with the partial number — ten cents at
/// partial 8 is an ordinary consequence of a 20% error in `B`. It does not need
/// to be tight: a mis-paired partial disagrees by tens of cents, not by ten.
const AGREE_CENTS: f64 = 15.0;

/// A component at half the target fundamental this strong, relative to the
/// partial being followed, means the octave below was struck.
///
/// Set high on purpose. Striking a note excites its neighbours sympathetically,
/// but the octave below resonates at the partial that *matches* — not at its own
/// fundamental — so a fundamental this strong down there is a string that was
/// actually hit.
const SUB_OCTAVE_SHARE: f64 = 0.6;

/// Below this there is nothing to be learned from looking an octave down: the
/// microphone and the instrument have both given up.
const SUB_OCTAVE_MIN_HZ: f64 = 60.0;

/// Confidence a reading should carry before it is put on the display.
///
/// High, and it earns its keep: on real recordings, admitting everything the
/// tracker returns doubles how much the display moves on a held note. What it
/// throws away is mostly the tail of a treble note, where the partial has faded
/// into the room and the phase fit is reading noise.
const DISPLAY_CONFIDENCE: f64 = 0.9;

/// Never show a number resting on fewer readings than this if more are to hand.
/// A meter that goes blank in a noisy room is not honest, only useless.
const MIN_DISPLAY_READINGS: usize = 3;

/// Which partial the meter has settled on following, and where it found it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lock {
    pub partial: u32,
    /// Where that partial actually is, in Hz.
    pub hz: f64,
    /// Where it would sit if the note were exactly on target.
    pub target_hz: f64,
    /// How far the note is from its target, in cents. Sharp is positive.
    pub cents: f64,
    pub amplitude: f64,
    pub confidence: f64,
}

/// One update from the meter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub hz: f64,
    /// How far the note is from its target, in cents. Sharp is positive.
    pub cents: f64,
    pub amplitude: f64,
    pub confidence: f64,
    /// Beat rate, when this note's strings are heard beating against each other.
    /// Present for the same reason it is present in a measurement: it is what
    /// the technician is listening for while setting a unison.
    pub beat_hz: Option<f64>,
}

/// Largest power of two not exceeding `n`.
#[inline]
fn floor_pow2(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1usize << (usize::BITS - 1 - n.leading_zeros()) as usize
    }
}

/// How to look at a block of audio of this length.
///
/// The frame must be long enough to separate one partial from its neighbours —
/// in the bass they sit only a fundamental apart — and the frames must still
/// have room to spread out in time, since their spread is where the precision
/// comes from. Half the block each is the balance.
fn refine_config(block_len: usize) -> Option<RefineConfig> {
    let frame_len = floor_pow2(block_len / 2).clamp(1024, 8192);
    if block_len < frame_len + 64 {
        return None;
    }
    Some(RefineConfig {
        frame_len,
        frames: 24,
        window: Window::BlackmanHarris,
    })
}

/// Decide which partial of this note to follow, and take a first reading.
///
/// `target_f0` is the fundamental the note is being tuned to and `b` its
/// stiffness — from the note's own measurement where there is one, and from the
/// keyboard model where there is not.
///
/// Returns `None` when the note is not sounding, or is further than
/// [`SEARCH_CENTS`] from its target, which a caller should report as nothing to
/// measure rather than as a failure.
pub fn lock_note(samples: &[f32], sample_rate: f64, target_f0: f64, b: f64) -> Option<Lock> {
    if target_f0 <= 0.0 || !target_f0.is_finite() || !b.is_finite() || b < 0.0 {
        return None;
    }
    let n = floor_pow2(samples.len());
    if n < 1024 {
        return None;
    }

    let w = window(Window::BlackmanHarris, n);
    let amps = amplitudes(&spectrum(&samples[..n], &w), &w);
    let bin = bin_hz(sample_rate, n);
    let floor = noise_floor(samples).max(1e-12);
    let nyquist = sample_rate / 2.0;

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut bands = 0usize;
    for partial in 1..=MAX_TRACK_PARTIAL {
        let want = partial_hz(target_f0, b, partial);
        if want < MIN_TRACK_HZ || want > MAX_TRACK_HZ || want >= nyquist * 0.95 {
            continue;
        }
        bands += 1;
        let Some((found, amplitude)) = peak_near(&amps, bin, want, SEARCH_CENTS) else {
            continue;
        };
        if amplitude > floor * NOISE_MARGIN {
            candidates.push(Candidate {
                partial,
                want,
                found,
                amplitude,
                cents: cents(want, found),
            });
        }
    }

    // At the very top of the keyboard a note has only one partial inside the
    // trackable range, so there is nothing available to corroborate with. That
    // is not a reason to refuse to meter the top octave: with a single band
    // being searched the mis-pairing this guards against cannot arise, and the
    // one thing that *can* sit there — the octave below — is caught separately.
    let alone = bands <= 1;

    // How many *other* partials say the same thing about where the note is.
    let support = |c: &Candidate| {
        let mut seen: Vec<u32> = Vec::new();
        for o in &candidates {
            if o.partial != c.partial
                && (o.cents - c.cents).abs() < AGREE_CENTS
                && !seen.contains(&o.partial)
            {
                seen.push(o.partial);
            }
        }
        seen.len()
    };

    let agreed: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| alone || support(c) >= 1)
        .collect();
    // Strength is judged against the corroborated peaks only. Measuring it
    // against the loudest thing anywhere would let an interloper set a bar our
    // own partials cannot clear.
    let strongest = agreed.iter().fold(0.0f64, |m, c| m.max(c.amplitude));

    // The lowest partial that is genuinely present and agrees with its fellows —
    // for the stiffness reason and the mis-pairing reason above, in that order.
    // Within a partial, the best-supported reading, then the strongest.
    let chosen = **agreed
        .iter()
        .filter(|c| c.amplitude >= strongest * RELATIVE_STRENGTH)
        .min_by(|a, c| {
            a.partial
                .cmp(&c.partial)
                .then_with(|| support(c).cmp(&support(a)))
                .then_with(|| c.amplitude.total_cmp(&a.amplitude))
        })?;

    // Is the octave below sounding instead? It shares all of our partials, so
    // nothing above could have noticed.
    let half = target_f0 / 2.0;
    if half >= SUB_OCTAVE_MIN_HZ {
        if let Some((_, amplitude)) = peak_near(&amps, bin, half, SEARCH_CENTS) {
            if amplitude >= chosen.amplitude * SUB_OCTAVE_SHARE {
                return None;
            }
        }
    }

    let cfg = refine_config(samples.len())?;
    let refined = refine(samples, sample_rate, chosen.found, cfg)?;

    Some(Lock {
        partial: chosen.partial,
        hz: refined.hz,
        target_hz: chosen.want,
        cents: cents(chosen.want, refined.hz),
        amplitude: refined.amplitude,
        confidence: refined.confidence,
    })
}

/// One partial we might follow: where it should be, where it is, and how far
/// apart those are.
#[derive(Clone, Copy)]
struct Candidate {
    partial: u32,
    want: f64,
    found: f64,
    amplitude: f64,
    cents: f64,
}

/// The strongest peak within `span_cents` either side of `center`, as
/// `(hz, amplitude)`.
///
/// A local maximum rather than the loudest bin: a bin on the shoulder of a much
/// louder neighbouring partial can out-read a real peak sitting in the middle of
/// the band, and following it would be following the neighbour.
///
/// # One peak per band, and why not more
///
/// The loudest peak in a band is sometimes not ours. A bass note rings for the
/// better part of a minute, so on a real recording of E1 the strongest peaks in
/// every band belonged to the C1 struck before it, still sounding. Keeping the
/// three strongest per band was tried, so that the consensus in [`lock_note`]
/// could pick out the quieter self-agreeing set — and it made matters worse, in
/// a way worth writing down.
///
/// C1 sat a major third below the note being metered. A partial series a simple
/// ratio away lands in our bands *consistently* — its 5th in our 4th, its 10th
/// in our 8th, both at the same offset — so it corroborates itself perfectly and
/// the meter locked onto it, reporting 29 cents sharp on a note that was 31
/// cents flat. Corroboration establishes that some note is sounding coherently,
/// never that it is ours.
///
/// With one peak per band the interloper's partials land at scattered offsets,
/// agree with nothing, and the meter says nothing at all — which is the right
/// answer when another string is drowning the one being tuned.
fn peak_near(amps: &[f64], bin: f64, center: f64, span_cents: f64) -> Option<(f64, f64)> {
    let span = 2f64.powf(span_cents / 1200.0);
    let lo = (((center / span) / bin).floor().max(1.0)) as usize;
    let hi = (((center * span) / bin).ceil() as usize).min(amps.len().saturating_sub(2));
    if hi <= lo + 1 {
        return None;
    }
    let mut best = (0.0f64, 0.0f64); // amplitude, hz
    for i in lo..=hi {
        if amps[i] > amps[i - 1] && amps[i] >= amps[i + 1] {
            let (offset, amplitude) = parabolic_peak(amps[i - 1], amps[i], amps[i + 1]);
            if amplitude > best.0 {
                best = (amplitude, (i as f64 + offset) * bin);
            }
        }
    }
    (best.0 > 0.0).then_some((best.1, best.0))
}

/// Read a partial already locked on to.
///
/// `previous_hz` is the last frequency reported for it, or zero if there is
/// none. Handing the previous answer back is what keeps consecutive updates
/// asking the same question about the same component, which is where the meter's
/// steadiness comes from; without it every update would re-acquire and the
/// reading would jitter by the width of the acquisition.
///
/// Returns `None` when the partial has fallen into the noise — the note has died
/// away, or was never struck. A caller should hold the last reading briefly
/// rather than blank the display, because a note fading is not a note moving.
pub fn track(
    samples: &[f32],
    sample_rate: f64,
    target_f0: f64,
    b: f64,
    partial: u32,
    previous_hz: f64,
) -> Option<Reading> {
    if partial == 0 || partial > MAX_TRACK_PARTIAL || target_f0 <= 0.0 || !target_f0.is_finite() {
        return None;
    }
    let want = partial_hz(target_f0, b, partial);
    if want <= 0.0 || want >= sample_rate / 2.0 {
        return None;
    }

    // Start from where the partial was last seen, but only if that is somewhere
    // this note could plausibly be. A stale hint left over from another note
    // would quietly aim the whole measurement at the wrong place.
    let span = 2f64.powf(SEARCH_CENTS / 1200.0);
    let start = if previous_hz > want / span && previous_hz < want * span {
        previous_hz
    } else {
        want
    };

    let cfg = refine_config(samples.len())?;
    let refined = refine(samples, sample_rate, start, cfg)?;
    if refined.amplitude <= noise_floor(samples).max(1e-12) * NOISE_MARGIN {
        return None;
    }

    Some(Reading {
        hz: refined.hz,
        cents: cents(want, refined.hz),
        amplitude: refined.amplitude,
        confidence: refined.confidence,
        beat_hz: refined.beat_hz,
    })
}

/// What to put on the display, from a short run of readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settled {
    /// The number to show, in cents from target.
    pub cents: f64,
    /// How far the readings behind it disagree. This is the meter's own
    /// uncertainty at this instant, and on a piano it is mostly the note's
    /// unison arguing with itself rather than anything the engine did.
    pub spread: f64,
    /// How many readings the answer rests on.
    pub used: usize,
}

/// Turn a run of readings into the one number the technician sees.
///
/// # Why the display is not simply the latest reading
///
/// A note with two or three strings has no single pitch. The strings beat, and a
/// quarter-second look at one partial catches the composite wherever it happens
/// to be in that beat — swinging most violently right at the beat's null, where
/// the phase of the sum turns over. Real recordings of an untuned piano swing
/// several cents this way, and it is not error: it is the note.
///
/// So the display is the **median** of a short run. The median rather than the
/// mean because the excursion at the null is large, brief, and not a pitch the
/// ear ever hears — a mean would be dragged by it, a median steps over it.
///
/// [`spread`](Settled::spread) is not thrown away, because how much the readings
/// disagree is exactly what the technician needs to know before trusting the
/// number: a wide spread means the unison wants setting before the pitch does.
///
/// Deliberately stateless, and deliberately here rather than in the page: this
/// is a judgement about audio, and it should not be restated in JavaScript.
pub fn settle(readings: &[Reading]) -> Option<Settled> {
    if readings.is_empty() {
        return None;
    }
    let mut by_confidence: Vec<&Reading> = readings.iter().collect();
    by_confidence.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

    let good = by_confidence
        .iter()
        .filter(|r| r.confidence >= DISPLAY_CONFIDENCE)
        .count();
    let take = good.max(MIN_DISPLAY_READINGS.min(readings.len()));

    let mut used: Vec<f64> = by_confidence[..take].iter().map(|r| r.cents).collect();
    used.sort_by(f64::total_cmp);

    Some(Settled {
        cents: used[used.len() / 2],
        spread: used[used.len() - 1] - used[0],
        used: used.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{render, ToneSpec};

    const SR: f64 = 48_000.0;

    /// A note sounding `off_cents` from where it is wanted.
    fn note(f0: f64, b: f64, off_cents: f64, seconds: f64, partials: usize) -> Vec<f32> {
        let mut spec = ToneSpec::single(f0 * 2f64.powf(off_cents / 1200.0), b, seconds);
        spec.strings[0].partials = partials;
        spec.noise_dbfs = Some(-70.0);
        render(&spec)
    }

    #[test]
    fn locks_onto_the_fundamental_in_the_middle() {
        let audio = note(440.0, 4.0e-4, 0.0, 0.4, 8);
        let lock = lock_note(&audio, SR, 440.0, 4.0e-4).expect("should lock");
        assert_eq!(lock.partial, 1);
        assert!(lock.cents.abs() < 0.1, "cents {}", lock.cents);
    }

    #[test]
    fn reads_the_offset_it_is_given() {
        for off in [-40.0, -7.3, -0.5, 0.0, 0.5, 7.3, 40.0] {
            let audio = note(220.0, 3.0e-4, off, 0.4, 8);
            let lock = lock_note(&audio, SR, 220.0, 3.0e-4).expect("should lock");
            assert!(
                (lock.cents - off).abs() < 0.2,
                "wanted {off}, read {}",
                lock.cents
            );
        }
    }

    /// The bass has no fundamental to track, which is the case the partial
    /// choice exists for.
    #[test]
    fn skips_a_missing_bass_fundamental() {
        // A1 at 55 Hz: the fundamental is below MIN_TRACK_HZ either way, so the
        // meter must climb to a partial the microphone could actually deliver.
        let audio = note(55.0, 8.0e-4, -6.0, 0.5, 12);
        let lock = lock_note(&audio, SR, 55.0, 8.0e-4).expect("should lock");
        assert!(lock.partial >= 2, "partial {}", lock.partial);
        assert!(lock.hz >= MIN_TRACK_HZ, "hz {}", lock.hz);
        assert!((lock.cents + 6.0).abs() < 0.3, "cents {}", lock.cents);
    }

    #[test]
    fn silence_locks_onto_nothing() {
        let quiet = vec![0.0f32; (SR * 0.4) as usize];
        assert!(lock_note(&quiet, SR, 440.0, 4.0e-4).is_none());
    }

    /// The first way to be confidently wrong. A note two semitones sharp puts
    /// its 5th partial squarely inside the band we search for our 6th, and
    /// before the corroboration guard existed the meter reported that pairing —
    /// steadily, at 0.9997 confidence, and 320 cents from the truth.
    #[test]
    fn a_note_far_from_its_target_is_not_claimed() {
        let audio = note(440.0, 4.0e-4, 200.0, 0.4, 8);
        assert!(lock_note(&audio, SR, 440.0, 4.0e-4).is_none());
    }

    /// The second way. The octave below shares every partial we look at, so no
    /// amount of agreement between partials can see it.
    #[test]
    fn the_octave_below_is_not_mistaken_for_the_note() {
        let audio = note(220.0, 3.5e-4, 0.0, 0.5, 12);
        assert!(lock_note(&audio, SR, 440.0, 4.0e-4).is_none());
    }

    /// The top octave has only its fundamental inside the trackable range, and
    /// must still be meterable — refusing to corroborate would refuse to tune
    /// exactly the notes that are hardest to tune by ear.
    #[test]
    fn the_top_octave_locks_on_one_partial() {
        let audio = note(4186.0, 1.2e-2, -3.0, 0.4, 2);
        let lock = lock_note(&audio, SR, 4186.0, 1.2e-2).expect("should lock");
        assert_eq!(lock.partial, 1);
        assert!((lock.cents + 3.0).abs() < 0.3, "cents {}", lock.cents);
    }

    #[test]
    fn tracking_agrees_with_the_lock_and_repeats_itself() {
        let audio = note(330.0, 3.5e-4, 2.4, 1.2, 8);
        let lock = lock_note(&audio[..19_200], SR, 330.0, 3.5e-4).expect("should lock");

        // Successive overlapping blocks, the way the page will feed it.
        let block = 16_800;
        let hop = 2_400;
        let mut previous = lock.hz;
        let mut readings = Vec::new();
        let mut start = 0;
        while start + block <= audio.len() {
            let r = track(
                &audio[start..start + block],
                SR,
                330.0,
                3.5e-4,
                lock.partial,
                previous,
            )
            .expect("should track");
            previous = r.hz;
            readings.push(r.cents);
            start += hop;
        }

        assert!(readings.len() >= 5, "only {} readings", readings.len());
        for c in &readings {
            assert!((c - 2.4).abs() < 0.3, "read {c}");
        }
        // Steadiness is the point of the meter, so measure it rather than
        // assuming it: the spread across a held note must be small enough that
        // the last digit shown does not dance.
        let lo = readings.iter().cloned().fold(f64::MAX, f64::min);
        let hi = readings.iter().cloned().fold(f64::MIN, f64::max);
        assert!(hi - lo < 0.2, "meter wandered {} cents", hi - lo);
    }

    fn reading(cents: f64, confidence: f64) -> Reading {
        Reading {
            hz: 440.0,
            cents,
            amplitude: 0.1,
            confidence,
            beat_hz: None,
        }
    }

    #[test]
    fn the_display_steps_over_a_beat_null() {
        // Eleven honest readings around a quarter cent, and one wild excursion
        // of the kind a beat's null produces. A mean would show a third of a
        // cent too sharp; the median must not notice it at all.
        let mut rs: Vec<Reading> = (0..11)
            .map(|i| reading(0.2 + f64::from(i) * 0.01, 0.95))
            .collect();
        rs.push(reading(4.0, 0.95));
        let s = settle(&rs).expect("should settle");
        assert!((s.cents - 0.25).abs() < 0.06, "showed {}", s.cents);
        // The excursion is reported rather than hidden: it is what tells the
        // technician the unison is arguing.
        assert!(s.spread > 3.0, "spread {}", s.spread);
    }

    #[test]
    fn the_display_prefers_confident_readings_but_never_goes_blank() {
        let rs = vec![
            reading(0.0, 0.95),
            reading(0.1, 0.95),
            reading(9.0, 0.2),
            reading(-9.0, 0.1),
        ];
        let s = settle(&rs).expect("should settle");
        assert_eq!(s.used, 3, "should have fallen back to a minimum of three");
        assert!(s.cents.abs() < 1.0, "showed {}", s.cents);

        // Nothing confident at all: still shows something, from the best it has.
        let poor = vec![reading(1.0, 0.3), reading(1.2, 0.2)];
        assert!(settle(&poor).is_some());
        assert!(settle(&[]).is_none());
    }

    #[test]
    fn tracking_a_dead_note_reports_nothing() {
        let quiet = vec![0.0f32; 16_800];
        assert!(track(&quiet, SR, 440.0, 4.0e-4, 1, 0.0).is_none());
    }

    /// The stiffness sensitivity that decides which partial to follow. Recorded
    /// as a test because getting it wrong is how a meter reads confidently and
    /// wrongly: the number would be steady, and steadily off.
    #[test]
    fn stiffness_error_moves_the_zero_as_the_square_of_the_partial() {
        let b = 1.0e-3;
        let wrong = b * 1.2;
        // Even the fundamental is not immune — it carries √(1+B) — but it is
        // forty times less sensitive than partial 4 and sixty times less than
        // partial 8, which is the whole argument for tracking low.
        for (partial, limit) in [(1u32, 0.2), (2, 0.75), (4, 3.0), (8, 11.0)] {
            let shift = cents(
                partial_hz(440.0, b, partial),
                partial_hz(440.0, wrong, partial),
            );
            assert!(
                shift.abs() < limit,
                "partial {partial} moved {shift} cents, over {limit}"
            );
            if partial > 1 {
                assert!(shift.abs() > 0.05, "partial {partial} moved only {shift}");
            }
        }
    }
}
