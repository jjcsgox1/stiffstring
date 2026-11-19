# Stiffstring

A piano tuning **measurement instrument** — it measures an individual acoustic piano's string
inharmonicity and computes tuning targets appropriate to that specific instrument, then meters
each note against them while the pins are set.

Working codename. Not a chromatic tuner with a piano interface.

## Status

**v1 is built**: measure a piano → 88 targets → live meter. It runs entirely on the phone, with
no account, no network and no recording leaving the device.

| Phase | | |
|---|---|---|
| 0a | iOS Safari delivers raw microphone audio | passed |
| 0b–3 | Synthesis, precision estimator, per-note inharmonicity | passed |
| 4 | Inharmonicity across 88 keys | passed on a real piano — two sessions agree to 0.10 cents median |
| 5 | Tuning curve solver | numerically sound; **against the owner's ear, evidence only** — 0.65 cents median over six notes |
| 6 | Live meter and workflow | built and used at a piano; a complete tuning done with it is still outstanding |
| 7–10 | Unisons, pitch raise, customer records, polish | not started, and deliberately post-v1 |

The honest summary of what remains is in [`PLAN.md`](PLAN.md) §28.

## Pages

All served by GitHub Pages from the repository root. The microphone requires HTTPS, so none of
these can be tested by opening the files from disk.

| | |
|---|---|
| [`measure/`](https://jjcsgox1.github.io/stiffstring/measure/) | The app. Measure a piano, solve its tuning, tune to it. |
| [`recorder/`](https://jjcsgox1.github.io/stiffstring/recorder/) | Captures single notes as uncompressed audio, for the regression corpus. |
| [`engine-check/`](https://jjcsgox1.github.io/stiffstring/engine-check/) | Runs the engine on this device against tones whose answer is known. |
| [`spikes/0a-audio-integrity/`](https://jjcsgox1.github.io/stiffstring/spikes/0a-audio-integrity/) | The phase 0a microphone test. |

## Layout

    crates/core/     the measurement engine — DSP, inharmonicity, curve solver, meter
    crates/wasm/     a plain C ABI over it, no wasm-bindgen and no npm
    wasm/            the compiled module and its loader, committed because Pages serves them
    corpus/          real recordings, with their analyses — the regression set
    measure/ recorder/ engine-check/ spikes/    the pages above

## Where to start

[`HANDOFF.md`](HANDOFF.md) — the ten-minute orientation: state of play, what is proven, what is
not, the faults that will bite you, and the decisions that were tested and should not be
re-litigated.

## Plan and decisions

[`PLAN.md`](PLAN.md) — the whole reasoning: what was decided, what each phase produced, and the
places where testing overturned a decision. It is the document to read before changing anything in
`crates/core`, because most of the constants in there are calibrated against findings recorded in
it rather than chosen.

## Building

Tests, and the re-runnable accuracy reports:

    cargo test --all
    cargo run -p stiffstring-core --example accuracy --release
    cargo run -p stiffstring-core --example tuning_curve --release

Against the real recordings:

    cargo run -p stiffstring-core --example measure_corpus --release -- corpus/<folder>
    cargo run -p stiffstring-core --example meter_check    --release -- corpus/<folder>
    cargo run -p stiffstring-core --example intervals      --release -- corpus/<folder> [a4Hz]
    cargo run -p stiffstring-core --example compare_sessions --release -- corpus/<a> corpus/<b>

The engine for the web app. There is no JavaScript build step and no npm; this produces a `.wasm`
that plain script tags load:

    cargo build -p stiffstring-wasm --target wasm32-unknown-unknown --release
    cp target/wasm32-unknown-unknown/release/stiffstring_wasm.wasm wasm/stiffstring.wasm

`wasm/stiffstring.wasm` is committed because GitHub Pages serves it. Rebuild and copy it in the
same commit as any change to `crates/wasm/src/lib.rs`, and keep the data layouts there in step
with `wasm/stiffstring.js` — the ABI version is checked at load, so a mismatch fails loudly.
