# Piano Tuning Instrument — Plan and Decisions Log

> **Status: v1 built (phases 0a–6).** Measure a piano, solve its tuning, meter
> each note against it. Sections 1–13 are the original plan, written before any
> code; 14 onward record what each phase actually produced, including the parts
> the plan got wrong. Decisions carry numbers (D1–D23) and are revised in place
> where later work overturned them.
>
> This document lived outside the repository until 2026-11-12 and was lost when
> that directory was cleaned up. It was recovered from session transcripts and
> now lives here, versioned with the code it describes.

---

## 1. Context

The owner — a working piano technician and a non-programmer — wants a professional-grade piano
tuning application for iPhone. Inspired by the *category* of tools like Reyburn CyberTuner, but
explicitly **not a clone**: no proprietary algorithms, source, branding, artwork, interface,
terminology, or protected methods. Our own product, built from published acoustics and DSP.

It is a **measurement instrument**, not a chromatic tuner with a piano skin. Core claim: measure an
individual acoustic piano's string inharmonicity, then compute tuning targets appropriate to *that*
instrument.

The owner will not hand-write code. Claude creates code, modifies files, diagnoses errors, fixes
bugs, manages project structure, and gives plain-language install / run / test instructions.

### Owner profile

| | |
|---|---|
| Hardware | Windows 11 desktop + iPhone. **No Mac**, no Xcode, no Node, no working Python |
| Tuning background | **Working technician, fully aural.** Fork/reference then entirely by ear |
| Commercial intent | Personal first, commercial door left open |
| Test fleet | Several varied pianos. **No professional ETD to cross-check against** |
| Pianos worked on | Home uprights/spinets, small–medium grands, old/neglected instruments |
| Available time | Steady, several hours a week |

Two consequences dominate everything below:

1. **The owner is fully aural with no ETD.** He has no electronic habits for us to accidentally
   replicate, and his judgment is independent of any existing product's opinion. He is a clean-room
   validator — and, as §6 explains, the ground truth for the tuning curve itself.
2. **He works on the hardest piano population.** Not concert instruments. Short strings, extreme
   inharmonicity, false beats, rust, unstable pins. Robustness is a core engine requirement rather
   than polish — but the product's value is also highest here, because generic equal temperament
   sounds worst on exactly these pianos.

---

## 2. Product architecture — three layers

| Layer | What it is | Risk | Volume |
|---|---|---|---|
| 1. Measurement engine | Precise partial extraction from decaying piano tones | Highest | Small |
| 2. Tuning model | Inharmonicity → 88 musically defensible targets | High | Small |
| 3. Technician product | Meter, workflow, unisons, pitch raise, records | Low | Large |

Risk concentrates in 1–2; effort concentrates in 3. The roadmap is ordered by risk, not by volume.
Layers 1 and 2 are also where the product's originality and defensibility live.

---

## 3. Platform (RATIFIED)

**One engine in Rust, compiled to several targets.**

- **Rust core** — DSP, inharmonicity fitting, curve solver. Zero platform dependencies.
- → **native Windows binary** for the test harness, where all validation runs (fast, scriptable)
- → **WebAssembly** for an installable iPhone web app — the actual product
- → **native iOS/Android later** if ever commercialized. Same core, unchanged, no rewrite

The iPhone app is a web app installed to the home screen: app icon, full-screen, offline-capable.
**No Mac, no Xcode, no App Store review, no developer account, no update approval delays.**

*Rust over C++*: single-command builds with no build-system configuration, best-in-class WASM
tooling, testing built into the language, and a compiler that fails loudly rather than emitting
subtly wrong numbers. Accelerate/vDSP deliberately skipped — a few FFTs per second needs no
acceleration, and it would tie the core to Apple for nothing. Flutter rejected: it puts a plugin
boundary in the middle of the audio path, exactly where reliability fails.

### Risks in the web path — unverified, tested first

1. **iOS Safari raw microphone access.** A page can *request* audio with automatic gain control,
   noise suppression and echo cancellation disabled. Whether Safari honors it is unconfirmed and
   varies. **This is phase 0a and it gates everything.** If Safari mangles the audio irreversibly,
   the web path dies for measurement and a Mac becomes necessary. Find out in week one.
2. Screen sleep must be blocked mid-tuning (Wake Lock API; recent iOS).
3. iOS can evict browser storage → export/backup for records designed in from day one.
4. Microphone access requires HTTPS → free static hosting (GitHub Pages).

---

## 4. Audio processing design

### Why standard pitch detection is the wrong tool

YIN, autocorrelation, and every general-purpose pitch detector work by finding a single repeating
period — which assumes partials sit at exact integer multiples of the fundamental. That assumption
is *precisely what is false* about a stiff piano string. They return a biased answer **and** discard
the partial structure we need. Useful only as a coarse initial guess; never as the engine.

### Where precision comes from

Required: ~0.1 cent ≈ 0.025 Hz at A4. An FFT bin at 48 kHz / 4096 is ~12 Hz. Bin-peaking cannot get
there, and no window function fixes it.

The resolution limit does not apply, because we are not *resolving* adjacent tones — we are
*estimating* the frequency of a well-isolated sinusoid. Piano partials sit far apart in their own
spectral neighborhoods, so each is individually well-conditioned. Estimation precision improves with
SNR and with observation time to roughly the 3/2 power. **Precision therefore comes from watching
how fast each partial's phase advances across successive overlapping frames** — a phase-based
estimator — not from larger transforms.

### Two paths, deliberately separate

**A. Measurement path — slow, patient, precise.** Triggered by note onset. Captures ~1.5 s of decay.
- FFT with a low-sidelobe window (Blackman–Harris family) to *locate* partials
- Phase-advance estimation between overlapping frames to *pin* each partial's frequency
- Quadratic log-magnitude interpolation as an independent cross-check
- Weighted robust fit of the located partials to `f_n = n·f0·√(1 + B·n²)`

**B. Live meter path — fast, cheap, rock-solid. The meter is a tracker, not a detector.**
Once the note is locked and its f0 and B are known, the expected partial frequencies are known
exactly. We are not asking "what note is this and what is its pitch?" — we are asking the far
easier question "how far is *this specific partial* from where I want it?" Narrowband tracking of
one or two strong partials, ~20 updates/second. This is why the meter can be steady instead of
dancing, which is the characteristic failure of general-purpose tuners.

**Trying to make one path serve both purposes produces something mediocre at each. This split is
the single most important architectural decision in the audio layer.**

### Register-specific strategies — one algorithm does not fit the piano

| Register | Strategy |
|---|---|
| Bass (below ~C2) | **Fundamental is unusable** — weak on the instrument, filtered by phone mics (A0 = 27.5 Hz). Analyze partials ~4–12 and *infer* f0 through the stiffness model. **This is why inharmonicity is a prerequisite for bass pitch detection, not a feature layered on top.** |
| Tenor / middle | Best case. Many strong partials, clean fitting, richest data. Anchor the model here |
| Treble (above ~C7) | Few partials, dying fast. Short windows, partials 1–4, and lean on the *interpolated* B model rather than measuring B per note |

### Note onset, locking, and rejecting sympathetic resonance

Energy-based onset detection, then spectral pattern matching against the expected partial sets of
candidate notes. Because the tuning sequence is known, candidates are biased toward the expected
next note. Sympathetic resonance from undamped strings is rejected by requiring a sharp attack
transient — resonating strings swell, struck strings strike.

Auto-advance along a known sequence, with **manual lock always one thumb away**. Open-ended
automatic recognition jumps to the wrong note exactly when concentration matters most.

### Confidence and bad-data rejection — a first-class requirement

Given the target piano population, a meaningful share of the engine is about *recognizing and
discarding measurements that lie.*

Detected and reported: false beats (near-degenerate string modes causing partial wobble),
low partial SNR, high model-fit residual, frame-to-frame inconsistency, microphone clipping,
excessive room noise, pitch drift during the measurement window.

**Decision: findings are surfaced, not hidden.** The app names what it found — "false beat on this
string," "unreliable reading" — and lets the technician decide. This makes it a diagnostic tool as
well as a tuner, and gives him something concrete to show a client about why their piano fights him.

### The sample clock is not a problem

Frequency accuracy depends on the device's sample clock, not microphone quality. A constant clock
error shifts every measurement identically, so it **cancels out of all interval relationships** and
only nudges absolute pitch. Worth measuring once; not worth engineering around.

---

## 5. Inharmonicity measurement protocol

**Budget: 2–3 minutes, ~10–16 notes.**

- **Adaptive sampling.** Begin with ~8 anchor notes spread across the compass. The app then requests
  extra samples where the fit residual is high — above all **bracketing the bass break**, whose
  location differs on every piano and which is violent on a spinet.
- **Partials per note:** as many as carry usable SNR — typically 6–12 in bass/tenor, 2–5 in treble.
- **Per note:** ~1.5 s of usable decay, re-strike always permitted.
- **Estimating B — linearize first, then refine.** From the model, `(f_n/n)² = f0²(1 + B·n²)`.
  Plotting `(f_n/n)²` against `n²` gives a **straight line**: intercept `f0²`, slope `f0²·B`. Plain
  linear regression yields a robust starting estimate; a weighted nonlinear fit with a robust loss
  then refines it and rejects outliers. Fast, stable, and easy to inspect when it misbehaves.
- **Extending across 88 notes:** log(B) is smooth *within* a string section and discontinuous at the
  wound/plain break. Fit a piecewise-smooth curve, detecting the break from a jump in residuals.
  Spinets exaggerate the characteristic shape — high at the bottom, minimum near the tenor, rising
  through the treble — so smoothness constraints need teeth and stretch may need clamping.
- **Continuous refinement:** every note tuned yields another measurement, which improves the model.
  **Targets for notes already tuned in this session never change.** Refinements apply only to
  untuned notes. Moving a target under the technician's hand is unacceptable.
### Unisons during measurement — measure as-is (D18)

No routine muting. The estimator instead models each partial as possibly **two or three closely
spaced components** rather than one isolated sinusoid.

*Why this is workable:* two strings 2 cents apart at A4 differ by ~0.5 Hz — needing ~2 s to
separate, which is marginal. But **higher partials pull them apart**: the same 2-cent spread becomes
~2 Hz at the 4th partial, resolvable in ~0.5 s. The upper partials are exactly what we rely on for
B estimation — overwhelmingly so in the bass (partials 4–12). *The data we most need is the data
least damaged by rough unisons.*

*Bonus:* unison spread is measured as a **side effect** of measuring inharmonicity. That directly
supplies unison quality for the client report (§8a), string-level diagnostics, and most of the
groundwork for phase 7 unison assistance. The cheaper choice is also the more capable one.

*Caveat:* badly split unisons (5+ cents) on neglected pianos can split high partials far enough to
be mistaken for separate partials. Needs care in the peak-association logic.

*Escape hatch:* if a note is so far out with itself that no clean reading is possible, the app says
so and asks for a mute on that note only. Rare, and consistent with the surfaced-diagnostics posture
(D13).

---

## 6. Tuning curve solver

### The central conflict

Because of inharmonicity the interval families **provably disagree** — not imprecisely, genuinely.
A 4:2 octave is wider than 2:1 by an amount string stiffness dictates; a 6:3 octave is wider still;
a pure twelfth gives a third answer. Individually a cent or two apart; stacked up the keyboard they
compound to several cents at the extremes. Every stretch scheme in existence is a *choice* about
which to favor, and where.

### The owner's choice: progressive thirds in the middle

Thirds and tenths lead through the temperament and mid-range; octaves absorb the compromise there.

This resolves into a **register-dependent weighting scheme, and that is principled rather than
convenient**: thirds are only *usable* from roughly the bottom of the bass staff to the top of the
treble staff. Below, the relevant partials are weak and muddy. Above, third beat rates exceed ~20/s
and stop being beats at all — they become roughness. So thirds and tenths dominate the middle where
they can be heard; octaves and twelfths take over at the extremes where thirds have nothing to say.
The smoothness term knits the regions together so there is no audible seam at the handoff.

### The formulation — it comes out linear

Work in **cents**, not hertz. Unknowns: `x_1 … x_88`, the fundamental of each note in cents.

For any interval requirement — partial *m* of note *i* coinciding with partial *n* of note *j* —
beat-free means:

```
x_i + 1200·log2( m·√(1+B_i·m²) )  =  x_j + 1200·log2( n·√(1+B_j·n²) )
```

Everything involving B is a **known constant** once measured, so this rearranges to:

```
x_j − x_i  =  D_ij        (D_ij computed from the measured inharmonicity)
```

**Linear in the unknowns.** Assemble one weighted equation per interval requirement across octaves,
double octaves, twelfths, fifths, fourths, thirds, tenths, seventeenths; add a smoothness penalty on
the second difference of `x`; add one pitch anchor. Solve as weighted least squares.

Properties that matter: fast, deterministic (same answer every time), a unique solution, and
**fully explainable** — every target traces back to the interval requirements that produced it and
how much each was satisfied. That explainability is a product feature, not just a debugging aid.

### Overrides and calibration are the same mechanism

A per-note override is simply another equation, `x_k = your_value`, with a high weight. It drops
straight into the system.

And a full aural tuning is the same thing at density — a dense set of corrections rather than a
sparse one. So **one learning mechanism serves both**: fitting the interval weights to reproduce the
owner's own tunings. Overrides remembered per piano; repeated overrides in the same register feed
back into how curves are computed for him generally. The app ends up expressing *his* aesthetic
generalized to pianos he hasn't tuned yet, rather than a generic opinion about stretch.

### Error units: beats per second (D19)

Interval error is weighted by **audible beating**, not by cents — the unit the ear actually
perceives. Since equal cent error beats faster at higher pitch, this makes the solver work harder in
the upper registers.

**This costs nothing structurally.** Beat rate is cents error scaled by the frequency at which the
partials coincide, so it enters as a per-equation weight (`w ∝ f_coincident`). The system stays
linear, the solve stays fast, deterministic and explainable. The more musically honest unit comes
free.

---

## 7. Pitch reference policy

**Measure, recommend, let him overrule.** The app determines where the piano actually sits, then
suggests a reference — full A4=440, or a compromise pitch — with its reasoning, and accepts being
overruled. On a neglected spinet sitting 40 cents flat, dragging it all the way up in one visit is
often the wrong call, and that judgment stays with the technician. Neither guesses silently nor nags.

---

## 8. Data model

```
Customer
 └── Piano            make, model, type (spinet/upright/grand), size, serial, scale notes
      ├── Measurements        dated; per-note partials, estimated B, confidence, diagnostics
      ├── InharmonicityModel  fitted B across 88 notes, detected bass break
      ├── TuningCurve         88 targets, solver weights used, per-note overrides
      └── TuningSessions      dated
           ├── NoteResults    before/after cents, stability, unison state
           ├── PitchReference chosen A4 and rationale
           └── TechnicianNotes
```

Opening a customer's piano shows previous tuning history, how far it had drifted, and which strings
have caused trouble before. **Export/backup is designed in from day one** — iOS can evict browser
storage, and customer records must never be at the mercy of that.

### 8a. Client report (D20)

A plain, non-technical summary the technician can show a client: how far the piano had drifted since
last visit, which strings misbehave (false beats, won't hold), unison condition, and whether the
instrument is holding pitch. Justifies the work and explains why a neglected piano needs more of it.
Commercially useful later. **Build the diagnostic data properly during v1; the report itself is a
small deliverable that can follow.**

---

## 8b. Technician workflow — opening the app to a finished tuning

**Setup.** Opens to the last piano used, or a new one; one tap to resume. For a new instrument,
minimal identification (make, type, size) — the rest can be filled in later or never.

**Pitch check.** Play a few notes across the compass. The app reports where the piano actually sits
and recommends a reference pitch *with its reasoning*. Accept or overrule (D14).

**Measurement pass (2–3 min).** The app names a note in large type; you play it; it takes the
reading and moves on. Onset-triggered, no buttons. Adds extra notes where data is thin — especially
bracketing the bass break. Bad readings are called out immediately, never silently swallowed (D13).

**Review.** The computed curve appears as a picture. Accept, or adjust overall stretch.

**Tune.** Full screen, dark, one number dominant. Strike a note → the meter locks and holds steady
(tracker, not detector — D16) → set the pin → when the reading sits inside tolerance for a moment
the app marks the note done and advances. A strip along the bottom shows all 88 keys and their
state. Long-press nudges a target if you disagree; remembered for this piano and fed back into
weight calibration (D15).

**Finish.** Session saved to the piano's history. Diagnostics summarized. Client report optional.

### Governing ergonomic rules

- Readable at arm's length; **never requires two hands**
- Manual note lock always one thumb away; auto-advance is a convenience, never a cage
- No animation anywhere except the meter itself
- Dark, high contrast; the cents reading is the most important pixel on the screen
- **A target for a note already tuned this session never changes.** The model keeps improving, but
  only for notes not yet reached. Moving a target under the technician's hand is unacceptable

---

## 9. Development phases

Ordered by risk. Each phase has a gate that must pass before the next begins.

| # | Phase | Gate |
|---|---|---|
| **0a** | **iOS Safari audio-integrity spike** | Raw unprocessed mic audio confirmed on the owner's iPhone — or the web path is rejected and we replan |
| 0b | Synthetic piano-tone generator + offline test rig | Ground truth we trust: known f0, B, decay, noise, false beats, clipping |
| 1 | Audio capture + verified pipeline | Measured frequency matches a known generated reference |
| 2 | Single-partial precision estimator | Meets accuracy targets on synthetic signals (§10) |
| 3 | Multi-partial tracking + per-note B estimation | Correct on synthetic, then on real recordings |
| 4 | Whole-piano measurement pass → B across 88 notes | Stable and repeatable across sessions on one piano |
| 5 | Tuning curve solver | Numerically sound + reproduces the owner's aural tunings within his own repeatability |
| 6 | Live meter + tuning workflow | **A complete real tuning gets done with it** — this is v1 |
| 7 | Unison assistance | Field-tested |
| 8 | Pitch raise assistance | Field-tested on a badly flat piano |
| 9 | Customers, pianos, session history | — |
| 10 | Polish; possible commercial pivot | — |

**v1 = phases 0a–6.** Measure the piano → compute 88 targets → rock-solid meter. The owner supplies
all tuning judgment. Pitch raise, unisons and records are explicitly post-v1.

Phases 0a–5 need no Mac and largely no piano — they run on the Windows machine against synthetic
signals and recordings. Phase 0a needs only the iPhone and a link.

---

## 10. Testing strategy

Numerical wherever possible; aural only where nothing else will do.

### Layer 1 ground truth — synthesis

A generator producing piano-like tones with *mathematically known* f0, B, per-partial decay rates,
double decay, added room noise, false beats, and clipping. Every estimator is graded against signals
whose true answer we already know. This is why phase 0b precedes all real audio work — without it we
would be tuning algorithms by intuition.

### Layer 2 ground truth — the owner's ears, made numerical

The owner tunes a piano by ear as he normally would. We then measure all 88 resulting notes
precisely. **That measured tuning becomes a target curve**, and solver quality becomes a number:
mean and maximum deviation in cents from a tuning his ears have already blessed.

**Required control experiment — schedule early.** He tunes the same piano twice, some weeks apart,
and we measure both. The difference between them is *his own repeatability*, and it sets the
**accuracy floor for the entire project**. No algorithm can be asked to match his intent more
closely than he matches himself. This number tells us when to stop optimizing.

### Regression corpus

Every real note he records is archived with its analysis. The corpus only grows, and every future
change is re-run against all of it. Old neglected pianos with false beats are the most valuable
recordings we can collect, not the least.

### Accuracy targets

| Measurement | Target |
|---|---|
| Partial estimation, clean synthetic (40 dB SNR, 1 s) | < 0.02 cents |
| Partial estimation, noisy synthetic (20 dB SNR) | < 0.1 cents |
| f0 recovered from partials, synthetic | < 0.1 cents |
| B recovered, synthetic | within 5% relative, and < 0.5 cent effect on targets |
| Repeat measurement of one real note, ×10 | scatter (SD) < 0.15 cents |
| Same piano measured twice in one day | per-note targets agree within 0.3 cents |
| Live meter on a steady tone | wander < 0.2 cents; updates within ~100 ms |
| Note identification within a known sequence | > 99% correct |
| **Solver vs. the owner's aural tuning** | **mean ≤ his measured repeatability; max ≤ 2×** |
| Field | he completes a real tuning and judges the result good |

---

## 11. Open questions

- **Product name.** Deferred by the owner. Working codename: **Stiffstring** (string stiffness is
  the physical cause of inharmonicity — the thing the whole product exists to measure). Disposable;
  nothing in the build depends on it.
- Tuning sequence default (middle-out vs chromatic) — round 5.
- Note-completion criterion: automatic on stability, or manual confirmation — round 5.
- Numeric v1 accuracy bar — proposed in §10, not yet ratified by the owner.
- Historical temperaments — trivial to add later as an offset table. Not in v1.
- Whether records eventually sync across devices.

---

## 12. Decisions log

| # | Decision | Date |
|---|---|---|
| D1 | Owner is a working technician → the aural validation authority | 2026-09-02 |
| D2 | No reference ETD → synthetic ground truth (0b) is mandatory | 2026-09-02 |
| D3 | Personal first; licensing clean and data exportable so a commercial pivot stays open | 2026-09-02 |
| D4 | Platform: single Rust core → WASM installable iPhone web app as the real product; native Windows test harness; native mobile later unchanged | 2026-09-02 |
| D5 | Safari microphone integrity is phase 0a and gates everything | 2026-09-02 |
| D6 | Solver is weighted multi-interval least squares — the owner relies on all four interval families and they provably conflict | 2026-09-02 |
| D7 | v1 scope: measure → 88 targets → meter. Pitch raise, unisons, records are post-v1 | 2026-09-02 |
| D8 | The owner's aural tunings are layer-2 ground truth and the weight-calibration target; his two-tunings repeatability sets the accuracy floor | 2026-09-02 |
| D9 | Curve favors progressive thirds/tenths in the middle via register-dependent weights | 2026-09-02 |
| D10 | Measurement pass: 2–3 minutes, ~10–16 notes, adaptively chosen | 2026-09-02 |
| D11 | Target instruments are spinets, uprights, small–medium grands, neglected pianos → bad-data rejection is a first-class engine requirement | 2026-09-02 |
| D12 | Readout: cents primary, beat rates available | 2026-09-02 |
| D13 | Diagnostics are surfaced, not hidden — the app names what it found and lets the technician decide | 2026-09-02 |
| D14 | Pitch reference: measure, recommend with reasoning, accept being overruled | 2026-09-02 |
| D15 | Per-note overrides are remembered per piano *and* feed back into weight calibration — one mechanism shared with aural-tuning calibration | 2026-09-02 |
| D16 | Meter is a tracker (deviation from a known target), not a detector | 2026-09-02 |
| D17 | Measurement path and live-meter path are architecturally separate | 2026-09-02 |
| D18 | Measure unisons as-is; estimator models each partial as possibly 2–3 close components. Yields unison spread for free. Mute requested only when a reading is otherwise impossible | 2026-09-02 |
| D19 | Interval error weighted in beats per second, not cents — enters as a per-equation weight, solver stays linear | 2026-09-02 |
| D20 | Client-facing report is wanted; build the diagnostic data in v1, the report follows | 2026-09-02 |
| D21 | Product name deferred; working codename **Stiffstring** | 2026-09-02 |
| D22 | Tuning sequence: **middle outward**, distributing tension change more evenly across the plate | 2026-09-02 |
| D23 | A note completes **automatically once it settles inside tolerance** — no interaction, both hands stay free | 2026-09-02 |

---

## 13. The owner's side of this

### One-time setup — about twenty minutes, once

| Install | Why | How |
|---|---|---|
| **Rust** (`rustup-init.exe`) | Builds the engine, both native and WASM | One installer, click through defaults |
| **Node.js** (LTS) | Dev server and packaging the web app | One installer, click through defaults |
| **GitHub account** (free) | HTTPS hosting so the iPhone can load the app — microphone access requires it | Sign up, no payment |

That is the entire toolchain. Claude does everything else.

### What running things looks like

Claude supplies one command at a time in a copy-and-run block — almost always `cargo test`,
`cargo run`, or `npm run dev`. The owner never edits code, never copies fragments between files,
never debugs, and never needs to understand Rust.

### What the owner actually does, by phase

| Phase | The owner's part | Time |
|---|---|---|
| **0a** | Install the toolchain. Open a link on the iPhone, allow the microphone, play a few notes and a steady tone. Report what the screen says | ~45 min, once |
| 0b–2 | Almost nothing — synthetic signals, no piano needed | — |
| **3** | **Record individual notes on real pianos.** ~20–30 single strikes spread across the compass, quiet room, phone in a consistent position. Ideally 3–4 instruments including a spinet and a grand. These become the permanent regression corpus and are never re-recorded | ~15 min per piano |
| **4** | Run the measurement pass on real pianos and report where it struggles. **From here on, measure every piano tuned as part of normal work** — the corpus builds itself out of jobs already happening | ~10 min per piano |
| **5** | **The important one.** Tune a piano by ear as normal, then let the app measure all 88 resulting notes. Repeat on a second instrument. **Plus the repeatability control: re-tune one of them some weeks later and measure again** | ~20 min beyond a normal tuning |
| **6** | Use it on real jobs and say where it fails | Ongoing |

### Scheduling note — start the repeatability clock early

The control experiment (§10) requires **weeks of elapsed calendar time** between the two tunings of
one piano. If it begins in phase 5 it stalls phase 5. **Capture the first aural tuning as soon as
the measurement pass works in phase 4**, so the second one lands when it's needed.

### Rough timeline

At several hours a week. An estimate, not a promise — phase 5 is the one that could expand, and
phase 0a could invalidate the platform outright.

| Weeks | |
|---|---|
| 1 | Phase 0a — Safari audio gate |
| 2–4 | Test harness and precision estimator (mostly Claude) |
| 5–8 | Partial tracking and B estimation; first real recordings |
| 9–12 | Whole-piano measurement; corpus building begins in the field |
| 13–18 | Curve solver and calibration against the owner's tunings |
| 19–24 | Live meter and workflow → **first real tuning done with it** |

Roughly five to six months to v1. Most of the owner's hours are spent playing notes and listening,
not sitting at a computer.

### First four actions once development is authorized

1. Owner installs Rust and Node — Claude walks him through it step by step.
2. Claude builds the phase 0a Safari microphone integrity test.
3. Owner opens a link on the iPhone, plays notes, and reports the numbers.
4. **Gate:** does Safari deliver raw, unprocessed audio? If yes, the no-Mac plan holds and we
   proceed to 0b. If no, we replan around acquiring a Mac before anything else is built.

## 14. Deployment

Repository: https://github.com/jjcsgox1/stiffstring (public — required for free GitHub Pages)
Live site:  https://jjcsgox1.github.io/stiffstring/
Phase 0a:   https://jjcsgox1.github.io/stiffstring/spikes/0a-audio-integrity/

GitHub CLI is installed but NOT on PATH in spawned shells — invoke it as
`"C:\Program Files\GitHub CLI\gh.exe"`. Authenticated as **jjcsgox1**.

Deploying is `git push`; Pages rebuilds automatically in under a minute.

When the tuning algorithm becomes real, revisit publishing: keep the Rust source private and
publish only compiled WebAssembly, so a commercial pivot stays open (D3).

---

## 15. Phase 0a result — GATE PASSED (2026-09-03)

Tested on iPhone, iOS 18.7.5, Safari 18.7. Sample rate 48000. Secure context, not framed.

### Verdict: iOS Safari delivers usable, unprocessed audio. The no-Mac plan holds.

**Evidence for no AGC:** the note decayed 36 dB (-40.4 -> -76) smoothly into the noise floor and
stayed there. Gain control lifts quiet passages; the tail would have flattened or risen. It did not.

**Evidence for no aggressive noise suppression:** noise floor sat near -76 dB wandering ~1 dB, i.e.
a real room floor rather than a scrubbed, unnaturally flat silence.

**Caveat:** Safari does NOT report autoGainControl or noiseSuppression in getSettings() at all — it
exposes only echoCancellation (confirmed false). The verdict therefore rests on behaviour, not on
the browser's own word. Worth one more targeted probe (sustained constant-level tone; loud-then-
quiet step) before treating it as settled. A slight ~1 dB downward drift of the noise floor over the
first 1.7 s is unexplained — mild NS engaging, or just the room. Watch it; not blocking.

### Bonus finding 1 — the capture path preserves beats

The decay trace carries a ~0.6 Hz amplitude modulation (dip/recover every ~1.7 s) sustained for the
full 10 s while the envelope decays underneath: a unison ~0.6 Hz apart, or a false beat. Sub-hertz
AM survives the microphone path intact over 10 seconds. This directly validates **D18** — measuring
unisons as-is and getting unison spread as a side effect is demonstrably feasible.

### Bonus finding 2 — inharmonicity is already visible

One capture returned partials 220.6 / 441.6 / 662.9 / 884.7 / 1107.2 Hz. Dividing by partial number
gives 110.30 / 110.40 / 110.48 / 110.59 / 110.72 — monotonic progressive sharpening, the stiffness
signature. Fitting f_n = n*f0*sqrt(1+B*n^2) gives **f0 ~ 110.28 Hz, B ~ 8e-5**, with all five
partials landing within 0.05 Hz of the model. Physically sensible for that register of a small piano.

The NUMBER is provisional — at a 1.465 Hz bin we are reading far below our own resolution, which is
what phases 2-3 exist to fix. The SHAPE is solid.

### Data quality issues

- The capture labelled **A4 is actually A2** (frequencies match the A2 capture within 0.3 Hz) —
  either A2 was still ringing or the wrong key was struck. Needs redoing.
- **A0: fundamental (27.5 Hz) and 2nd partial absent entirely.** Strongest component is the 5th
  partial at 137.4 Hz; 3rd/4th present at -73/-67 dB. Confirms on the real instrument what the plan
  assumed: the bass must be read from upper partials.

### Still outstanding

- Screen wake lock test — not run
- A4 capture — redo
- Frequency accuracy (desktop plays 440 Hz, phone measures) — decides whether absolute pitch is
  trustworthy; a constant clock error cancels from intervals but shifts absolute pitch
- Desktop Chrome control run — lower priority now that the iPhone result is this clean

---

## 16. Phase 0b — DONE (2026-09-03)

`crates/core/src/synth.rs`, 16 tests, clippy clean. Dependency-free (own PRNG) so it compiles
unchanged to WebAssembly.

Provides `partial_hz` / `f0_from_partial` / `partial_stretch_cents`, and a renderer producing the
things that break naive estimators: differential partial decay, beating unisons, noise floor,
clipping. Partials above Nyquist are dropped, never folded back.

### Corrected inharmonicity fits from phase 0a data

Proper least squares (linearised: (f_n/n)^2 against n^2), superseding earlier eyeballed values:

| Note | Partials measured (Hz) | f0 | B |
|---|---|---|---|
| A3 | 220.6 / 441.6 / 662.9 / 884.7 / 1107.2 | 220.63 | 3.04e-4 |
| A4 | 442.1 / 885.2 / 1329.8 / 1778.2 | 441.92 | 7.33e-4 |

Ratio ~2.4x per octave — textbook plain-wire behaviour, from two independent captures. Both values
are ~10x a concert grand: short, stiff strings, i.e. a spinet or small console. A4 sits at 442.1 Hz,
~8 cents sharp of 440. Uncertainty roughly +/-20-25% (four partials through 1.465 Hz bins).

### Toolchain note

Rust defaults to the MSVC target, which needs a Visual Studio C++ linker (multi-GB) this project has
no other use for. Switched the default to **stable-x86_64-pc-windows-gnu**, which bundles its own
linker. Equivalent for a dependency-free numeric crate, and the WASM target needs no native linker
at all. Neither `cargo` nor `gh` is on PATH in spawned shells — prepend `$env:USERPROFILE\.cargo\bin`
and call gh as `"C:\Program Files\GitHub CLI\gh.exe"`.

### Next: phase 1-2 — the precision estimator

Gate: better than 0.02 cents on clean synthetic, better than 0.1 cents at 20 dB SNR. Approach per
section 4: FFT to locate partials, phase advance across overlapping frames to pin them.

---

## 17. Phases 1-2 — GATES PASSED (2026-09-03)

`crates/core/src/fft.rs` + `crates/core/src/estimate.rs`. 37 tests, clippy clean.
Re-runnable report: `cargo run -p stiffstring-core --example accuracy --release`

### Method

The FFT **locates** partials and never measures them. Precision comes from **phase**: a sinusoid
advances phase at a constant rate, so measuring it at 8 instants across a second and fitting a line
gives frequency from the slope — limited by phase measurement quality, not bin width.

Two properties make it work on real notes:
- a decaying amplitude does not disturb phase, so the note dying away is harmless
- a partial between bins adds the SAME offset to every frame, moving the intercept, not the slope

### Measured accuracy (1 s, 48 kHz, 8192-pt frames, 8 phase measurements)

| Condition | Worst error | Gate |
|---|---|---|
| Clean, 27.5-3500 Hz | 0.00008 cents | 0.020 — PASS, 250x margin |
| 20 dB SNR | 0.00259 cents | 0.100 — PASS, 38x margin |
| 0 dB SNR | 0.02585 cents | — |
| -6 dB SNR (noise louder than signal) | 0.05149 cents | — |

Decaying A3 note with a 60 dB noise floor: **B recovered to -0.05%%**, f0 to 0.0009 cents.

### What this does and does not prove

The synthetic signals are generated by exactly the model the estimator assumes — pure exponentially
decaying sinusoids. So these numbers bound **numerical precision**, not robustness to real
acoustics. Room reflections, sympathetic resonance, microphone response, false beats and the
model own imperfections are all still ahead.

The useful conclusion: **the estimator is not the bottleneck.** With ~40x margin at realistic SNR,
the limiting factor from here is physics and messy data, not arithmetic.

### Bad-data detection came free

Phase that does not advance linearly means beating strings, a false beat, or noise swamping the
partial. The straight-line fit residual therefore doubles as a quality score (`phase_residual`,
plus a heuristic `confidence`). A test confirms a 6-cent beating pair drops confidence below 0.7
while a clean tone sits above 0.95. This is **D11/D13 falling out of the same machinery** rather
than being bolted on — significant for the neglected pianos this tool targets.

### Fixed along the way

Single-bin amplitudes under-read by up to 0.8 dB when a partial falls between bins (scalloping
loss), which would have made a partial apparent strength depend on where it happened to land.
Now interpolated across the peak; shared `fft::parabolic_peak` used by both coarse and fine paths.

### Next: phase 3 — multi-partial tracking and per-note B estimation

Weighted robust fit with outlier rejection, false-beat handling, and the bass strategy (infer f0
from partials 4-12 where the fundamental is absent — confirmed absent on the real A0 in phase 0a).

---

## 18. Phase 3 — GATE PASSED (2026-09-03)

`crates/core/src/inharmonicity.rs`. 48 tests, clippy clean.

### The real problem was assignment, not measurement

`estimate::refine` pins a partial down once told where to look, but cannot say WHICH partial it
found. Since B is recovered from how partial frequencies grow with partial number, a misnumbered
partial produces a **confident wrong answer**, not a noisy one.

Assignment would be trivial if partials sat at exact multiples. The deviation grows as n^2, and by
the 10th partial of a stiff bass string it exceeds half the spacing between neighbours — exactly the
margin assignment depends on.

Solution: we already know roughly which note was struck (the technician is working a sequence).
That hint, plus a grid search over plausible B, fixes the numbering; the partials then determine f0
far more precisely than the hint did. **Verified tolerant of a hint 120 cents wrong** — which a badly
flat piano will produce.

### Fitting

Linearised solve for a starting point, then Gauss-Newton on residuals in **cents** with Huber
reweighting, so one false-beating partial pulls with bounded force rather than dragging the answer.
Outliers past a threshold are dropped and the fit repeated without them.

### Measured across a simulated spinet (A0-C7, 65 dB noise floor)

| | Worst |
|---|---|
| Fundamental | 0.0028 cents |
| Stiffness B | 0.07%% |
| Time per note | 129-354 ms (native) |

Timing is comfortable against the 2-3 minute measurement budget (D10): even at 3x slower under
WASM, 16 notes is a few seconds of compute. The budget is dominated by the technician playing.

### Two fixes found by testing

1. **Noise matched as partials.** The matcher paired absent partials with whatever noise sat
   nearest, leaving the fitter to reject them. Added `estimate::noise_floor` (median spectral bin —
   robust because most bins are noise) and a threshold below which nothing counts as a partial.

2. **Unweighted phase regression hurt the treble.** Treble partials die within ~0.5 s, so later
   frames read phase out of noise and degraded a fit the early frames had already settled. Now
   weighted by amplitude squared — statistically correct since phase variance goes as 1/amplitude^2,
   and it shortens the effective window exactly as far as each note decay demands, with no
   register-specific special casing. **Treble B error 0.95%% -> 0.07%%**, spurious warnings cleared.

### Diagnostics (D13) now emitted per note

`FundamentalMissing` (normal in bass, notable higher), `FewPartials`, `PoorFit`, `UnstablePartials`
(beating/false beat), `PartialsRejected`. A clean note raises none; a 6-cent beating unison shakes
partial confidences as intended.

### Next: phase 4 — whole-piano measurement pass

Adaptive note selection (~10-16 notes, D10), fitting log(B) across all 88 with a detected
bass/plain-wire break, and the smoothness constraints spinets demand. Gate: stable and repeatable
across sessions on one piano — the first phase needing real recordings.

---

## 19. Phase 4 — inharmonicity across 88 keys (2026-09-03)

`crates/core/src/piano.rs`. 61 tests, clippy clean.
Report: `cargo run -p stiffstring-core --example inharmonicity_curve --release`

### Results against a reference small piano

The reference carries a smooth wobble the model structurally CANNOT represent — a reference matching
the model exactly would prove only that the arithmetic works.

| Sampling | Worst | Median |
|---|---|---|
| 11 anchor notes | 11.0%% | 1.8%% |
| + 4 notes it requested | 8.4%% | 1.5%% |
| 11 anchors, one bad string | 11.0%% | 2.0%% (culprit dropped) |

Worst case sits within a key or two of the break, where the curve genuinely steps and nothing in
the samples says exactly where. Anchors are now keys 1, 8, 16, 22, 28, 34, 40, 52, 64, 76, 85.

### Three real defects the tests forced out

1. **A global two-line fit cannot follow real curvature** — 22%% out at worst. Individual keys are
   now estimated by **locally weighted regression** over nearby samples on the same side of the
   break, recovering curvature the samples already carry. Median error fell under 2%%.

2. **Committing to an exact break location was worse than admitting ignorance** — 40%% error on keys
   between the guess and the truth. Where the break falls inside an unsampled gap its position is
   genuinely unknown, so the two local estimates are **blended across that gap**.

3. **Leverage defeats single-pass outlier rejection.** A bad reading at the edge of a section tilts
   the line toward itself until its own residual looks tolerable, while making a good neighbour look
   wrong (observed: culprit residual 0.263, innocent neighbour -0.206, threshold 0.3 — so the wrong
   one would have been dropped). Now the worst offender is removed and the fit repeated, up to 3
   times, threshold 0.18 log10 (~50%% in B), far beyond genuine string variation.

### Sensitivity note — why B accuracy matters

A 4:2 octave stretches by roughly 866*(16*B_low - 4*B_high) cents. At B = 3e-4 that is ~3.1 cents.
A 20%% error in B_low alone shifts octave width ~0.8 cents. So the interpolated model wants to be
good to ~10%% to stand alone — which the median comfortably is, but the break neighbourhood is not.
Mitigated in practice by D10 continuous refinement: every note gets measured directly as it is
tuned, so the interpolation only governs notes not yet reached.

### Phase 4 gate NOT yet met

The stated gate is **stable and repeatable across sessions on one real piano**. Everything above is
synthetic. This is the first phase that genuinely requires the owner recordings — synthetic pianos
have tidy curves; real ones have breaks in odd places and strings that misbehave.

**Blocked on:** single-note recordings across the compass from 3-4 real instruments, ideally
including a rough old upright. ~15 min per piano.

---

## 20. Note recorder (2026-09-03)

https://jjcsgox1.github.io/stiffstring/recorder/ — `recorder/index.html`

Built to unblock the phase 4 gate, which needs real instruments rather than synthetic ones.

22 notes: keys 1,4,8,12,16,19,22,25,28,31,34,37,40,46,52,58,64,70,76,82,85,88 — denser in the bass
where the curve bends hardest and the break must be found. Onset-triggered, no buttons while both
hands are at the piano. ~5 min per instrument, ~15 MB per archive.

**32-bit float WAV, never compressed.** Compression discards spectral detail chosen to be inaudible
— exactly what we measure. It could move a partial by more than the entire error budget and leave
no trace. Also: no audio decoder is installed on the owner machine, so m4a would be unreadable.

Design points worth remembering:
- Rolling **pre-roll buffer** (150 ms) so the attack is captured before the onset is noticed
- Noise floor tracker falls fast, rises slowly, so a ringing note cannot drag the trigger up
- 1.5 s settling period before arming, so a door closing is not filed as A0
- Worklet batches to ~50 messages/sec rather than ~400
- One stored (not deflated) ZIP per piano + manifest.json with piano details, condition,
  applied capture constraints, per-note peak levels and quality verdicts

WAV/ZIP writers verified in-browser: headers correct, samples round-trip bit-exactly, central
directory arithmetic consistent.

### Transfer route

**iCloud for Windows is NOT installed** on the owner machine (checked: no `~/iCloudDrive`, no
Apple folder in Program Files\Common Files). Only OneDrive exists on the PC side.

Chosen route: phone saves Safari downloads to iCloud Drive (Settings > Safari > Downloads >
iCloud Drive), owner fetches from icloud.com in a PC browser. One file per piano makes manual
download acceptable. Fallbacks: install iCloud for Windows, or use OneDrive (already on the PC,
needs the phone app).

---

## 21. Phase 5 — tuning curve solver (2026-09-03)

`crates/core/src/curve.rs`. 76 tests, clippy clean.
Report: `cargo run -p stiffstring-core --example tuning_curve --release`

### The formulation correction that made it sound

Writing each requirement as "these two partials should not beat" gives a solver that also
re-derives the TEMPERAMENT: with B=0 it returns a compromise between pure fifths and pure thirds,
not equal temperament. Stating it instead as **preserve the beating the temperament already
intends** makes the just ratio cancel:

    x_j - x_i = 600*log2(1 + B_i*m^2) - 600*log2(1 + B_j*n^2)

Ideal strings then ask for zero stretch on every interval. Everything produced is *stretch* over a
temperament, so historical temperaments later are an additive table (not in v1).

### Three errors found by reading output, not by tests failing

1. **Partial audibility was missing.** Families were weighted by register, but nothing asked whether
   the partials exist there. A 4:2 octave at C7 needs partial 4 near 8.6 kHz. Added
   `partial_usability`, which is also what makes bass use 4:2/6:3 and treble use 2:1 — the same
   interval is a different measurement at each end of the keyboard.
2. **Low cutoff was ~100 Hz**, killing bass 2:1 and forcing 6:3. Partial 6 of a stiff bass string is
   60 cents sharp, demanding absurd octaves. Correct threshold is ~30 Hz: A0 fundamental is truly
   missing, but its 2nd partial at 55 Hz is audible and beats fine.
3. **Beat weighting used the coincident partial frequency**, rewarding partial 8 over partial 2 in
   the same interval. Higher NOTES should count more; higher PARTIALS should not. Two conflated
   concerns; sensitivity now comes from the note fundamental.

Together: curve went from an indefensible -81/+83 cents to a proper Railsback shape.

### Results on a simulated spinet

| | |
|---|---|
| Stretch | -27.2 cents at A0 to +16.9 at C8 |
| Worst kink | 0.19 cents note to note |
| Major thirds | 4.1/s at G#2 climbing steadily to 21.2/s at E5 (D9 satisfied) |
| 4:2 octaves | 0.03-0.40/s through bass and middle |
| Weighted error | 3.22 cents over every interval checked |

### D15 revised — overrides are LOCAL

Feeding overrides into the global solve as heavy requirements reshaped the instrument: a 2-cent
treble nudge moved A0 by 1 cent. Wrong twice — overruling a note is a *local* judgment (bad string,
odd scale point), and **a note already tuned this session must never move** (section 8b rule).
Overrides now apply after the solve, exactly, blended out over ~14 keys.

Weight calibration against the owner aural tunings remains a separate, global mechanism (D8).

### Phase 5 gate NOT met

Gate is "reproduces the owner aural tunings within his own repeatability". That needs his recordings
and his tunings measured. Everything here is synthetic and self-consistent; whether these weights
match HIS ear is entirely unknown and is the real test.

---

## 22. WebAssembly build (2026-09-04)

`crates/wasm/` -> `wasm/stiffstring.wasm` (139 KB, committed; GitHub Pages serves it).
Loader: `wasm/stiffstring.js`. Self-test page: https://jjcsgox1.github.io/stiffstring/engine-check/

### No binding generator, by choice

No wasm-bindgen, no wasm-pack, no npm. Plain C ABI; the whole build is:

    cargo build -p stiffstring-wasm --target wasm32-unknown-unknown --release
    cp target/wasm32-unknown-unknown/release/stiffstring_wasm.wasm wasm/stiffstring.wasm

Stateless interface: JS allocates, writes samples into module memory, calls, reads, frees. Every
function returns a count written or 0, so failures cross the boundary as values, not traps.
Two entry points: `ss_measure_note` and `ss_solve_curve` (fit model + solve in one call).

**Gotcha recorded:** views into module memory MUST be re-taken after every allocation. Growing the
heap replaces the backing buffer and detaches held views — fails silently, not loudly.

**Layouts in `crates/wasm/src/lib.rs` and `wasm/stiffstring.js` must change together.** ABI version
is checked at load; bump `ABI_VERSION` on any layout change.

### Verified on desktop Chrome (WASM matches native exactly)

| | |
|---|---|
| Worst f0 error over 7 notes A0-C7 | 0.001 cents |
| Worst B error | 0.01%% |
| Per note | 53-166 ms |
| Whole-piano model + curve | 3.2 ms |
| Module load | 11-137 ms |

Curve output identical to the native run: A0 -27.20c, A4 -0.0000c, C8 +16.85c, break G2.

### Test-page bug worth remembering

The first run showed 1.27 cents / 8%% error at C6 and "13 partials" in a 5-partial tone. Cause was
the CHECK PAGE, not the engine: its LCG multiplied past the 53 bits a JS number holds exactly, lost
low bits, and produced a periodic sequence containing tones. The engine correctly found real peaks
that the test had accidentally planted. Replaced with xorshift32 (stays in 32 bits). Errors fell to
0.001 cents / 0.01%%.

### Next

Phase 6 proper: live meter as a tracker, note sequencing, measurement pass UI, curve review,
pitch-reference decision, storage, home-screen install. Also: wire the engine into the recorder so
the phone measures while recording, collapsing the phase 4/5 feedback loop.

---

## 23. First real recordings — phase 4 gate NOT met (2026-09-04)

22 notes, iPhone, 48 kHz, all graded good, peaks -6 to -17 dBFS. **Piano: a BABY GRAND ("grand at
home"), and it is NOT the same instrument used for the phase 0a hand captures.**

### CORRECTION — there is no cross-check

I briefly claimed the phase 0a values (A3 3.04e-4, A4 7.33e-4) corroborated the recorded ones
(A3 2.85e-4, A4 ~8.15e-4 interpolated) because they agree within 6-11%%. **That claim is void:
different pianos.** Their agreement is coincidence. As of now there is **no independent evidence**
that the engine measures the instrument rather than itself.

What would provide it, in order of value:
1. **Record the same baby grand again on another day** — this IS the phase 4 gate wording (stable
   and repeatable across sessions) and needs no reference equipment.
2. Record the phase 0a piano with the recorder, making the hand-captured numbers and the full
   pipeline describe one instrument.

### Two faults fixed by the real data

1. **Analysis ran over the whole 3.65 s recording, not the note in it.** Treble notes finish in well
   under a second; they were diluted with silence and phase was measured over empty frames. Four
   top notes returned nothing. Now trimmed to the sounding portion, frame length adapted to what
   remains.
2. **Two notes were confidently wrong** — C1 by -150 cents, D#3 by -205 — the signature of a partial
   series numbered one out. Such a numbering fits its own partials beautifully, places f0 a whole
   tone away, and every quality signal looks excellent. Now the fitted f0 is checked against the
   expected note (after the anchor fit and again at the end) and out-of-tolerance measurements are
   **declined**. A missing reading is obvious; a confident wrong one poisons the keyboard model.

Failures 6 -> 3 notes. Model residual 0.30 -> 0.036 log10.

### Dominant remaining defect: no unison modelling

Confidence 0.02-0.56 on nearly every note (synthetic scores >0.95); almost all flag
`UnstablePartials`. This is exactly **D18**, deliberately deferred: the estimator must model each
partial as **2-3 closely spaced components**, because every real note has 2-3 strings. Built the
single-component version only, so partials genuinely do not hold still and the confidence metric is
correctly reporting an unmodelled phenomenon. Measurements may still be usable (residual says so)
but cannot be claimed as good until this exists.

Also still failing: F#7, A7, C8 — few partials, very fast decay.

### Measured (baby grand, provisional)

Pitch median -6.1 cents from A4=440, i.e. A4 ~ 438.5 Hz.

| | A0 | A1 | A2 | A3 | A4 | A5 | A6 |
|---|---|---|---|---|---|---|---|
| B | 7.9e-4 | 4.1e-4 | 3.0e-4 | 2.9e-4 | 8.2e-4 | 2.2e-3 | 6.1e-3 |

Plausible for a baby grand. Plausibility is not evidence.

---

## 24. Unison / beat model — D18 delivered (2026-09-04)

### First independent corroboration

The engine reported the widest beat on the baby grand at **C1**, and the cleanest at **C4**.
The owner ear agrees on both. **This is the first evidence the engine measures the instrument
rather than itself** — the gap left when the cross-check between two different pianos was retracted.

### It also caught a labelling error that would have misled in the field

**C1 on that piano has ONE string.** So the beat there is a *false beat* — one string vibrating in
two planes at slightly different rates (bridge/bearing asymmetry) — not a unison. Tuning cannot
cure it. Calling it a "unison spread" was confidently wrong.

Nothing in the sound distinguishes the two cases. The engine now **measures and declines to
interpret**: `unison_spread_cents` -> `beat_spread_cents`, `Concern::BeatingUnison` ->
`StringsBeating`, and `piano::Stringing { single_through, double_through }` carries how the piano is
strung so a caller can say which it is. **Defaults (single<=key 8, double<=key 16) are a guess and
want setting per piano** — the recorder should capture this.

### How the beat model works

A partial amplitude is followed across the note and searched for a steady rise and fall. Two
components `d` cents apart put their nth partials `f_n*d/1731` Hz apart, so **every beating partial
independently estimates the same spread** and the median is taken. Costs nothing extra — the payoff
for D18 (measure notes as they sound rather than muting to one string).

Wobble a beat explains no longer counts against confidence. On the real recordings confidence rose
**0.02-0.56 -> 0.10-0.88**, and `UnstablePartials` now marks the few genuinely troubled notes.

### Two mis-calibrations, not bugs

1. Quarter-radian phase wobble as "worthless" was measured from synthetic SINGLE strings; real notes
   have three. Widened to 0.6 and documented as calibrated against real notes.
2. Requiring a fixed share of partials to beat. The lowest partials of a tight unison beat too
   slowly for any window to resolve and can never qualify. Two partials agreeing is the evidence.

### False-positive guards (a detector that finds beats in silence is worse than none)

Scanning hundreds of candidate rates and keeping the best explains some of any noise. A beat must be
coherent (>50%% of the wobble) AND deep (>12%% modulation), and one slower than **1.5 cycles of the
window** is not claimed at all — indistinguishable from a note getting quieter then less so.

### Performance: the per-frame FFT is gone

It computed thousands of bins to read one. Direct demodulation at the frequency of interest is >10x
cheaper — which is what makes dozens of looks per note affordable — and removes scalloping loss,
since it measures the frequency asked for rather than whichever a bin is centred on.

WASM ABI -> 2 (beat spread per note, beat rate per partial). 80 tests, clippy clean.

### Measured beat spreads, baby grand

C1 8.42 (false beat) | E1 3.35 | G#1 1.95 | C2 1.60 | D#2 2.39 | F#2 1.99 | A2 2.77 | C3 1.00 |
F#3 2.06 | A3 2.09 | C4 0.84 | F#4 1.75 | C5 3.80 | F#6 2.27 | C7 3.26  (cents)

### Still open

- 4 notes unmeasurable: D#3, F#7, A7, C8 (few partials, fast decay)
- **Phase 4 gate still untested**: needs the same piano recorded on another day
- Where the string-count transitions actually fall on the baby grand

---

## 25. Phase 4 GATE PASSED + Phase 6 measurement pass (2026-09-04/05)

### Phase 4 gate: PASSED

Two recordings of the same baby grand, a day apart (2026-09-04 and 2026-09-05):

| | |
|---|---|
| Median disagreement, all 88 targets | **0.10 cents** |
| Worst anywhere | **0.52 cents** (A6) |
| Stiffness per note, median | 1.7%% |

Tool: `cargo run -p stiffstring-core --example compare_sessions --release -- <folderA> <folderB>`

First attempt failed at 9.3 cents in the top octave while agreeing to 0.1c elsewhere — which
located the fault exactly. Two fixes:

1. **`fit_f0_given_b`** — treble notes give 2-3 partials, too few for both f0 and B (3 points needed
   for a line). Stiffness varies smoothly, so borrow it from below. Two-pass measurement: measure,
   fit model, retry failures with `b_hint`. C7/F#7/A7 now measure on both days.
2. **Extrapolation backstop was clamping relative to `highest_sampled`** — and the treble is exactly
   where notes fail, so it engaged at whatever note sampling reached. Session B reported IDENTICAL
   stiffness for F#7 and A7 (both held flat). **A model must not change shape because of an accident
   of which notes were measurable.** Widened 12 -> 24 keys; B_MAX still bounds runaway.

**Cost of borrowing, now a test:** a 20%% stiffness error moves pitch 0.1 cents at A3 and ~7.7 cents
at A7 (stiffness stretches partial n by ~B*n^2, and B is 100x larger up there). My own estimate was
wrong by 8x. **A top note pitch is only as good as the notes below it.**

Still unmeasured on both days: **C8**.

### Phase 6: the measurement pass shipped

https://jjcsgox1.github.io/stiffstring/measure/ — `measure/index.html`

First screen that is the product rather than a test of it. Name piano -> play the notes it asks for
-> tuning for all 88 keys. Nothing leaves the phone.

Design points:
- Measuring face built for one hand at arm length: note name is the largest element, capture starts
  on the strike, advances itself. Nothing to press while both hands are busy.
- **Adaptive plan**: anchors -> `suggest_next_key` for up to 5 more -> second pass retrying failures
  with borrowed stiffness.
- **Pitch hint improves as it goes**: after 3 notes, the median offset relocates the search, so a
  piano sitting 40 cents flat is looked for where it actually is.
- Reference pitch offers the piano own pitch beside 440 (D14).
- **String counts captured per piano** — decides false beat vs unison (the C1 finding).
- localStorage + export (storage can be evicted).

### WASM ABI 3

- curve output gains **stiffness per key** (so callers can lend it to the top octave)
- `ss_anchor_keys`, `ss_suggest_next_key` exposed
- each measurement reports **its own weight** — the rule for how much a troubled reading counts now
  lives in one place instead of being restated in JS

Verified live on GitHub Pages: ABI 3, anchors, suggestion (asked for key 25, bracketing the break),
solve, chart, storage, save/load, and the false-beat labelling all working. Mic path untested by me
by nature.

### Remaining

- **Phase 5 gate**: owner tunes a piano by ear, then records those 22 notes -> fit solver weights to
  his ear. The only remaining non-engineering question. Does NOT need the app; the recorder suffices.
- Live meter (tracker, not detector) -> completes v1
- C8 unmeasurable
- Rough old upright still the most valuable recording

---

## 26. Phase 6 COMPLETE — the live meter, and v1 (2026-09-05)

`crates/core/src/meter.rs`, WASM ABI 4, tune panel in `measure/index.html`. 97 tests, clippy clean.
Report: `cargo run -p stiffstring-core --example meter_check --release -- <corpus folder>`

Owner ran the measurement pass on the baby grand for real: 24 notes measured, curve solved,
piano reported at 438.2 Hz as it sits. First use of the app at a piano by its owner.

### Tracker, not detector (D16/D17 realised)

By the time a pin is being set, the note, its stiffness and its target are known, so every
partial's wanted frequency is arithmetic. The only question left is how far ONE component is from
it. Consecutive updates ask the same narrow question of overlapping audio — the previous frequency
is handed back in — so the reading moves only when the string does. **5 ms per update in WASM on
desktop**; the page paces itself at max(70 ms, 2.5x what the last update cost).

### Follow the LOWEST usable partial, not the loudest

The zero of the meter carries `600*log2(1 + B*n^2)` of whatever error the stiffness model has, and
that grows as n^2. With B known to 20%: **0.17 cents at the fundamental, 0.7 at partial 2, 2.7 at
partial 4, 10.3 at partial 8.** So the treble follows partial 1, the bass follows whatever low
partial exists (fundamental absent below ~C2, and below MIN_TRACK_HZ = 90 Hz regardless). Bass
metering is therefore inherently more sensitive to B than treble metering — a real limit, recorded
as a test.

### Three findings from testing, all "confidently wrong" failures

1. **A note 2 semitones out still locks.** Its 5th partial lands inside the band searched for our
   6th. Reported -119 cents at 0.9997 confidence when the truth was +200. Fix: a reading must be
   **corroborated by another partial**.
2. **The octave below shares every partial** — corroboration cannot see it. Checked for directly:
   a strong component at half the target fundamental means the lower note was struck.
3. **Keeping 3 peaks per band made things WORSE.** Tried so a quieter self-agreeing set could be
   found under a louder interloper (a real E1 recording where C1 was still ringing and owned every
   band). But C1 sat a major third below: its 5th lands in our 4th and its 10th in our 8th at the
   SAME offset, so it corroborated itself perfectly and the meter locked onto it — 29 cents sharp
   on a note 31 cents flat. **Corroboration proves some note is sounding coherently, never that it
   is ours.** Reverted to one peak per band, where an interloper's partials scatter and the meter
   says nothing. Saying nothing is right when another string is drowning the one being tuned.

### What the display shows, and why it is not the latest reading

A note with 2-3 strings has no single pitch: the strings beat, and a quarter-second look catches
the composite wherever it is in that beat — swinging hardest at the null, where the phase of the
sum turns over. So the display is the **median** of the last 8 readings (mean would be dragged by
the null excursion), preferring readings above 0.9 confidence but never fewer than 3. `settle()`
lives in the engine, not the page: it is a judgement about audio.

The spread is reported alongside, because how much the readings disagree is what tells the
technician the unison wants setting before the pitch does.

### Graded on REAL recordings, both days

| | day 1 | day 2 |
|---|---|---|
| Meter vs full measurement of the same partial | 0.60c median | 0.47c |
| Cost of turning one partial into a note (the B model) | 0.67c | 0.46c |
| Display steadiness on a held note, median | 0.57c | 0.60c |
| Worst steadiness | 6.1c (C6) | 9.7c (D#2) |
| Per update, native | 8.6 ms | 8.7 ms |

The plan's 0.20-cent target is for a **steady tone**, and is met there (unit test asserts < 0.2 on
synthetic). On a real untuned piano the swing tracks the unison width: the 9.7c worst case is a
note whose strings are 4.2 cents apart. That is the note, not the meter, and the page says so.

**Also measured: notes drift DOWN as they decay** (median -0.33 / -0.08 cents over the first
second) — the nonlinear tension effect. Pitch must be judged at a consistent moment after the
strike, which the technician's ear does anyway.

### The tune screen

Panel 4 of `measure/`. Note name + target Hz, one huge cents number, a needle with the tolerance
band drawn on it, and a line saying what the reading rests on (which partial, beats/sec with false
beat vs unison from the piano's string counts, and a warning when the reading is swinging).
88-key strip to jump anywhere.

- **D23 delivered**: a note completes itself after holding inside tolerance for 1.2 s, and a
  swinging reading does not count as holding. Can be switched to manual.
- **Reference pitch and stretch lock as soon as anything is tuned** — a target a note has already
  been set to must never move (section 8b).
- Tuned keys are saved with the piano.

### v1 IS COMPLETE (phases 0a-6)

Measure the piano -> 88 targets -> meter each note against them. Verified end to end in the
browser against synthetic audio driven through the real page: lock, track, settle, needle,
auto-advance, key strip, manual advance.

---

## 27. Field findings from the first real use (2026-09-06)

Three faults, all found by the owner taking the app to a piano, none of which any amount of
synthetic testing would have produced.

### D22 REVISED — the tuning sequence follows the tuning, not the plate

The sequence worked a semitone outward from C4 in both directions at once. That spreads the
tension change evenly across the plate and is **confusing to follow at the piano**, and confusing
wins. Replaced with the order an aural tuning is actually laid:

    temperament   F3 -> F4     keys 33-45
    treble        F#4 -> C8    keys 46-88
    bass          E3 -> A0     keys 32-1

**Always the next key along.** Marking a note done also no longer skips to the next *untuned* key —
that sends the technician somewhere else on the piano without saying why. A section line above the
note name says which stage is under way and how far through it is. The 88-key strip remains for
deliberate jumps.

Original D22 rationale (even tension change) is not lost so much as outranked: it was a guess about
what would matter, tested against a technician and found not to.

### The measurement pass could skip the note it was asking for

`finishCapture` incremented the note index **unconditionally**, so anything that completed a
capture consumed the slot for the note being asked for. Two ways it fired:

1. **False trigger.** The onset test looks for a 14 dB rise above the tracked floor — a chair, a
   pedal, or the swell of a beating note left ringing all clear it. Three seconds of room got
   captured, measured nothing, and the pass advanced. The technician then played the note **still
   on screen** into the NEXT note's slot. That is worse than a skip: it files a real measurement
   under the wrong key, and every downstream guard then has to catch it.
2. **A note the engine could not read** was recorded as a failure and the pass moved on, which
   reads as being skipped even though it was deliberate.

Fixes: a capture whose **peak never reaches -42 dBFS** was the room, not a strike — it does not
count as an attempt and does not advance (real strikes sit at -6 to -17 dBFS). A note that cannot
be measured **keeps the screen** and asks again, up to 3 tries, before being set aside for the
second pass. The re-arm window now starts when the engine finishes rather than before it.

**Lesson worth keeping: an onset detector is not a strike detector.** Detecting that the level rose
says nothing about whether a note was played, and a UI that advances on that signal alone will
silently mis-file data rather than obviously fail.

### D14 was half-built — the reference pitch was chosen silently

Owner tuned a temperament with the app and measured it **10 cents flat of A440** on another tuner.
The numbers were right. The app had made a decision quietly that it should have made out loud.

| | |
|---|---|
| Reference pitch defaulted to the piano's own | -6.3 cents (his baby grand sits at 438.4) |
| Stretch at F3, relative to equal temperament | -4.1 cents |
| Total a 440 chromatic tuner shows at F3 | **-10.4 cents** |

**Leaving a piano where it sits is right for an instrument too flat to pull up in one visit and
hold. It is wrong for one a few cents down, which is most of them.** Changed: standard pitch is
recommended unless the piano is more than `EASY_PULL_CENTS` = 15 from it, and the reason is printed
either way; the curve panel prints what a 440 tuner will read for F3 and A4; and the tuning face
shows **both** numbers per note — `-4.1c stretch · -10.4c vs A440`.

D14 said "measure, recommend with reasoning, accept being overruled". Measure and overrule were
built; **recommend with reasoning** was not, and a silent default is not a recommendation.

Generalisation worth keeping: **every quantity this app reports is relative to something, and the
something has to be on screen.** A number without its frame reads as an error to anyone holding a
second instrument.

---

## 28. First phase 5 evidence — the solver against the owner's ear (2026-09-06)

Owner tuned the home baby grand **by ear** and recorded the 22-note survey.
Corpus: `corpus/home-grand-tuned-2026-09-06`. Report: `--example intervals`.

### What he tuned

The dividing line falls exactly at E3. Comparing the three sessions of this piano in cents from
A440 equal temperament, everything from F#3 up moved +4 to +6 cents; everything from D#3 down is
within ~1.5 cents of where it was two days earlier. F#5 and F#6 also did not move.

So: **temperament and treble tuned, bass section not started** — the sequence the app now lays out,
stopped two thirds of the way through.

E1 appears to move +15.8 cents. It did not: the 09-04 and 09-05 E1 recordings had C1 still ringing
over them (the same file the meter's corroboration guard refused to lock onto, section 26). The old
reading was contaminated; the new one is right.

### The result

Against the solver's own targets at A4 = 440, restricted to the notes that actually moved:

| F#3 | A3 | C4 | F#4 | C5 | C6 |
|---|---|---|---|---|---|
| -1.6c | -0.5c | +0.3c | +1.1c | +0.1c | -0.8c |

**Median 0.65 cents, worst 1.6 cents.**

Real disagreement, not noise: the engine's own repeatability on this piano is 0.10 cents median
(section 25), more than six times finer.

Octave beat rates, his beside the solver's, for the octaves where both notes were tuned:

| | his 2:1 | solver | his 4:2 | solver |
|---|---|---|---|---|
| F#3-F#4 | +0.84 | +0.26 | +1.38 | +0.21 |
| C4-C5 | +0.35 | +0.39 | +0.10 | +0.06 |
| C5-C6 | +0.65 | +1.22 | -3.31 | -1.79 |

C4-C5 is a bullseye. F#3-F#4 he laid wider than the solver wants, C5-C6 narrower. **No consistent
direction across three octaves, so there is nothing here to fit weights to yet.**

### Why this is evidence and not the gate

Six notes, one register, one piano. The family weights have nine free parameters; fitting them to
six points in the middle of the keyboard would be overfitting a curve to its easiest region. The
gate also requires his own repeatability, which needs two aural tunings of one piano weeks apart
and has not started.

What it does establish, for the first time: **the solver is already within about a cent and a half
of a working technician's ear through the tenor and lower treble.** Every previous number in this
project graded the engine against itself or against synthesis.

### The 22-note survey is the wrong instrument for this question

It samples a smooth stiffness curve. A tuning is not smooth — it is a decision at each note. Six of
the twenty-two landed in the tuned section. **The recorder now offers all 88 keys**, chosen before
recording and recorded in the manifest (~60 MB; partial saves already worked and now say so).
