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
