/// Wave format tag, matching Windows WAVE_FORMAT_* constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveFormatTag {
    Pcm,
    IeeeFloat,
}

/// Detect leading silence in PCM or IEEE float audio data and return the byte
/// count of the silent prefix, rounded down to a `block_align` boundary.
///
/// This is the Rust equivalent of the C++ `SilenceDetect::getLeadingSilenceSize`.
pub fn get_leading_silence_size(
    format_tag: WaveFormatTag,
    bits_per_sample: u16,
    block_align: u16,
    data: &[u8],
) -> usize {
    let raw_len = match (format_tag, bits_per_sample) {
        (WaveFormatTag::Pcm, 8) => leading_silence_pcm_u8(data),
        (WaveFormatTag::Pcm, 16) => leading_silence_pcm_i16(data),
        (WaveFormatTag::Pcm, 24) => leading_silence_pcm_i24(data),
        (WaveFormatTag::Pcm, 32) => leading_silence_pcm_i32(data),
        (WaveFormatTag::IeeeFloat, 32) => leading_silence_float_f32(data),
        (WaveFormatTag::IeeeFloat, 64) => leading_silence_float_f64(data),
        _ => return 0,
    };
    // Round down to block (channel) boundary.
    let align = block_align as usize;
    if align == 0 {
        return 0;
    }
    raw_len - raw_len % align
}

// ---------------------------------------------------------------------------
// PCM integer formats
// ---------------------------------------------------------------------------

/// 8-bit unsigned PCM. Zero point is 128, threshold is 255/1024 = 0.
fn leading_silence_pcm_u8(data: &[u8]) -> usize {
    const ZERO: u8 = 128;
    const THRESHOLD: u8 = (255u16 / 1024) as u8; // 0
    let min = ZERO - THRESHOLD; // 128
    let max = ZERO + THRESHOLD; // 128
    for (i, &s) in data.iter().enumerate() {
        if s < min || s > max {
            return i;
        }
    }
    data.len()
}

/// 16-bit signed PCM, little-endian.
fn leading_silence_pcm_i16(data: &[u8]) -> usize {
    const BYTES: usize = 2;
    const THRESHOLD: i16 = i16::MAX / 1024; // 31
    let mut pos = 0;
    while pos + BYTES <= data.len() {
        let s = i16::from_le_bytes([data[pos], data[pos + 1]]);
        if !(-THRESHOLD..=THRESHOLD).contains(&s) {
            return pos;
        }
        pos += BYTES;
    }
    data.len()
}

/// 24-bit signed PCM, little-endian (3 bytes per sample, sign-extended to i32).
fn leading_silence_pcm_i24(data: &[u8]) -> usize {
    const BYTES: usize = 3;
    // Max for 24-bit signed: i32::MAX >> 8 = 8388607
    const MAX_24: i32 = i32::MAX >> 8;
    const THRESHOLD: i32 = MAX_24 / 1024; // 8191
    let mut pos = 0;
    while pos + BYTES <= data.len() {
        // Read 3 bytes into i32 and sign-extend.
        let raw = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], 0]);
        let s = ((raw as i32) << 8) >> 8; // sign-extend from bit 23
        if !(-THRESHOLD..=THRESHOLD).contains(&s) {
            return pos;
        }
        pos += BYTES;
    }
    data.len()
}

/// 32-bit signed PCM, little-endian.
fn leading_silence_pcm_i32(data: &[u8]) -> usize {
    const BYTES: usize = 4;
    const THRESHOLD: i32 = i32::MAX / 1024; // 2097151
    let mut pos = 0;
    while pos + BYTES <= data.len() {
        let s = i32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        if !(-THRESHOLD..=THRESHOLD).contains(&s) {
            return pos;
        }
        pos += BYTES;
    }
    data.len()
}

// ---------------------------------------------------------------------------
// IEEE float formats
// ---------------------------------------------------------------------------

/// 32-bit IEEE float. Range is -1.0 to 1.0, threshold is 1.0/1024.
fn leading_silence_float_f32(data: &[u8]) -> usize {
    const BYTES: usize = 4;
    const THRESHOLD: f32 = 1.0 / 1024.0;
    let mut pos = 0;
    while pos + BYTES <= data.len() {
        let s = f32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        if !(-THRESHOLD..=THRESHOLD).contains(&s) {
            return pos;
        }
        pos += BYTES;
    }
    data.len()
}

/// 64-bit IEEE float. Range is -1.0 to 1.0, threshold is 1.0/1024.
fn leading_silence_float_f64(data: &[u8]) -> usize {
    const BYTES: usize = 8;
    const THRESHOLD: f64 = 1.0 / 1024.0;
    let mut pos = 0;
    while pos + BYTES <= data.len() {
        let s = f64::from_le_bytes([
            data[pos],
            data[pos + 1],
            data[pos + 2],
            data[pos + 3],
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]);
        if !(-THRESHOLD..=THRESHOLD).contains(&s) {
            return pos;
        }
        pos += BYTES;
    }
    data.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- PCM 8-bit unsigned ---

    #[test]
    fn pcm_u8_all_silence() {
        // 128 is the zero point for unsigned 8-bit
        let data = vec![128, 128, 128, 128];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 8, 1, &data), 4);
    }

    #[test]
    fn pcm_u8_no_silence() {
        let data = vec![255, 128, 128, 128];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 8, 1, &data), 0);
    }

    #[test]
    fn pcm_u8_partial_silence() {
        let data = vec![128, 128, 128, 255];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 8, 1, &data), 3);
    }

    #[test]
    fn pcm_u8_near_zero_not_silent() {
        // For 8-bit, threshold is 255/1024 = 0, so only exactly 128 is silent
        let data = vec![129, 128];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 8, 1, &data), 0);
    }

    // --- PCM 16-bit signed ---

    #[test]
    fn pcm_i16_all_silence() {
        let data: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 8);
    }

    #[test]
    fn pcm_i16_no_silence() {
        // 0x7FFF = 32767
        let data: Vec<u8> = vec![0xFF, 0x7F, 0, 0];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 0);
    }

    #[test]
    fn pcm_i16_partial_silence() {
        let mut data: Vec<u8> = vec![0, 0, 0, 0];
        data.extend_from_slice(&[0xFF, 0x7F]);
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 4);
    }

    #[test]
    fn pcm_i16_threshold_boundary() {
        // Threshold is 32767/1024 = 31
        // Value 31 should be silent (not > threshold)
        let data = 31i16.to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 2);
        // Value 32 should NOT be silent (> threshold)
        let data = 32i16.to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 0);
        // Negative threshold: -31 should be silent
        let data = (-31i16).to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 2);
        // -32 should NOT be silent
        let data = (-32i16).to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &data), 0);
    }

    // --- PCM 24-bit signed ---

    #[test]
    fn pcm_i24_all_silence() {
        let data = vec![0, 0, 0, 0, 0, 0];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 3, &data), 6);
    }

    #[test]
    fn pcm_i24_loud_sample() {
        // 0x7FFFFF = 8388607 (max positive 24-bit)
        let data = vec![0xFF, 0xFF, 0x7F];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 3, &data), 0);
    }

    #[test]
    fn pcm_i24_negative_loud() {
        // 0x800000 = -8388608 in signed 24-bit
        let data = vec![0x00, 0x00, 0x80];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 3, &data), 0);
    }

    #[test]
    fn pcm_i24_threshold_boundary() {
        // Threshold is 8388607/1024 = 8191
        // 8191 in 24-bit LE: 0xFF, 0x1F, 0x00
        let val: i32 = 8191;
        let data = [val as u8, (val >> 8) as u8, (val >> 16) as u8];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 3, &data), 3);
        // 8192 should NOT be silent
        let val: i32 = 8192;
        let data = [val as u8, (val >> 8) as u8, (val >> 16) as u8];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 3, &data), 0);
    }

    // --- PCM 32-bit signed ---

    #[test]
    fn pcm_i32_all_silence() {
        let data: Vec<u8> = vec![0; 8];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 32, 4, &data), 8);
    }

    #[test]
    fn pcm_i32_threshold_boundary() {
        // Threshold is 2147483647/1024 = 2097151
        let data = 2097151i32.to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 32, 4, &data), 4);
        let data = 2097152i32.to_le_bytes();
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 32, 4, &data), 0);
    }

    // --- IEEE float 32-bit ---

    #[test]
    fn float_f32_all_silence() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.0f32.to_le_bytes());
        data.extend_from_slice(&0.0f32.to_le_bytes());
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 32, 4, &data),
            8
        );
    }

    #[test]
    fn float_f32_no_silence() {
        let data = 1.0f32.to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 32, 4, &data),
            0
        );
    }

    #[test]
    fn float_f32_threshold_boundary() {
        // Threshold is 1.0/1024 ≈ 0.0009765625
        let threshold: f32 = 1.0 / 1024.0;
        // At threshold: silent
        let data = threshold.to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 32, 4, &data),
            4
        );
        // Just above threshold: not silent
        let data = (threshold + 0.0001).to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 32, 4, &data),
            0
        );
        // Negative at threshold: silent
        let data = (-threshold).to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 32, 4, &data),
            4
        );
    }

    // --- IEEE float 64-bit ---

    #[test]
    fn float_f64_all_silence() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.0f64.to_le_bytes());
        data.extend_from_slice(&0.0f64.to_le_bytes());
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 64, 8, &data),
            16
        );
    }

    #[test]
    fn float_f64_threshold_boundary() {
        let threshold: f64 = 1.0 / 1024.0;
        let data = threshold.to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 64, 8, &data),
            8
        );
        let data = (threshold + 0.0001).to_le_bytes();
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 64, 8, &data),
            0
        );
    }

    // --- Block alignment ---

    #[test]
    fn block_align_rounding_stereo_16bit() {
        // Stereo 16-bit: block_align = 4
        let mut data: Vec<u8> = vec![0, 0, 0, 0]; // 1 stereo frame of silence
        data.extend_from_slice(&[0xFF, 0x7F, 0xFF, 0x7F]); // loud stereo frame
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 4, &data), 4);
    }

    #[test]
    fn block_align_rounding_rounds_down() {
        // Mono 24-bit with block_align = 6 (stereo 24-bit)
        // 3 silent mono samples = 9 bytes, rounded down to 6
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 6, &[0; 9]), 6);
        // But with non-silent at sample 4 (byte 9), first 3 samples (9 bytes) are silent
        let mut data = vec![0u8; 9];
        data.extend_from_slice(&[0xFF, 0xFF, 0x7F]); // loud 24-bit sample
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 24, 6, &data), 6);
    }

    // --- Edge cases ---

    #[test]
    fn empty_data() {
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &[]), 0);
    }

    #[test]
    fn unknown_format() {
        let data = vec![0; 8];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 12, 2, &data), 0);
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::IeeeFloat, 16, 2, &data),
            0
        );
    }

    #[test]
    fn zero_block_align() {
        let data = vec![0; 8];
        assert_eq!(get_leading_silence_size(WaveFormatTag::Pcm, 16, 0, &data), 0);
    }

    #[test]
    fn data_shorter_than_one_sample() {
        // 1 byte is not enough for a 16-bit sample
        assert_eq!(
            get_leading_silence_size(WaveFormatTag::Pcm, 16, 2, &[0]),
            0
        );
    }
}
