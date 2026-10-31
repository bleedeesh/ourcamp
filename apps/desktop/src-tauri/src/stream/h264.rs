//! What the encoder's output must look like for `WebRTC`: Annex B access units, and a key
//! frame that carries its own SPS and PPS. Media Foundation encoders may keep those two
//! only in the media type, so they are put in front of the key frame when missing.

/// NAL unit type of a sequence parameter set.
const NAL_SPS: u8 = 7;

/// The types of the NAL units of an Annex B stream, in order.
fn nal_types(annex_b: &[u8]) -> impl Iterator<Item = u8> + '_ {
    annex_b
        .windows(4)
        .filter(|w| w[0] == 0 && w[1] == 0 && w[2] == 1)
        .map(|w| w[3] & 0x1f)
}

/// Whether this is Annex B at all: it starts with a start code.
#[cfg(test)]
#[must_use]
pub fn is_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])
}

/// A key frame ready to send: as it came if it has an SPS, else after `parameter_sets`.
#[must_use]
pub fn with_parameter_sets(frame: Vec<u8>, parameter_sets: &[u8]) -> Vec<u8> {
    if nal_types(&frame).any(|t| t == NAL_SPS) {
        return frame;
    }
    let mut joined = Vec::with_capacity(parameter_sets.len() + frame.len());
    joined.extend_from_slice(parameter_sets);
    joined.extend_from_slice(&frame);
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS_PPS: [u8; 12] = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce];
    const IDR: [u8; 6] = [0, 0, 0, 1, 0x65, 0x88];

    #[test]
    fn a_key_frame_gets_its_parameter_sets_once() {
        let joined = with_parameter_sets(IDR.to_vec(), &SPS_PPS);
        assert_eq!(joined, [&SPS_PPS[..], &IDR[..]].concat());
        assert_eq!(
            with_parameter_sets(joined.clone(), &SPS_PPS),
            joined,
            "already there"
        );
        assert_eq!(nal_types(&joined).collect::<Vec<_>>(), [7, 8, 5]);
    }

    #[test]
    fn annex_b_starts_with_a_start_code() {
        assert!(is_annex_b(&IDR));
        assert!(is_annex_b(&[0, 0, 1, 0x65]));
        assert!(!is_annex_b(&[0, 0, 0, 9, 0x65]), "length-prefixed");
        assert!(!is_annex_b(&[]));
    }
}
