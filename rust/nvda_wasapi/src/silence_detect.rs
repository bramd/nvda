/// Read a single PCM sample from raw bytes, returning it as a signed i64.
/// For 8-bit PCM (unsigned), the zero point is 128, so we subtract 128 to get signed.
/// For 16/24/32-bit PCM, samples are signed little-endian integers.
fn read_sample(data: &[u8], bits_per_sample: u16) -> i64 {
    match bits_per_sample {
        8 => data[0] as i64 - 128,
        16 => i16::from_le_bytes([data[0], data[1]]) as i64,
        24 => {
            // Sign-extend 24-bit to 32-bit, then to i64
            let raw = u32::from_le_bytes([data[0], data[1], data[2], 0]);
            // If bit 23 is set, sign-extend
            if raw & 0x80_0000 != 0 {
                (raw | 0xFF00_0000) as i32 as i64
            } else {
                raw as i64
            }
        }
        32 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i64,
        _ => 0,
    }
}

/// Returns the default silence threshold for a given bits_per_sample,
/// matching the C++ `defaultThreshold()` which is max_value / 1024.
fn default_threshold(bits_per_sample: u16) -> i64 {
    match bits_per_sample {
        8 => 127 / 1024,       // 0
        16 => 32767 / 1024,    // 31
        24 => 8388607 / 1024,  // 8191
        32 => 2147483647 / 1024, // 2097151
        _ => 0,
    }
}

/// Detect leading silence in PCM audio data and return the byte count of the
/// silent prefix, rounded down to a block_align boundary.
///
/// This is the Rust equivalent of the C++ `SilenceDetect::getLeadingSilenceSize`.
pub fn get_leading_silence_size(bits_per_sample: u16, block_align: u16, data: &[u8]) -> usize {
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    if bytes_per_sample == 0 || block_align == 0 {
        return 0;
    }
    let threshold = default_threshold(bits_per_sample);
    let mut pos = 0;
    while pos + bytes_per_sample <= data.len() {
        let sample = read_sample(&data[pos..], bits_per_sample);
        if sample.abs() > threshold {
            // Found non-silent sample; round down to block_align boundary
            let align = block_align as usize;
            return (pos / align) * align;
        }
        pos += bytes_per_sample;
    }
    // Entire buffer is silent; round down to block_align boundary
    let align = block_align as usize;
    (data.len() / align) * align
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_silence_16bit() {
        let data: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0];
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 8);
    }

    #[test]
    fn test_no_silence_16bit() {
        let data: Vec<u8> = vec![0xFF, 0x7F, 0, 0];
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_partial_silence_16bit() {
        let mut data: Vec<u8> = vec![0, 0, 0, 0];
        data.extend_from_slice(&[0xFF, 0x7F]);
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 4);
    }

    #[test]
    fn test_8bit_unsigned_silence() {
        // 8-bit PCM is unsigned, zero point is 128
        let data: Vec<u8> = vec![128, 128, 128, 255];
        let result = get_leading_silence_size(8, 1, &data);
        assert_eq!(result, 3);
    }

    #[test]
    fn test_block_align_rounding() {
        // Stereo 16-bit: nBlockAlign = 4
        let mut data: Vec<u8> = vec![0, 0, 0, 0];
        data.extend_from_slice(&[0xFF, 0x7F, 0xFF, 0x7F]);
        let result = get_leading_silence_size(16, 4, &data);
        assert_eq!(result, 4);
    }
}
