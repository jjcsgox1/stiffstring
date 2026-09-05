//! Stiffstring measurement engine.
//!
//! The engine is deliberately platform-free: it compiles unchanged to a native
//! binary for the test harness and to WebAssembly for the phone.
//!
//! - [`synth`] generates piano tones whose `f0` and `B` we choose, so everything
//!   else can be graded against a known answer.
//! - [`fft`] locates partials.
//! - [`estimate`] measures them, to a precision the FFT cannot reach, by
//!   watching phase rather than magnitude.

pub mod curve;
pub mod estimate;
pub mod fft;
pub mod inharmonicity;
pub mod piano;
pub mod synth;
pub mod wav;
