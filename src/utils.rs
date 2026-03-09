/// Minimal percent-encoder for use in redirect query params.
///
/// Encodes all bytes except unreserved characters (A-Z, a-z, 0-9, -, _, ., ~).
/// Spaces are encoded as `+` (application/x-www-form-urlencoded convention),
/// which browsers and the dashboard query parser both handle correctly.
pub(crate) fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            b => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_chars_pass_through() {
        assert_eq!(pct_encode("hello"), "hello");
        assert_eq!(pct_encode("abc-123_ok"), "abc-123_ok");
    }

    #[test]
    fn spaces_become_plus() {
        assert_eq!(pct_encode("hello world"), "hello+world");
    }

    #[test]
    fn special_chars_are_encoded() {
        assert_eq!(pct_encode("a=b&c=d"), "a%3Db%26c%3Dd");
    }
}
