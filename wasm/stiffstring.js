/*
 * Loader for the Stiffstring measurement engine.
 *
 * The engine is Rust compiled to WebAssembly with a plain C interface — no
 * wasm-bindgen, no npm, no build step beyond cargo. This file is the entire
 * JavaScript side of that boundary, and its layouts must match the ones
 * documented in crates/wasm/src/lib.rs. Change one, change both.
 *
 * Usage:
 *   const engine = await Stiffstring.load("../wasm/stiffstring.wasm");
 *   const note = engine.measureNote(samples, 48000, 220.0);
 */
(function (global) {
  "use strict";

  const EXPECTED_ABI = 3;

  const NOTE_HEADER = 7;
  const PARTIAL_STRIDE = 7;
  const KEYS = 88;
  const CURVE_OUT_LEN = 2 + 3 * KEYS;

  const CONCERNS = [
    [1, "fundamental missing"],
    [2, "few partials"],
    [4, "poor fit"],
    [8, "unstable partials"],
    [16, "partials rejected"],
    [32, "strings beating"],
  ];

  function decodeConcerns(bits) {
    return CONCERNS.filter(([bit]) => (bits & bit) !== 0).map(([, name]) => name);
  }

  class Engine {
    constructor(instance) {
      this.exports = instance.exports;
      this.abiVersion = this.exports.ss_abi_version();
      if (this.abiVersion !== EXPECTED_ABI) {
        throw new Error(
          "engine ABI " + this.abiVersion + " but this loader expects " + EXPECTED_ABI
        );
      }
    }

    /*
     * Views must be taken fresh after every allocation. Growing the module's
     * memory replaces its backing buffer and detaches any view held across the
     * call, which fails silently as zeroed or throwing reads.
     */
    _f32() {
      return new Float32Array(this.exports.memory.buffer);
    }
    _f64() {
      return new Float64Array(this.exports.memory.buffer);
    }

    _alloc(bytes) {
      const ptr = this.exports.ss_alloc(bytes);
      if (!ptr) throw new Error("engine is out of memory (" + bytes + " bytes)");
      return ptr;
    }

    _free(ptr, bytes) {
      if (ptr) this.exports.ss_free(ptr, bytes);
    }

    /** Equal-tempered frequency of a key, 1 = A0 and 88 = C8. */
    keyNominalHz(key, a4Hz) {
      return this.exports.ss_key_nominal_hz(key, a4Hz === undefined ? 440 : a4Hz);
    }

    /** The notes worth measuring first, spread across the compass. */
    anchorKeys() {
      const count = 48;
      const bytes = count * 8;
      const ptr = this._alloc(bytes);
      try {
        const written = this.exports.ss_anchor_keys(ptr, count);
        return Array.from(this._f64().subarray(ptr / 8, ptr / 8 + written));
      } finally {
        this._free(ptr, bytes);
      }
    }

    /**
     * The next note most worth measuring, or null when more would not help.
     *
     * `notes` is the same shape solveCurve takes.
     */
    suggestNextKey(notes) {
      const packed = new Float64Array(notes.length * 4);
      notes.forEach((n, i) => {
        packed[i * 4] = n.key;
        packed[i * 4 + 1] = n.f0 || 0;
        packed[i * 4 + 2] = n.b;
        packed[i * 4 + 3] = n.weight === undefined ? 1 : n.weight;
      });
      const bytes = packed.byteLength;
      if (!bytes) return null;
      const ptr = this._alloc(bytes);
      try {
        this._f64().set(packed, ptr / 8);
        return this.exports.ss_suggest_next_key(ptr, notes.length) || null;
      } finally {
        this._free(ptr, bytes);
      }
    }

    /**
     * Measure one struck note.
     *
     * `samples` is a Float32Array of mono audio. `f0Hint` is where the
     * fundamental is expected — the key being worked on. It may be well over a
     * semitone out; the partials themselves settle the answer.
     *
     * Returns null when the note could not be measured, which is a real answer
     * rather than an error: too quiet, too short, or nothing there to measure.
     */
    measureNote(samples, sampleRate, f0Hint) {
      const inBytes = samples.length * 4;
      // Enough room for the header plus a generous number of partials.
      const outCount = NOTE_HEADER + 32 * PARTIAL_STRIDE;
      const outBytes = outCount * 8;

      const inPtr = this._alloc(inBytes);
      let outPtr = 0;
      try {
        outPtr = this._alloc(outBytes);
        this._f32().set(samples, inPtr / 4);

        const written = this.exports.ss_measure_note(
          inPtr,
          samples.length,
          sampleRate,
          f0Hint,
          outPtr,
          outCount
        );
        if (written === 0) return null;

        const out = this._f64();
        const base = outPtr / 8;
        const count = out[base + 4];
        const partials = [];
        for (let i = 0; i < count; i++) {
          const p = base + NOTE_HEADER + i * PARTIAL_STRIDE;
          partials.push({
            n: out[p],
            hz: out[p + 1],
            amplitude: out[p + 2],
            confidence: out[p + 3],
            residualCents: out[p + 4],
            used: out[p + 5] === 1,
            // Zero means not beating: no string beats at no hertz.
            beatHz: out[p + 6] || null,
          });
        }
        return {
          f0: out[base],
          b: out[base + 1],
          rmsCents: out[base + 2],
          beatSpreadCents: out[base + 5] || null,
          // How much this measurement should count toward a keyboard model.
          weight: out[base + 6],
          concerns: decodeConcerns(out[base + 3]),
          partials,
        };
      } finally {
        this._free(inPtr, inBytes);
        this._free(outPtr, outBytes);
      }
    }

    /**
     * Fit inharmonicity across the keyboard from measured notes, then solve for
     * the 88 targets.
     *
     * `notes` is an array of { key, f0, b, weight }. Options: a4Hz, stretch,
     * smoothness, and overrides as an array of { key, cents }.
     *
     * Returns null when there is too little to go on — fewer than four usable
     * notes will not determine a model.
     */
    solveCurve(notes, options) {
      const opts = options || {};
      const packed = new Float64Array(notes.length * 4);
      notes.forEach((n, i) => {
        packed[i * 4] = n.key;
        packed[i * 4 + 1] = n.f0 || 0;
        packed[i * 4 + 2] = n.b;
        packed[i * 4 + 3] = n.weight === undefined ? 1 : n.weight;
      });

      const overrides = opts.overrides || [];
      const packedOverrides = new Float64Array(overrides.length * 2);
      overrides.forEach((o, i) => {
        packedOverrides[i * 2] = o.key;
        packedOverrides[i * 2 + 1] = o.cents;
      });

      const inBytes = packed.byteLength;
      const ovBytes = packedOverrides.byteLength;
      const outBytes = CURVE_OUT_LEN * 8;

      const inPtr = this._alloc(inBytes);
      let ovPtr = 0;
      let outPtr = 0;
      try {
        if (ovBytes > 0) ovPtr = this._alloc(ovBytes);
        outPtr = this._alloc(outBytes);

        this._f64().set(packed, inPtr / 8);
        if (ovPtr) this._f64().set(packedOverrides, ovPtr / 8);

        const written = this.exports.ss_solve_curve(
          inPtr,
          notes.length,
          opts.a4Hz === undefined ? 440 : opts.a4Hz,
          opts.stretch === undefined ? 1 : opts.stretch,
          opts.smoothness === undefined ? 12 : opts.smoothness,
          ovPtr,
          overrides.length,
          outPtr,
          CURVE_OUT_LEN
        );
        if (written === 0) return null;

        const out = this._f64();
        const base = outPtr / 8;
        return {
          breakKey: out[base] || null,
          rmsLog10: out[base + 1],
          cents: Array.from(out.subarray(base + 2, base + 2 + KEYS)),
          hz: Array.from(out.subarray(base + 2 + KEYS, base + 2 + 2 * KEYS)),
          // Stiffness per key. The top octave gives too few partials to
          // determine its own and borrows from here.
          b: Array.from(out.subarray(base + 2 + 2 * KEYS, base + 2 + 3 * KEYS)),
        };
      } finally {
        this._free(inPtr, inBytes);
        this._free(ovPtr, ovBytes);
        this._free(outPtr, outBytes);
      }
    }
  }

  async function load(url) {
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error("could not fetch " + url + ": HTTP " + response.status);
    }
    // Not instantiateStreaming: it insists on an application/wasm content type,
    // which local files and some hosts do not provide, and the failure looks
    // like a corrupt module rather than a served-wrong one.
    const bytes = await response.arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    return new Engine(instance);
  }

  global.Stiffstring = { load, KEYS, EXPECTED_ABI };
})(typeof window !== "undefined" ? window : globalThis);
