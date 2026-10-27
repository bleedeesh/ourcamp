//! Crockford base32 (lower-case output, case-insensitive input, `o`→0 and `i`/`l`→1).
//!
//! Chosen for invite tokens because it has no visually ambiguous characters and no
//! characters that need URL escaping.

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Encodes bytes, MSB first, without padding.
/// Encodes bytes as lower-case Crockford base32 (no padding).
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn value(c: u8) -> Option<u32> {
    let c = c.to_ascii_lowercase();
    let v = match c {
        b'o' => 0,
        b'i' | b'l' => 1,
        b'0'..=b'9' => c - b'0',
        b'a'..=b'h' => c - b'a' + 10,
        b'j' | b'k' => c - b'j' + 18,
        b'm' | b'n' => c - b'm' + 20,
        b'p'..=b't' => c - b'p' + 22,
        b'v'..=b'z' => c - b'v' + 27,
        _ => return None,
    };
    Some(u32::from(v))
}

/// Decodes text produced by [`encode`]. Returns `None` on invalid characters or
/// non-zero padding bits (which would mean the text was not produced by `encode`).
/// Decodes Crockford base32, case-insensitively, mapping look-alikes; `None` if invalid.
#[must_use]
pub fn decode(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        buffer = (buffer << 5) | value(c)?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    if bits >= 5 || buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0]), "00");
        assert_eq!(encode(&[0xFF]), "zw");
        assert_eq!(encode(b"hello"), "d1jprv3f");
    }

    #[test]
    fn round_trip_all_lengths() {
        for len in 1..40 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&data)).unwrap(), data, "len {len}");
        }
    }

    #[test]
    fn case_and_look_alikes() {
        assert_eq!(decode("D1JPRV3F").unwrap(), b"hello");
        assert_eq!(decode("oo").unwrap(), decode("00").unwrap());
        assert_eq!(decode("l0").unwrap(), decode("10").unwrap());
        assert_eq!(decode("I0").unwrap(), decode("10").unwrap());
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(decode("").is_none());
        assert!(decode("u0").is_none(), "u is excluded from Crockford");
        assert!(decode("d1-j").is_none());
        assert!(decode("zz").is_none(), "non-zero padding bits");
        assert!(decode("000").is_none(), "dangling 5-bit group");
    }
}
