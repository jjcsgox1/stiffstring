//! Decoding WAV bytes to samples.
//!
//! Parsing only — nothing here touches a filesystem, so it compiles to
//! WebAssembly along with the rest of the engine and the phone can read a file
//! someone hands it.
//!
//! Handles what the note recorder writes (32-bit float mono) and what other
//! tools commonly produce (16-bit integer), which is the whole of what this
//! project needs to read.

/// Samples and their sample rate.
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: f64,
}

/// Decode a RIFF/WAVE file.
///
/// Multi-channel input keeps only the first channel rather than mixing: summing
/// two microphones can partially cancel a partial, which would quietly corrupt a
/// measurement rather than obviously break it.
pub fn decode(bytes: &[u8]) -> Result<Audio, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);

    let (mut format, mut channels, mut sample_rate, mut bits) = (0u16, 0u16, 0f64, 0u16);
    let mut samples: Vec<f32> = Vec::new();
    let mut pos = 12;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        if body + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = u16_at(body);
            channels = u16_at(body + 2);
            sample_rate = f64::from(u32_at(body + 4));
            bits = u16_at(body + 14);
        } else if id == b"data" {
            match (format, bits) {
                (3, 32) => {
                    samples = (0..size / 4)
                        .map(|i| {
                            let o = body + i * 4;
                            f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
                        })
                        .collect();
                }
                (1, 16) => {
                    samples = (0..size / 2)
                        .map(|i| {
                            let o = body + i * 2;
                            f32::from(i16::from_le_bytes([bytes[o], bytes[o + 1]])) / 32768.0
                        })
                        .collect();
                }
                _ => return Err(format!("unsupported format {format}, {bits} bits")),
            }
        }
        // Chunks are padded to an even length.
        pos = body + size + (size & 1);
    }

    if samples.is_empty() {
        return Err("no audio data".into());
    }
    if sample_rate <= 0.0 {
        return Err("no sample rate".into());
    }
    if channels > 1 {
        samples = samples.iter().step_by(channels as usize).copied().collect();
    }
    Ok(Audio {
        samples,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 32-bit float mono WAV, the way the recorder does.
    fn float_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
        let bytes = samples.len() * 4;
        let mut out = Vec::with_capacity(44 + bytes);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + bytes) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(bytes as u32).to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn round_trips_the_recorder_own_format() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 100.0).sin() * 0.5).collect();
        let audio = decode(&float_wav(&samples, 48_000)).expect("decode failed");
        assert_eq!(audio.sample_rate, 48_000.0);
        assert_eq!(audio.samples, samples, "samples did not survive intact");
    }

    #[test]
    fn reads_sixteen_bit_files_too() {
        let mut out = float_wav(&[], 44_100);
        // Rewrite the header as 16-bit integer with two samples.
        out[20..22].copy_from_slice(&1u16.to_le_bytes());
        out[34..36].copy_from_slice(&16u16.to_le_bytes());
        out[40..44].copy_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&16384i16.to_le_bytes());
        out.extend_from_slice(&(-16384i16).to_le_bytes());

        let audio = decode(&out).expect("decode failed");
        assert_eq!(audio.sample_rate, 44_100.0);
        assert!((audio.samples[0] - 0.5).abs() < 1e-6);
        assert!((audio.samples[1] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(decode(b"nonsense").is_err());
        assert!(decode(&[]).is_err());
        // Valid header, no data chunk.
        let mut headerless = float_wav(&[0.1, 0.2], 48_000);
        headerless.truncate(36);
        assert!(decode(&headerless).is_err());
    }
}
