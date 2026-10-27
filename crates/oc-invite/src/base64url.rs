//! Unpadded base64url (RFC 4648 §5): fingerprints in links, keys and signatures in the API.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encodes bytes as unpadded base64url.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn value(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

/// Strict decoder: rejects padding, invalid characters, impossible lengths and
/// non-canonical trailing bits.
/// Decodes unpadded base64url; `None` if invalid.
#[must_use]
pub fn decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = chunk.len() - 1;
        let unused_mask = match bytes {
            1 => 0xFFFF,
            2 => 0xFF,
            _ => 0,
        };
        if n & unused_mask != 0 {
            return None;
        }
        for i in 0..bytes {
            out.push(((n >> (16 - 8 * i)) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors_without_padding() {
        let vectors: [(&[u8], &str); 7] = [
            (b"", ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg"),
            (b"fooba", "Zm9vYmE"),
            (b"foobar", "Zm9vYmFy"),
        ];
        for (raw, text) in vectors {
            assert_eq!(encode(raw), text);
            assert_eq!(decode(text).unwrap(), raw);
        }
    }

    #[test]
    fn url_safe_alphabet() {
        assert_eq!(encode(&[0xFB, 0xFF]), "-_8");
        assert_eq!(decode("-_8").unwrap(), [0xFB, 0xFF]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(decode("Zg==").is_none(), "padding not allowed");
        assert!(decode("Z").is_none(), "impossible length");
        assert!(decode("Zh").is_none(), "non-canonical trailing bits");
        assert!(decode("Zm+v").is_none(), "standard alphabet not allowed");
    }
}
