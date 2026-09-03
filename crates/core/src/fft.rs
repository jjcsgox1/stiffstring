//! Radix-2 FFT and analysis windows.
//!
//! Written rather than pulled in as a dependency: the transform is small,
//! well understood, and keeping the crate dependency-free keeps the eventual
//! WebAssembly payload small and the build trivial.
//!
//! Note what the FFT is *for* here. It locates partials; it does not measure
//! them. At 48 kHz a 32768-point transform still has 1.46 Hz bins, roughly sixty
//! times coarser than the precision this project needs. Pinning a partial down
//! is [`crate::estimate`]'s job, and it works by watching phase rather than by
//! looking harder at magnitude.

use std::f64::consts::TAU;

/// A complex number. Deliberately minimal — only what the transform needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };

    #[inline]
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// Magnitude.
    #[inline]
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Phase in radians, in (-pi, pi].
    #[inline]
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    #[inline]
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// In-place radix-2 decimation-in-time FFT. `buf.len()` must be a power of two.
///
/// Twiddle factors are computed directly rather than by recurrence: the
/// recurrence accumulates error across the long transforms this project uses,
/// and we are chasing hundredths of a cent.
pub fn fft(buf: &mut [Complex]) {
    let n = buf.len();
    assert!(
        n.is_power_of_two(),
        "FFT length must be a power of two, got {n}"
    );
    if n < 2 {
        return;
    }

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let mut base = 0;
        while base < n {
            for k in 0..half {
                let ang = -TAU * k as f64 / len as f64;
                let (s, c) = ang.sin_cos();
                let lo = buf[base + k];
                let hi = buf[base + k + half];
                let v = Complex::new(hi.re * c - hi.im * s, hi.re * s + hi.im * c);
                buf[base + k] = Complex::new(lo.re + v.re, lo.im + v.im);
                buf[base + k + half] = Complex::new(lo.re - v.re, lo.im - v.im);
            }
            base += len;
        }
        len <<= 1;
    }
}

/// Inverse FFT, by conjugation. Present mainly so tests can prove the forward
/// transform round-trips.
pub fn ifft(buf: &mut [Complex]) {
    for c in buf.iter_mut() {
        c.im = -c.im;
    }
    fft(buf);
    let scale = 1.0 / buf.len() as f64;
    for c in buf.iter_mut() {
        c.re *= scale;
        c.im = -c.im * scale;
    }
}

/// Analysis windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// No window. Maximum frequency resolution, disastrous leakage.
    Rectangular,
    /// Reasonable general-purpose compromise.
    Hann,
    /// Four-term Blackman-Harris. Sidelobes near -92 dB.
    ///
    /// The default for locating partials: a piano note puts strong low partials
    /// beside weak high ones, and with a lesser window the skirts of the strong
    /// ones bury the weak ones entirely.
    BlackmanHarris,
}

/// Window coefficients of length `n`.
pub fn window(kind: Window, n: usize) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let denom = n as f64;
    (0..n)
        .map(|i| {
            let x = TAU * i as f64 / denom;
            match kind {
                Window::Rectangular => 1.0,
                Window::Hann => 0.5 - 0.5 * x.cos(),
                Window::BlackmanHarris => {
                    0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos()
                        - 0.01168 * (3.0 * x).cos()
                }
            }
        })
        .collect()
}

/// Mean of the window, the factor by which it scales a sinusoid's amplitude.
pub fn coherent_gain(w: &[f64]) -> f64 {
    if w.is_empty() {
        return 0.0;
    }
    w.iter().sum::<f64>() / w.len() as f64
}

/// Window `samples` and transform, returning the full complex spectrum.
///
/// `samples.len()` must be a power of two.
pub fn spectrum(samples: &[f32], w: &[f64]) -> Vec<Complex> {
    debug_assert_eq!(samples.len(), w.len(), "window length must match the frame");
    let mut buf: Vec<Complex> = samples
        .iter()
        .zip(w)
        .map(|(&s, &g)| Complex::new(f64::from(s) * g, 0.0))
        .collect();
    fft(&mut buf);
    buf
}

/// Magnitudes of the first half of a spectrum, scaled so a full-scale sinusoid
/// reads 1.0 regardless of window or transform length.
pub fn amplitudes(spec: &[Complex], w: &[f64]) -> Vec<f64> {
    let n = spec.len();
    let scale = 2.0 / (n as f64 * coherent_gain(w));
    spec[..n / 2].iter().map(|c| c.abs() * scale).collect()
}

/// Width of one FFT bin in Hz.
#[inline]
pub fn bin_hz(sample_rate: f64, fft_len: usize) -> f64 {
    sample_rate / fft_len as f64
}

/// Fit a parabola through a peak bin and its two neighbours, in the log
/// magnitude domain.
///
/// Returns the peak's offset from the centre bin, in bins and within -1..1, and
/// its interpolated amplitude.
///
/// Both outputs matter. The offset locates a partial to a fraction of a bin. The
/// amplitude corrects *scalloping loss*: a single bin under-reads a tone sitting
/// between bins by up to 0.8 dB with Blackman-Harris, which would otherwise make
/// a partial's strength depend on where it happened to land.
pub fn parabolic_peak(below: f64, at: f64, above: f64) -> (f64, f64) {
    const TINY: f64 = 1e-300;
    let y0 = below.max(TINY).ln();
    let y1 = at.max(TINY).ln();
    let y2 = above.max(TINY).ln();

    let denom = y0 - 2.0 * y1 + y2;
    let offset = if denom.abs() > 1e-12 {
        let d = 0.5 * (y0 - y2) / denom;
        // A well-formed peak puts the vertex between its neighbours; anything
        // further means this is not really a peak, so do not extrapolate.
        if d.abs() <= 1.0 {
            d
        } else {
            0.0
        }
    } else {
        0.0
    };

    (offset, (y1 - 0.25 * (y0 - y2) * offset).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(got: f64, want: f64, tol: f64, what: &str) {
        assert!(
            (got - want).abs() <= tol,
            "{what}: got {got}, want {want} (tolerance {tol})"
        );
    }

    #[test]
    fn impulse_transforms_to_a_flat_spectrum() {
        let mut buf = vec![Complex::ZERO; 8];
        buf[0] = Complex::new(1.0, 0.0);
        fft(&mut buf);
        for (i, c) in buf.iter().enumerate() {
            assert_close(c.re, 1.0, 1e-12, &format!("bin {i} real"));
            assert_close(c.im, 0.0, 1e-12, &format!("bin {i} imag"));
        }
    }

    #[test]
    fn a_constant_transforms_to_dc_only() {
        let mut buf = vec![Complex::new(1.0, 0.0); 8];
        fft(&mut buf);
        assert_close(buf[0].re, 8.0, 1e-12, "dc");
        for (i, c) in buf.iter().enumerate().skip(1) {
            assert_close(c.abs(), 0.0, 1e-12, &format!("bin {i}"));
        }
    }

    #[test]
    fn round_trips_through_the_inverse() {
        let n = 256;
        let original: Vec<Complex> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64;
                Complex::new((TAU * 7.0 * t).sin() + 0.3 * (TAU * 31.0 * t).cos(), 0.0)
            })
            .collect();
        let mut buf = original.clone();
        fft(&mut buf);
        ifft(&mut buf);
        for (i, (a, b)) in buf.iter().zip(&original).enumerate() {
            assert_close(a.re, b.re, 1e-10, &format!("sample {i} real"));
            assert_close(a.im, b.im, 1e-10, &format!("sample {i} imag"));
        }
    }

    #[test]
    fn energy_is_conserved() {
        // Parseval: sum |x|^2 == (1/N) sum |X|^2
        let n = 512;
        let x: Vec<Complex> = (0..n)
            .map(|i| Complex::new(((i * 37) % 11) as f64 - 5.0, ((i * 13) % 7) as f64 - 3.0))
            .collect();
        let time_energy: f64 = x.iter().map(|c| c.norm_sqr()).sum();
        let mut buf = x;
        fft(&mut buf);
        let freq_energy: f64 = buf.iter().map(|c| c.norm_sqr()).sum::<f64>() / n as f64;
        assert_close(freq_energy, time_energy, time_energy * 1e-12, "energy");
    }

    #[test]
    fn a_bin_centred_sinusoid_lands_in_exactly_one_bin() {
        let n = 1024;
        let k = 64; // exactly k cycles across the frame
        let samples: Vec<f32> = (0..n)
            .map(|i| (TAU * k as f64 * i as f64 / n as f64).sin() as f32)
            .collect();
        let w = window(Window::Rectangular, n);
        let amps = amplitudes(&spectrum(&samples, &w), &w);

        // Tolerance is set by f32 samples, not by the transform: single
        // precision carries about seven digits, so ~1e-7 relative error is the
        // floor no amount of care in the FFT can beat.
        assert_close(amps[k], 1.0, 1e-6, "peak amplitude");
        for (i, &a) in amps.iter().enumerate() {
            if i != k {
                assert!(a < 1e-6, "energy leaked into bin {i}: {a}");
            }
        }
    }

    #[test]
    fn amplitude_scaling_is_window_independent() {
        // A half-scale sinusoid must read 0.5 whatever window we analyse it with.
        let n = 4096;
        let k = 100.0;
        let samples: Vec<f32> = (0..n)
            .map(|i| (0.5 * (TAU * k * i as f64 / n as f64).sin()) as f32)
            .collect();
        for kind in [Window::Rectangular, Window::Hann, Window::BlackmanHarris] {
            let w = window(kind, n);
            let amps = amplitudes(&spectrum(&samples, &w), &w);
            let peak = amps.iter().cloned().fold(0.0, f64::max);
            assert_close(peak, 0.5, 0.01, &format!("{kind:?} peak amplitude"));
        }
    }

    #[test]
    fn blackman_harris_suppresses_nearby_leakage_far_better_than_hann() {
        // The case that matters: a weak high partial sitting a short distance
        // from a strong low one, which is every piano note. Worst case for
        // leakage is a tone sitting halfway between two bins.
        //
        // Measured just outside Blackman-Harris's main lobe, which is where the
        // difference decides whether a weak partial is visible at all. Far from
        // the peak the gap narrows — Blackman-Harris bottoms out near its -92 dB
        // floor while Hann is still rolling off steeply — so a distant probe
        // would understate the advantage that actually matters here.
        let n = 4096;
        let k = 200.5;
        let samples: Vec<f32> = (0..n)
            .map(|i| (TAU * k * i as f64 / n as f64).sin() as f32)
            .collect();

        let leakage_at = |kind: Window, bin: usize| {
            let w = window(kind, n);
            amplitudes(&spectrum(&samples, &w), &w)[bin]
        };

        // ~7 bins out: past the 4-bin half-width of Blackman-Harris's main lobe.
        let hann = leakage_at(Window::Hann, 208);
        let bh = leakage_at(Window::BlackmanHarris, 208);
        assert!(
            bh < hann * 0.2,
            "Blackman-Harris should leak far less close in: {bh:.3e} vs Hann {hann:.3e}"
        );
    }

    #[test]
    fn interpolation_corrects_for_a_peak_between_bins() {
        // A tone parked between two bins under-reads badly if you just take the
        // tallest bin. Worst case is dead centre between them.
        let n = 4096;
        for offset in [0.0, 0.25, 0.5] {
            let k = 200.0 + offset;
            let samples: Vec<f32> = (0..n)
                .map(|i| (0.5 * (TAU * k * i as f64 / n as f64).sin()) as f32)
                .collect();
            let w = window(Window::BlackmanHarris, n);
            let amps = amplitudes(&spectrum(&samples, &w), &w);

            let peak_bin = (0..amps.len())
                .max_by(|&a, &b| amps[a].total_cmp(&amps[b]))
                .unwrap();
            let (frac, interpolated) =
                parabolic_peak(amps[peak_bin - 1], amps[peak_bin], amps[peak_bin + 1]);

            assert_close(
                peak_bin as f64 + frac,
                k,
                0.02,
                &format!("interpolated bin at offset {offset}"),
            );
            assert_close(
                interpolated,
                0.5,
                0.005,
                &format!("interpolated amplitude at offset {offset}"),
            );
        }
    }

    #[test]
    fn bin_width_is_the_resolution_we_expect() {
        // The number that motivates the whole phase-based approach: at 48 kHz a
        // 32768-point transform still resolves only 1.46 Hz, about 5.8 cents at
        // A4 — roughly sixty times coarser than we need.
        let bin = bin_hz(48_000.0, 32_768);
        assert_close(bin, 1.4648, 1e-4, "bin width");
        let cents = 1200.0 * ((440.0 + bin) / 440.0f64).log2();
        assert!(cents > 5.0, "expected bins coarser than 5 cents, got {cents}");
    }
}
