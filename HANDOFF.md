# Handoff

**As of 2026-11-19.** Written for whoever picks this up next — another session, another
developer, or the owner six months from now. Ten minutes here should make you competent to
continue. [`PLAN.md`](PLAN.md) has the full reasoning behind every decision; this is the map.

---

## What it is

A piano tuning **measurement instrument** for the iPhone. It listens to an individual acoustic
piano, measures the inharmonicity of its strings, and computes tuning targets for *that*
instrument — then meters each note against them while the pins are set.

It is not a chromatic tuner with a piano skin. The whole claim rests on measuring the piano in
front of you rather than assuming an ideal one.

**Originality constraint, standing and non-negotiable:** this is inspired by the *category* of
professional tuning software, not by any product. Nothing is copied — no proprietary algorithm,
source, branding, artwork, interface, terminology or protected method. Everything here is built
from published acoustics and signal processing. Keep it that way.

## Who the owner is, and what that means

A working piano technician. Fully aural — fork or reference, then entirely by ear. **He does not
program and does not want to.** He will not edit code, copy fragments between files, or debug.
Whoever works on this writes the code, runs it, diagnoses it, fixes it, and hands him plain
instructions and a link.

He has a Windows 11 desktop and an iPhone. **No Mac**, no Xcode, no Node. That is why the product
is a web app and why the Rust toolchain is the whole build.

He is also the project's only source of truth about whether a tuning is any good. There is no
reference ETD to check against. His ear is layer-2 ground truth, and he is a clean-room validator
precisely because he has no electronic tuning habits to replicate.

---

## State of play

**v1 is built and deployed.** Phases 0a–6 of the plan: measure a piano → 88 targets → live meter.

| | |
|---|---|
| Engine | ~6,300 lines of Rust, 97 tests, clippy clean |
| Web app | ~2,600 lines of hand-written HTML/JS, no framework, no build step |
| WASM ABI | version 4 — checked at load, mismatch fails loudly |
| Corpus | 3 sessions of one baby grand, 22 notes each |

Everything is live at <https://jjcsgox1.github.io/stiffstring/> and deploys on `git push`.

### What is actually proven

- **Phase 4 passed on a real piano.** Two recordings of the same baby grand a day apart give
  tuning curves agreeing to **0.10 cents median, 0.52 worst**.
- **The meter is accurate and steady.** Against a full measurement of the same partial,
  0.47–0.60 cents median; the display holds to 0.57–0.60 cents on a held note.
- **Phase 5 has evidence, not a gate.** The owner tuned part of his piano by ear; the solver had
  independently put those notes within **0.65 cents median, 1.6 worst**. Six notes, one register,
  one piano — enough to be encouraging, nowhere near enough to calibrate on.

### What is not done

1. **Nobody has completed a whole tuning with it.** That is phase 6's own gate and the single most
   important outstanding item. Everything else is building on an unverified base.
2. **Unison assistance.** The full-screen tuning view (2026-11-19) now shows the string count and
   the meter's beat rate, and a row per partial so a rough unison shows as rows fanning out. That
   is display only. Nothing yet guides setting the unison, and on synthetic two-string tones the
   meter's beat rate did not register at all — check it on a real unison before building on it.
   The view's "held after strike" reading is triggered by a jump in the followed partial's level;
   it is untested against real hammer blows.
3. **Phase 5 calibration.** Needs complete aural tunings recorded at all 88 keys, on more than one
   piano, plus the repeatability control.
4. **The repeatability control has not started.** The owner tunes one piano twice, some weeks
   apart; the difference is his own repeatability and it sets the accuracy floor for the entire
   project. Without it there is no way to know when to stop optimising. It has a calendar
   dependency and nothing substitutes for it.
5. **Pitch raise, customer records, client report** — phases 7–10, deliberately post-v1.
6. **C8 has never been measurable** on any recording. Too few partials, decays too fast.

### What the owner needs to do

In priority order. Only the first blocks anything.

1. **Take it on a job and complete a tuning with it**, muting as usual. Report where it fails.
2. **Tune a piano by ear and record all 88 keys** (the recorder has the option).
3. **The same piano again some weeks later**, all 88. That is the repeatability pair.
4. **Record a rough old upright** — 22-note survey is fine. The baby grand is the easy case and
   the only one in the corpus.
5. Tell us **where the string counts change** on his pianos. The app guesses single ≤ key 8,
   double ≤ key 16. Wrong values mislabel a false beat as a bad unison; they do not spoil a
   measurement.

---

## How it works, in one page

**Precision comes from phase, not from bigger transforms.** An FFT bin at 48 kHz is ~1.5 Hz; the
target is 0.1 cent, about 0.025 Hz at A4. The FFT only *locates* partials. Each one is then pinned
by watching its phase advance across overlapping frames and fitting a line — limited by how well
phase is measured, not by bin width. A decaying amplitude does not disturb phase, and a partial
between bins shifts the intercept, not the slope.

**The stiff-string model is the spine.** `f_n = n·f0·√(1 + B·n²)`. Everything else follows:

- In the **bass** the fundamental does not exist on the instrument and would not survive a phone
  microphone anyway, so `f0` is *inferred* from partials 4–12 through the model. Inharmonicity is
  therefore a prerequisite for bass pitch detection, not a feature layered on top.
- In the **treble** there are only two or three partials — too few to fit both `f0` and `B`, since
  a line needs three points. `B` is borrowed from the notes below, which is why a top note's pitch
  is only as good as the notes beneath it. A 20% stiffness error moves A7 by **7.7 cents**.

**The curve solver is linear, which is not obvious.** Stating each interval requirement as
*"preserve the beating the temperament already intends"* rather than *"be beatless"* makes the
just ratio cancel, leaving `x_j − x_i = 600·log2(1+B_i·m²) − 600·log2(1+B_j·n²)`. Linear in cents,
so the whole thing is one weighted least-squares solve. With `B = 0` it collapses to exactly equal
temperament, which is the test that it is right.

**The meter is a tracker, not a detector.** By the time a pin is being set the note, its stiffness
and its target are all known, so every partial's wanted frequency is arithmetic. The only question
left is how far one component is from it. That is why the reading is steady where a general-purpose
tuner's dances: consecutive updates ask the same narrow question of overlapping audio.

---

## Things that will bite you

Each of these was found the hard way. The constants involved are calibrated, not chosen.

- **An estimator asked to find something will find it.** Nearly every serious bug in this project
  produced a *confident wrong answer*, not a noisy one. A misnumbered partial fits its own series
  beautifully and puts `f0` a whole tone away with every quality signal looking excellent. Guard
  accordingly, and prefer declining to answer over answering wrongly.
- **Corroboration proves some note is sounding coherently, never that it is yours.** Keeping
  several peaks per band was tried so the meter could find a quiet self-agreeing set under a loud
  interloper. It made things worse: a bass note a major third away lands its 5th partial in our
  4th and its 10th in our 8th at the same offset, corroborating itself perfectly. One peak per
  band; say nothing when another string is drowning the one being tuned.
- **An onset detector is not a strike detector.** Knowing the level rose says nothing about
  whether a note was played. A UI that advances on that alone will silently mis-file data rather
  than obviously fail — which is exactly what it did.
- **Every quantity this app reports is relative to something, and the something has to be on
  screen.** Stretch is relative to equal temperament at the chosen reference; the reference is
  relative to 440. A number without its frame reads as an error to anyone holding a second tuner,
  and the owner checks against one.
- **A model must not change shape because of an accident of which notes happened to be
  measurable.** An extrapolation clamp tied to the highest sampled note did exactly that and cost
  9.3 cents in the top octave.
- **Confidence thresholds calibrated on synthetic single strings are wrong.** Real notes have two
  or three strings and genuinely wobble. The first real recordings marked almost the whole
  instrument untrustworthy; the measurements were fine and the yardstick was not.
- **A note already tuned in a session must never move.** The model keeps improving, but only for
  notes not yet reached. Reference pitch and stretch lock as soon as anything is tuned.
- **Views into WASM memory must be re-taken after every allocation.** Growing the heap detaches
  them silently — zeroed or throwing reads, never an error that names itself.
- **Layouts in `crates/wasm/src/lib.rs` and `wasm/stiffstring.js` must change together**, and
  `ABI_VERSION` bumped. The loader checks it, so a mismatch fails at load instead of corrupting
  numbers.

## Decisions that were tested and should not be re-litigated

- **Middle-outward tuning order was tried and rejected.** It spreads tension evenly across the
  plate and is confusing to follow at the piano. The sequence is temperament F3→F4, treble
  F#4→C8, bass E3→A0, always the next key along.
- **Per-note overrides are local**, applied after the solve and blended over ~14 keys. Feeding
  them into the global solve reshaped the instrument — a 2-cent treble nudge moved A0 by 1 cent.
- **Measure unisons as they sound; do not ask for mutes routinely.** Upper partials separate two
  close strings far better than the fundamental does, and the data most needed for `B` is the data
  least damaged by a rough unison. Unison spread comes out as a side effect.
- **No wasm-bindgen, no npm, no JavaScript build step.** The entire build is two cargo commands.
  This is deliberate and keeps the owner's toolchain to one installer.

---

## Where things are

    PLAN.md              the full reasoning — 28 sections, decisions D1–D23 with revisions
    README.md            build commands, page list, status table
    crates/core/src/
      synth.rs           ground-truth tone generator; everything is graded against it
      fft.rs             radix-2 FFT, windows, parabolic peak interpolation
      estimate.rs        phase-based precision estimation, beat detection
      inharmonicity.rs   per-note f0 and B, partial assignment, diagnostics
      piano.rs           inharmonicity across 88 keys, bass break, note choosing
      curve.rs           the weighted least-squares tuning solver
      meter.rs           the live tracker
      detect.rs          which key was just struck — matched against the piano's own 88 targets
      wav.rs             parsing-only WAV decode, so it compiles to WASM too
    crates/wasm/src/lib.rs   the C ABI — layouts documented on each function
    wasm/                    compiled module + loader, committed because Pages serves them
    corpus/                  real recordings; the regression set, which only grows
    measure/ recorder/ engine-check/ spikes/   the pages

### Reports worth running

    cargo run -p stiffstring-core --example accuracy        --release
    cargo run -p stiffstring-core --example tuning_curve    --release
    cargo run -p stiffstring-core --example measure_corpus  --release -- corpus/<folder>
    cargo run -p stiffstring-core --example meter_check     --release -- corpus/<folder>
    cargo run -p stiffstring-core --example intervals       --release -- corpus/<folder> [a4Hz]
    cargo run -p stiffstring-core --example compare_sessions --release -- corpus/<a> corpus/<b>
    cargo run -p stiffstring-core --example identify_corpus  --release -- corpus/<folder> [model folder]
    cargo run -p stiffstring-core --example capture_length   --release -- corpus/<folder>

`intervals` is the one that grades a *tuning* rather than the engine: it reports what each octave
actually beats at, beside what the solver would have asked for.

`capture_length` measures every recorded note from only its first part, cut from just before the
blow, and compares with the whole recording. It set the measuring pass's capture lengths
(2026-11-22): 1.2 s gives every note from F#1 up within about a cent and its stiffness within a few
percent; the deep bass wants 1.6 s. It also showed C7 and up cannot be measured at all without a
stiffness lent from below, which the page now does (`lentStiffness`), and that C8 usually cannot be
measured even then. Several 2026-09-04 recordings hold a quieter strike or seconds of room before
the real blow, which is why the cut is taken at the loudest blow, not the file start.

**Measuring runs up the keyboard, every key, A0 to C8** (2026-11-22, at the owner's request: the
old survey of ~22 scattered notes made him hunt for keys). Each note: onset (a jump of 8 dB over
what was sounding, so the last note's tail cannot start a capture), 1.2–1.6 s captured, measured,
checked against the note below for a wrong key, and on. About 2 s a note, under 3 minutes for 88.

`identify_corpus` grades the key detector (`detect.rs`) — the "follow the key I play" feature —
in the situations it meets at the piano: from silence, moving on from a neighbour or octave, the
same key again, the string being tuned 30–60 cents out, and struck over the last note ringing. A
WRONG is a switch to a key that was not struck; staying put only costs a tap. As of 2026-11-22,
across all three sessions and with the model measured on a different day: no wrong switches in
ordinary playing. The remaining misses are strings pushed 60 cents out, which physics cannot
separate from the neighbour.

**`grand-at-home-2026-09-04/D#3-key31.wav` holds C#3, not D#3.** Found by the detector, confirmed
by `measure_note`, which cannot measure it as D#3. The report skips it. `E1-key8.wav` in the same
session has no silence before the blow and C1 still ringing, so it cannot be graded fairly.

### Environment notes

- Rust is on `stable-x86_64-pc-windows-gnu`, not MSVC — the GNU target bundles its own linker and
  this project has no other use for Visual Studio.
- Neither `cargo` nor `gh` is on `PATH` in spawned shells. Prepend `$USERPROFILE\.cargo\bin`, and
  invoke GitHub CLI as `"C:\Program Files\GitHub CLI\gh.exe"`.
- There is no Node and no Python on the machine. To serve the pages locally for testing, a
  throwaway `System.Net.HttpListener` script in PowerShell works fine.
- Deploy is `git push`. Pages rebuilds in under a minute.

---

## If you are picking this up cold

Read [`PLAN.md`](PLAN.md) §1–§13 for what was intended, then §25–§28 for where it actually landed.
Run `cargo test --all` and `measure_corpus` against a corpus folder; if both are clean the engine
is healthy.

Then do the thing the project most needs and has never had: **get it used on a real job and find
out what breaks.** Every genuine fault in the last two months was found that way, and none of them
by testing.

**Pitch correction (owner's rule).** In the pitch sheet. When on, every note aims past its own
stretched target by half of how far it measured out in the measuring pass (20¢ flat → aim 10¢
sharp; sharp notes the same way, flat), capped at a chosen limit — 10¢ recommended, up to 50¢.
Unmeasured keys take the line between measured neighbours. The meter, strobe rows and key
following all read against the aim; the tuning screen states the overpull and what a 440 tuner
will show. Switching it on or off clears the ticks (a fresh pass), and turning it on moves the
reference to 440. Code: `computeRaise`, `aimHz`, `changeRaise` in measure/index.html.

**Bass octaves (owner field report, 2026-11-23).** The owner's m3–M6 checks did not beat evenly
in the low bass. The m3–M6 test is a 6:3 test (L–T m3 and T–U M6 share T's 5th partial, so they
beat equally exactly when L6 = U3), and the solver's bass sits between 4:2 and 6:3 — about 1.3
beats/s narrow of 6:3 at partial 6 on the home grand (`--example intervals`). New setting in the
pitch sheet, `#bass`: 6:3 (default for new pianos), 4:2, or the solver's balance (what pianos saved
before this get, so their targets do not move). `layBass` lays E3 down to A0 as pure octaves from
the note above, using the curve's own B for both notes. On the home grand's 09-06 stiffness, 6:3
puts A0 at −40.8¢ against −14.7¢ balanced; the E3–F3 join is 1.6¢. Done in the page, not the Rust
solver, so no ABI change.

**Meter delay.** The reading window is now 50 cycles of the followed partial, 0.2–0.5 s (was a
fixed 0.5 s), the display median is over 3 readings (was 8), and updates come every 40 ms or 1.8×
the cost. On the 09-06 recordings 0.2 s was as steady as 0.5 s from the tenor up; C2 at 0.2 s
jumped ~7¢ between updates, hence the cycles rule. "Held after strike" follows the window.

**Wrong key at ±100¢.** When following misses a blow, the meter (searching ±150¢) reads the new
string as the old key a semitone out. `neighbourFor` checks any reading 45¢+ out against where
keys ±1, ±2 are expected (`expectCents`); if one explains it within 35¢ and 30¢ better, it follows
there after 2 updates, or says "That sounds like X" when following is off.
