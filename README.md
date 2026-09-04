# Stiffstring

A piano tuning **measurement instrument** — measures an individual acoustic piano's string
inharmonicity and computes tuning targets appropriate to that specific instrument.

Working codename. Not a chromatic tuner with a piano interface.

## Status

**Phase 0a — iOS Safari audio-integrity spike.** Everything else is gated on this: does iPhone
Safari deliver raw, unprocessed microphone audio? If it silently applies automatic gain control or
noise suppression, the measurement engine cannot be trusted on that platform.

## Layout

    spikes/0a-audio-integrity/    Safari audio integrity test (no build step, no dependencies)

## Plan

The full plan, with the decisions log and the reasoning behind each choice, lives at
`~/.claude/plans/i-wanna-make-a-swirling-fairy.md`.

## Hosting

Served by GitHub Pages from the repository root. The microphone requires HTTPS, so the
pages cannot be tested by opening the files directly from disk.

## Building

Tests and the accuracy reports:

    cargo test --all
    cargo run -p stiffstring-core --example accuracy --release
    cargo run -p stiffstring-core --example tuning_curve --release

The engine for the web app. There is no JavaScript build step and no npm; this
produces a `.wasm` that plain script tags load:

    cargo build -p stiffstring-wasm --target wasm32-unknown-unknown --release
    cp target/wasm32-unknown-unknown/release/stiffstring_wasm.wasm wasm/stiffstring.wasm

`wasm/stiffstring.wasm` is committed because GitHub Pages serves it. Rebuild and
copy it in the same commit as any change to `crates/wasm/src/lib.rs`, and keep
the data layouts there in step with `wasm/stiffstring.js`.
