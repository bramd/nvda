use nvda_core::{AMPLITUDE, SAMPLE_RATE};
use std::f64::consts::PI;

/// Generate a sine-wave beep as interleaved stereo 16-bit little-endian PCM.
///
/// Matches the C++ `generateBeep()` from `nvdaHelper/local/beeps.cpp` exactly.
///
/// # Arguments
/// * `hz` - Frequency in hertz
/// * `length_ms` - Duration in milliseconds
/// * `left` - Left channel volume (0-100)
/// * `right` - Right channel volume (0-100)
///
/// # Returns
/// A `Vec<u8>` containing interleaved stereo 16-bit little-endian PCM samples.
pub fn generate_beep(hz: f32, length_ms: u32, left: u32, right: u32) -> Vec<u8> {
    let samples_per_cycle = (SAMPLE_RATE as f32 / hz) as u32;
    let mut total_samples =
        ((length_ms as f64 / 1000.0) / (1.0 / SAMPLE_RATE as f64)) as u32;
    total_samples += samples_per_cycle - (total_samples % samples_per_cycle);

    let lpan = (left as f64 / 100.0) * AMPLITUDE;
    let rpan = (right as f64 / 100.0) * AMPLITUDE;
    let sin_freq = (2.0 * PI) / (SAMPLE_RATE as f64 / hz as f64);

    // Each sample frame is 4 bytes: 2 bytes left i16 + 2 bytes right i16
    let mut buf = Vec::with_capacity(total_samples as usize * 4);

    for sample_num in 0..total_samples {
        let sample = (((sample_num % SAMPLE_RATE) as f64 * sin_freq).sin() * 2.0).clamp(-1.0, 1.0);
        let left_sample = (sample * lpan) as i16;
        let right_sample = (sample * rpan) as i16;
        buf.extend_from_slice(&left_sample.to_le_bytes());
        buf.extend_from_slice(&right_sample.to_le_bytes());
    }

    buf
}

/// Return the buffer size in bytes needed for the given parameters,
/// without generating any samples. Matches the C++ behavior when `buf == NULL`.
pub fn beep_buffer_size(hz: f32, length_ms: u32) -> u32 {
    let samples_per_cycle = (SAMPLE_RATE as f32 / hz) as u32;
    let mut total_samples =
        ((length_ms as f64 / 1000.0) / (1.0 / SAMPLE_RATE as f64)) as u32;
    total_samples += samples_per_cycle - (total_samples % samples_per_cycle);
    total_samples * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_size_440hz_100ms() {
        // samplesPerCycle = 44100/440 = 100 (integer truncation)
        // totalSamples = (100/1000.0) / (1.0/44100) = 4410
        // 4410 % 100 = 10, 4410 + (100 - 10) = 4500
        // 4500 * 4 = 18000 bytes
        let size = beep_buffer_size(440.0, 100);
        assert_eq!(size, 18000);
    }

    #[test]
    fn test_buffer_size_1000hz_50ms() {
        // samplesPerCycle = (int)(44100.0f/1000.0f) = 44 (integer truncation)
        // totalSamples = (int)((50/1000.0) / (1.0/44100)) = (int)(2205.0) = 2205
        // 2205 % 44 = 5, 2205 + (44 - 5) = 2244
        // 2244 * 4 = 8976 bytes
        let size = beep_buffer_size(1000.0, 50);
        assert_eq!(size, 8976);
    }

    #[test]
    fn test_stereo_panning_left_only() {
        let buf = generate_beep(440.0, 100, 100, 0);
        // Check that every right channel sample is zero.
        // Samples are interleaved i16 LE pairs: [left_lo, left_hi, right_lo, right_hi, ...]
        for frame in buf.chunks_exact(4) {
            let right = i16::from_le_bytes([frame[2], frame[3]]);
            assert_eq!(right, 0, "right channel should be zero when right=0");
        }
    }

    #[test]
    fn test_stereo_panning_right_only() {
        let buf = generate_beep(440.0, 100, 0, 100);
        // Check that every left channel sample is zero.
        for frame in buf.chunks_exact(4) {
            let left = i16::from_le_bytes([frame[0], frame[1]]);
            assert_eq!(left, 0, "left channel should be zero when left=0");
        }
    }

    #[test]
    fn test_zero_volume_both_channels() {
        let buf = generate_beep(440.0, 100, 0, 0);
        for &byte in &buf {
            assert_eq!(byte, 0, "all samples should be 0 when both channels are 0");
        }
    }

    #[test]
    fn test_complete_cycles_no_click() {
        // total_samples must be a multiple of samples_per_cycle
        let hz = 440.0_f32;
        let samples_per_cycle = (SAMPLE_RATE as f32 / hz) as u32; // 100
        let buf = generate_beep(hz, 100, 100, 100);
        let total_samples = buf.len() as u32 / 4;
        assert_eq!(
            total_samples % samples_per_cycle,
            0,
            "total samples ({}) should be a multiple of samples_per_cycle ({})",
            total_samples,
            samples_per_cycle
        );
    }
}
