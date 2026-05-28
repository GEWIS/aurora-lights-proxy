pub const ARTNET_HEADER_LEN: usize = 18;
pub const ARTNET_ID: &[u8; 8] = b"Art-Net\0";
pub const OP_DMX: u16 = 0x5000;
pub const PROTOCOL_VERSION: u16 = 14;
pub const ARTNET_PORT: u16 = 6454;

/// Art-Net DMX frames carry at most one universe (512 channels) of data.
pub const MAX_DMX_PAYLOAD: usize = 512;

/// Clamp every input value into 0..=255 and pad or truncate to `desired_length`.
///
/// Mirrors the Python `parse_array` helper: values below zero become 0, values
/// above 255 are saturated, and the resulting vector is padded with zeros when
/// the source is shorter than the universe. Input values are accepted as i32 so
/// that out-of-range numbers from the wire are clamped instead of overflowing.
#[must_use]
pub fn parse_array(input: &[i32], desired_length: usize) -> Vec<u8> {
    #[allow(clippy::cast_sign_loss)] // value is clamped to 0..=255 before the cast
    let mut out: Vec<u8> = input.iter().map(|&x| x.clamp(0, 255) as u8).collect();

    if out.len() < desired_length {
        out.resize(desired_length, 0);
    } else if out.len() > desired_length {
        out.truncate(desired_length);
    }

    out
}

/// Build an Art-Net `OpDmx` packet for the given universe and DMX payload.
///
/// `sequence` of 0 disables sequence tracking on the receiver. The returned
/// vector contains the full 18-byte header followed by the data. Length is
/// rounded up to an even number as required by the spec. Payloads longer than
/// `MAX_DMX_PAYLOAD` are truncated so the length field can never overflow the
/// 16-bit on-wire encoding.
#[must_use]
pub fn build_artnet_frame(universe: u16, sequence: u8, data: &[u8]) -> Vec<u8> {
    let payload = &data[..data.len().min(MAX_DMX_PAYLOAD)];
    let mut length = payload.len();
    if !length.is_multiple_of(2) {
        length += 1;
    }

    let mut frame = Vec::with_capacity(ARTNET_HEADER_LEN + length);
    frame.extend_from_slice(ARTNET_ID);
    frame.extend_from_slice(&OP_DMX.to_le_bytes());
    frame.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    frame.push(sequence);
    frame.push(0); // physical
    frame.push((universe & 0xFF) as u8);
    frame.push(((universe >> 8) & 0x7F) as u8);
    #[allow(clippy::cast_possible_truncation)] // length ≤ MAX_DMX_PAYLOAD (512) always fits in u16
    frame.extend_from_slice(&(length as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    if payload.len() != length {
        frame.push(0);
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_array_clamps_negative_and_over_255() {
        let out = parse_array(&[-5, 0, 100, 255, 300, 999], 6);
        assert_eq!(out, vec![0, 0, 100, 255, 255, 255]);
    }

    #[test]
    fn parse_array_pads_when_shorter() {
        let out = parse_array(&[10, 20, 30], 6);
        assert_eq!(out, vec![10, 20, 30, 0, 0, 0]);
    }

    #[test]
    fn parse_array_truncates_when_longer() {
        let out = parse_array(&[1, 2, 3, 4, 5], 3);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn parse_array_handles_exact_length() {
        let out = parse_array(&[1, 2, 3], 3);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn parse_array_empty_pads_to_full_universe() {
        let out = parse_array(&[], 512);
        assert_eq!(out.len(), 512);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn build_artnet_frame_has_correct_header() {
        let data = vec![0u8; 512];
        let frame = build_artnet_frame(0, 0, &data);

        assert_eq!(&frame[0..8], b"Art-Net\0");
        // OpDmx little-endian
        assert_eq!(&frame[8..10], &[0x00, 0x50]);
        // Protocol version big-endian
        assert_eq!(&frame[10..12], &[0x00, 0x0E]);
        // Sequence + physical
        assert_eq!(frame[12], 0);
        assert_eq!(frame[13], 0);
        // SubUni + Net for universe 0
        assert_eq!(frame[14], 0);
        assert_eq!(frame[15], 0);
        // Length big-endian, 512
        assert_eq!(&frame[16..18], &[0x02, 0x00]);
        assert_eq!(frame.len(), ARTNET_HEADER_LEN + 512);
    }

    #[test]
    fn build_artnet_frame_splits_universe_into_subuni_and_net() {
        let data = vec![0u8; 2];
        // Universe 0x0123 -> SubUni=0x23, Net=0x01
        let frame = build_artnet_frame(0x0123, 7, &data);
        assert_eq!(frame[12], 7); // sequence preserved
        assert_eq!(frame[14], 0x23);
        assert_eq!(frame[15], 0x01);
    }

    #[test]
    fn build_artnet_frame_pads_odd_length_to_even() {
        let data = vec![0xAAu8; 3];
        let frame = build_artnet_frame(0, 0, &data);
        // Length field reports 4 (rounded up from 3)
        assert_eq!(&frame[16..18], &[0x00, 0x04]);
        // Payload is 4 bytes total
        assert_eq!(frame.len(), ARTNET_HEADER_LEN + 4);
        assert_eq!(&frame[ARTNET_HEADER_LEN..], &[0xAA, 0xAA, 0xAA, 0x00]);
    }

    #[test]
    fn build_artnet_frame_masks_net_to_7_bits() {
        let frame = build_artnet_frame(0xFFFF, 0, &[0u8; 2]);
        // Net high bit must be cleared
        assert_eq!(frame[15], 0x7F);
        assert_eq!(frame[14], 0xFF);
    }

    #[test]
    fn build_artnet_frame_truncates_oversized_payload() {
        let data = vec![0xCDu8; 2048];
        let frame = build_artnet_frame(0, 0, &data);
        // Length field caps at MAX_DMX_PAYLOAD, with header in front.
        assert_eq!(frame.len(), ARTNET_HEADER_LEN + MAX_DMX_PAYLOAD);
        assert_eq!(
            &frame[16..18],
            &(MAX_DMX_PAYLOAD as u16).to_be_bytes(),
            "length field should report exactly MAX_DMX_PAYLOAD"
        );
    }
}
