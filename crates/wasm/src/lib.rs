//! WebAssembly interface to the measurement engine.
//!
//! # Why a bare C interface
//!
//! No `wasm-bindgen`, no `wasm-pack`, no npm. The whole build is
//!
//! ```text
//! cargo build -p stiffstring-wasm --target wasm32-unknown-unknown --release
//! ```
//!
//! which produces a `.wasm` file that plain JavaScript instantiates directly.
//! Binding generators are pleasant when a project already has a JavaScript build
//! pipeline; this one deliberately has none, and the surface here is small enough
//! that hand-marshalling costs less than the tooling would.
//!
//! # Calling convention
//!
//! JavaScript allocates a buffer with [`ss_alloc`], writes samples into the
//! module's memory, calls a measurement function with a second buffer for
//! results, reads the results out, and frees both. Every function returns a
//! count of `f64`s written, or zero on failure, so nothing ever traps across the
//! boundary if it can be helped.
//!
//! All layouts are documented on the functions themselves and are the contract
//! the JavaScript loader depends on. Change one and change `wasm/stiffstring.js`
//! in the same commit.

use std::alloc::{alloc, dealloc, Layout};

use stiffstring_core::curve::{solve, CurveConfig};
use stiffstring_core::inharmonicity::{measure_note, Concern, MeasureConfig};
use stiffstring_core::meter::{lock_note, settle, track, Reading};
use stiffstring_core::piano::{
    anchor_keys, fit_model, key_nominal_hz, suggest_next_key, NoteSample, KEYS,
};

/// Interface version. Bumped whenever a layout below changes, so the loader can
/// refuse to run against a module it does not understand.
///
/// 2: added unison spread per note and beat rate per partial.
/// 3: stiffness per key added to the curve output; note-choosing exposed.
/// 4: the live meter — locking, tracking, and what to put on the display.
pub const ABI_VERSION: u32 = 4;

#[no_mangle]
pub extern "C" fn ss_abi_version() -> u32 {
    ABI_VERSION
}

/// Reserve `bytes` of module memory, aligned for `f64`. Returns null on failure.
///
/// # Safety
/// The caller must eventually pass the returned pointer and the same `bytes` to
/// [`ss_free`].
#[no_mangle]
pub extern "C" fn ss_alloc(bytes: usize) -> *mut u8 {
    if bytes == 0 {
        return std::ptr::null_mut();
    }
    match Layout::from_size_align(bytes, 8) {
        // SAFETY: size is non-zero and the alignment is valid.
        Ok(layout) => unsafe { alloc(layout) },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Release memory obtained from [`ss_alloc`].
///
/// # Safety
/// `ptr` must have come from [`ss_alloc`] with the same `bytes`, and must not be
/// used afterwards.
#[no_mangle]
pub unsafe extern "C" fn ss_free(ptr: *mut u8, bytes: usize) {
    if ptr.is_null() || bytes == 0 {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(bytes, 8) {
        dealloc(ptr, layout);
    }
}

/// Equal-tempered frequency of a key, for a reference pitch. Convenience so the
/// interface and the page cannot disagree about what key 49 means.
#[no_mangle]
pub extern "C" fn ss_key_nominal_hz(key: u32, a4_hz: f64) -> f64 {
    if !(1..=u32::from(KEYS)).contains(&key) {
        return 0.0;
    }
    key_nominal_hz(key as u8, a4_hz)
}

fn concern_bits(concerns: &[Concern]) -> f64 {
    let mut bits = 0u32;
    for c in concerns {
        bits |= match c {
            Concern::FundamentalMissing => 1,
            Concern::FewPartials => 2,
            Concern::PoorFit => 4,
            Concern::UnstablePartials => 8,
            Concern::PartialsRejected => 16,
            Concern::StringsBeating => 32,
        };
    }
    f64::from(bits)
}

/// Number of `f64`s [`ss_measure_note`] writes before the per-partial block.
const NOTE_HEADER: usize = 7;
/// Number of `f64`s per partial.
const PARTIAL_STRIDE: usize = 7;

/// Measure one struck note.
///
/// Input is `len` `f32` samples at `samples`. `f0_hint` is where the fundamental
/// is expected, from the key the technician selected; it may be well over a
/// semitone out.
///
/// Writes to `out`:
///
/// ```text
/// [0] fundamental, Hz
/// [1] inharmonicity coefficient
/// [2] fit residual, cents
/// [3] concern bits: 1 fundamental missing, 2 few partials, 4 poor fit,
///                   8 unstable partials, 16 partials rejected,
///                   32 beating unison
/// [4] partial count P
/// [5] unison spread in cents, or 0 if the strings were not heard beating
/// [6] how much this measurement should count toward a keyboard model, 0 to 1
/// then P blocks of 7: partial number, Hz, amplitude, confidence,
///                     residual cents, used (1 or 0),
///                     beat rate in Hz or 0 if not beating
/// ```
///
/// Returns the number of `f64`s written, or 0 if the note could not be measured
/// or `out` is too small.
///
/// # Safety
/// `samples` must point to `len` readable `f32`s and `out` to `out_len`
/// writable `f64`s.
#[no_mangle]
pub unsafe extern "C" fn ss_measure_note(
    samples: *const f32,
    len: usize,
    sample_rate: f64,
    f0_hint: f64,
    out: *mut f64,
    out_len: usize,
) -> usize {
    if samples.is_null() || out.is_null() || len == 0 || sample_rate.is_nan() || sample_rate <= 0.0
    {
        return 0;
    }
    let audio = std::slice::from_raw_parts(samples, len);
    let Some(m) = measure_note(audio, sample_rate, f0_hint, MeasureConfig::default()) else {
        return 0;
    };

    let needed = NOTE_HEADER + m.partials.len() * PARTIAL_STRIDE;
    if out_len < needed {
        return 0;
    }
    let dst = std::slice::from_raw_parts_mut(out, needed);

    dst[0] = m.f0;
    dst[1] = m.b;
    dst[2] = m.rms_cents;
    dst[3] = concern_bits(&m.concerns);
    dst[4] = m.partials.len() as f64;
    // Zero stands for "not beating". A beat of exactly no hertz is not a thing a
    // string can do, so the value is free to carry the meaning.
    dst[5] = m.beat_spread_cents.unwrap_or(0.0);
    // Computed here rather than in JavaScript so the rule for how much a
    // troubled measurement counts lives in exactly one place.
    dst[6] = NoteSample::from_measurement(1, &m).weight;
    for (i, p) in m.partials.iter().enumerate() {
        let base = NOTE_HEADER + i * PARTIAL_STRIDE;
        dst[base] = f64::from(p.n);
        dst[base + 1] = p.hz;
        dst[base + 2] = p.amplitude;
        dst[base + 3] = p.confidence;
        dst[base + 4] = p.residual_cents;
        dst[base + 5] = if p.used { 1.0 } else { 0.0 };
        dst[base + 6] = p.beat_hz.unwrap_or(0.0);
    }
    needed
}

/// Decide which partial of a note to follow, and take a first reading.
///
/// Called once when a note is struck, and again whenever the meter loses it.
/// `target_f0` is what the note is being tuned to and `b` its stiffness, both
/// from [`ss_solve_curve`]'s output.
///
/// Writes to `out`:
///
/// ```text
/// [0] partial number to follow
/// [1] where that partial actually is, Hz
/// [2] where it would be if the note were on target, Hz
/// [3] how far the note is from its target, cents, sharp positive
/// [4] amplitude
/// [5] confidence, 0 to 1
/// ```
///
/// Returns 6, or 0 when there is nothing to lock onto: the note is not
/// sounding, it is more than a semitone and a half from its target, or what is
/// sounding is a different note. Zero is an ordinary answer, not an error.
///
/// # Safety
/// `samples` must point to `len` readable `f32`s and `out` to `out_len`
/// writable `f64`s.
#[no_mangle]
pub unsafe extern "C" fn ss_lock_note(
    samples: *const f32,
    len: usize,
    sample_rate: f64,
    target_f0: f64,
    b: f64,
    out: *mut f64,
    out_len: usize,
) -> usize {
    if samples.is_null() || out.is_null() || len == 0 || out_len < 6 || sample_rate <= 0.0 {
        return 0;
    }
    let audio = std::slice::from_raw_parts(samples, len);
    let Some(l) = lock_note(audio, sample_rate, target_f0, b) else {
        return 0;
    };
    let dst = std::slice::from_raw_parts_mut(out, 6);
    dst[0] = f64::from(l.partial);
    dst[1] = l.hz;
    dst[2] = l.target_hz;
    dst[3] = l.cents;
    dst[4] = l.amplitude;
    dst[5] = l.confidence;
    6
}

/// Read a partial already locked on to.
///
/// `previous_hz` is the frequency last reported for it, or zero if there is
/// none; handing it back is what keeps the reading steady between updates.
///
/// Writes to `out`:
///
/// ```text
/// [0] where the partial is now, Hz
/// [1] how far the note is from its target, cents, sharp positive
/// [2] amplitude
/// [3] confidence, 0 to 1
/// [4] beat rate in Hz, or 0 if the strings are not heard beating
/// ```
///
/// Returns 5, or 0 when the partial has fallen into the noise. The caller
/// should hold the last reading briefly rather than blank the display: a note
/// fading is not a note moving.
///
/// # Safety
/// `samples` must point to `len` readable `f32`s and `out` to `out_len`
/// writable `f64`s.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn ss_track(
    samples: *const f32,
    len: usize,
    sample_rate: f64,
    target_f0: f64,
    b: f64,
    partial: u32,
    previous_hz: f64,
    out: *mut f64,
    out_len: usize,
) -> usize {
    if samples.is_null() || out.is_null() || len == 0 || out_len < 5 || sample_rate <= 0.0 {
        return 0;
    }
    let audio = std::slice::from_raw_parts(samples, len);
    let Some(r) = track(audio, sample_rate, target_f0, b, partial, previous_hz) else {
        return 0;
    };
    let dst = std::slice::from_raw_parts_mut(out, 5);
    dst[0] = r.hz;
    dst[1] = r.cents;
    dst[2] = r.amplitude;
    dst[3] = r.confidence;
    // Zero stands for "not beating", as everywhere else in this interface.
    dst[4] = r.beat_hz.unwrap_or(0.0);
    5
}

/// Turn a short run of readings into the one number to show.
///
/// `readings` is `count` pairs of `f64`: the cents and the confidence of each,
/// which is all the rule needs. Newest last, though the rule does not care.
///
/// Writes to `out`:
///
/// ```text
/// [0] the number to show, cents
/// [1] how far the readings behind it disagree, cents
/// [2] how many readings it rests on
/// ```
///
/// Lives here rather than in the page because deciding what a run of readings
/// means is a judgement about audio, and it should not be restated in
/// JavaScript.
///
/// # Safety
/// `readings` must point to `count * 2` readable `f64`s and `out` to `out_len`
/// writable `f64`s.
#[no_mangle]
pub unsafe extern "C" fn ss_settle(
    readings: *const f64,
    count: usize,
    out: *mut f64,
    out_len: usize,
) -> usize {
    if readings.is_null() || out.is_null() || count == 0 || out_len < 3 {
        return 0;
    }
    let raw = std::slice::from_raw_parts(readings, count * 2);
    let rs: Vec<Reading> = (0..count)
        .map(|i| Reading {
            hz: 0.0,
            cents: raw[i * 2],
            amplitude: 0.0,
            confidence: raw[i * 2 + 1],
            beat_hz: None,
        })
        .collect();
    let Some(s) = settle(&rs) else {
        return 0;
    };
    let dst = std::slice::from_raw_parts_mut(out, 3);
    dst[0] = s.cents;
    dst[1] = s.spread;
    dst[2] = s.used as f64;
    3
}

/// Number of `f64`s [`ss_solve_curve`] writes: two header values then three
/// arrays of 88.
pub const CURVE_OUT_LEN: usize = 2 + 3 * KEYS as usize;

/// Read the notes worth measuring first into `out`, returning how many.
///
/// Returns 0 if `out` is too small. Ask for at least 32 and there will be room.
///
/// # Safety
/// `out` must point to `out_len` writable `f64`s.
#[no_mangle]
pub unsafe extern "C" fn ss_anchor_keys(out: *mut f64, out_len: usize) -> usize {
    if out.is_null() {
        return 0;
    }
    let keys = anchor_keys();
    if out_len < keys.len() {
        return 0;
    }
    let dst = std::slice::from_raw_parts_mut(out, keys.len());
    for (slot, key) in dst.iter_mut().zip(&keys) {
        *slot = f64::from(*key);
    }
    keys.len()
}

/// The next note most worth measuring, given what has been measured already.
///
/// `samples` is the same layout [`ss_solve_curve`] takes: `count` groups of four
/// `f64`s — key, fundamental, stiffness, weight.
///
/// Returns the key, or 0 when nothing further would meaningfully improve the
/// model, which is the signal to stop asking the technician for notes.
///
/// # Safety
/// `samples` must point to `count * 4` readable `f64`s.
#[no_mangle]
pub unsafe extern "C" fn ss_suggest_next_key(samples: *const f64, count: usize) -> u32 {
    if samples.is_null() || count == 0 {
        return 0;
    }
    let notes = read_samples(samples, count);
    let Some(model) = fit_model(&notes) else {
        return 0;
    };
    suggest_next_key(&notes, &model).map_or(0, u32::from)
}

/// Unpack the caller's four-per-note layout, dropping anything unusable.
unsafe fn read_samples(samples: *const f64, count: usize) -> Vec<NoteSample> {
    let raw = std::slice::from_raw_parts(samples, count * 4);
    (0..count)
        .map(|i| &raw[i * 4..i * 4 + 4])
        .filter(|c| c[0] >= 1.0 && c[0] <= f64::from(KEYS) && c[2] > 0.0)
        .map(|c| NoteSample {
            key: c[0] as u8,
            f0: c[1],
            b: c[2],
            weight: c[3],
        })
        .collect()
}

/// Fit inharmonicity across the keyboard from measured notes, then solve for
/// tuning targets.
///
/// Deliberately stateless: the caller passes every sample each time. Keeping a
/// model alive across calls would buy nothing measurable — the whole solve is
/// milliseconds — and would cost a lifetime to get wrong.
///
/// `samples` is `count` groups of four `f64`s: key (1..=88), measured
/// fundamental in Hz (unused by the fit, carried for the record), inharmonicity
/// coefficient, and weight.
///
/// `overrides` is `override_count` pairs of `f64`: key, and cents from equal
/// temperament. Pass a null pointer and zero for none.
///
/// Writes to `out`:
///
/// ```text
/// [0] detected break key, or 0 if the samples supported no split
/// [1] model residual, log10 units
/// [2 .. 90]   cents from equal temperament, keys 1 to 88
/// [90 .. 178] target frequencies, keys 1 to 88
/// [178 .. 266] inharmonicity coefficient, keys 1 to 88
/// ```
///
/// The stiffness array is what lets a caller measure the top octave: those notes
/// give too few partials to determine their own, and can borrow it from here.
///
/// Returns the number of `f64`s written, or 0 on failure — most often too few
/// usable samples to fit a model at all.
///
/// # Safety
/// Pointers must reference at least the described number of readable or writable
/// elements.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn ss_solve_curve(
    samples: *const f64,
    count: usize,
    a4_hz: f64,
    stretch: f64,
    smoothness: f64,
    overrides: *const f64,
    override_count: usize,
    out: *mut f64,
    out_len: usize,
) -> usize {
    if samples.is_null() || out.is_null() || count == 0 || out_len < CURVE_OUT_LEN {
        return 0;
    }
    let notes = read_samples(samples, count);
    let Some(model) = fit_model(&notes) else {
        return 0;
    };

    let mut cfg = CurveConfig {
        a4_hz,
        stretch,
        smoothness,
        ..CurveConfig::default()
    };
    if !overrides.is_null() && override_count > 0 {
        let raw = std::slice::from_raw_parts(overrides, override_count * 2);
        cfg.overrides = (0..override_count)
            .map(|i| &raw[i * 2..i * 2 + 2])
            .filter(|c| c[0] >= 1.0 && c[0] <= f64::from(KEYS))
            .map(|c| (c[0] as u8, c[1]))
            .collect();
    }

    let Some(curve) = solve(&model, &cfg) else {
        return 0;
    };

    let dst = std::slice::from_raw_parts_mut(out, CURVE_OUT_LEN);
    dst[0] = model.break_key.map_or(0.0, f64::from);
    dst[1] = model.rms_log10;
    let n = KEYS as usize;
    dst[2..2 + n].copy_from_slice(&curve.cents);
    dst[2 + n..2 + 2 * n].copy_from_slice(&curve.hz);
    dst[2 + 2 * n..2 + 3 * n].copy_from_slice(&model.all());
    CURVE_OUT_LEN
}
