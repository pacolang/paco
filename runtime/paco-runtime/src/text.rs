//! String and scalar-to-text helpers shared by the runtime's C entry points
//! and the compiler's compile-time evaluator.

pub fn encode_char(c: u32) -> ([u8; 4], usize) {
    let mut buf = [0u8; 4];
    let len = match c {
        0..0x80 => {
            buf[0] = c as u8;
            1
        }
        0x80..0x800 => {
            buf[0] = (0xC0 | (c >> 6)) as u8;
            buf[1] = (0x80 | (c & 0x3F)) as u8;
            2
        }
        0x800..0x10000 => {
            buf[0] = (0xE0 | (c >> 12)) as u8;
            buf[1] = (0x80 | ((c >> 6) & 0x3F)) as u8;
            buf[2] = (0x80 | (c & 0x3F)) as u8;
            3
        }
        _ => {
            buf[0] = (0xF0 | (c >> 18)) as u8;
            buf[1] = (0x80 | ((c >> 12) & 0x3F)) as u8;
            buf[2] = (0x80 | ((c >> 6) & 0x3F)) as u8;
            buf[3] = (0x80 | (c & 0x3F)) as u8;
            4
        }
    };
    (buf, len)
}

fn is_boundary(s: &[u8], pos: i64) -> bool {
    pos == 0 || pos == s.len() as i64 || (pos > 0 && pos < s.len() as i64 && s[pos as usize] & 0xC0 != 0x80)
}

fn char_width(lead: u8) -> i64 {
    match lead {
        0..0x80 => 1,
        0xF0.. => 4,
        0xE0.. => 3,
        _ => 2,
    }
}

pub fn next_char_boundary(s: &[u8], pos: i64) -> i64 {
    if pos < 0 || pos >= s.len() as i64 { pos } else { pos + char_width(s[pos as usize]) }
}

pub fn char_at(s: &[u8], pos: i64) -> Option<u32> {
    if pos < 0 || pos >= s.len() as i64 || !is_boundary(s, pos) {
        return None;
    }
    let p = &s[pos as usize..];
    let width = char_width(p[0]) as usize;
    let mut c = if width == 1 { p[0] as u32 } else { (p[0] & (0x7F >> width)) as u32 };
    for &byte in &p[1..width.min(p.len())] {
        c = (c << 6) | (byte & 0x3F) as u32;
    }
    Some(c)
}

pub fn byte_at(s: &[u8], pos: i64) -> Option<i64> {
    (pos >= 0 && pos < s.len() as i64).then(|| s[pos as usize] as i64)
}

pub fn slice_utf8(s: &[u8], start: i64, end: i64) -> Option<&[u8]> {
    if start < 0 || end < start || end > s.len() as i64 || !is_boundary(s, start) || !is_boundary(s, end) {
        return None;
    }
    Some(&s[start as usize..end as usize])
}

/// `bytes[start..end]` when the range is in bounds and valid UTF-8.
pub fn from_utf8_range(bytes: &[u8], start: i64, end: i64) -> Option<&[u8]> {
    if start < 0 || end < start || end > bytes.len() as i64 {
        return None;
    }
    let range = &bytes[start as usize..end as usize];
    std::str::from_utf8(range).is_ok().then_some(range)
}

/// 64-bit FNV-1a, the hash `string` keys use.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3))
}

pub fn int_to_string(value: i64) -> String {
    value.to_string()
}

pub fn uint_to_string(value: u64) -> String {
    value.to_string()
}

pub fn bool_to_string(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_helpers_respect_char_boundaries() {
        let text = "aé€😀".as_bytes();
        assert_eq!(next_char_boundary(text, 1), 3);
        assert_eq!(char_at(text, 1), Some('é' as u32));
        assert_eq!(char_at(text, 2), None);
        assert_eq!(char_at(text, 6), Some('😀' as u32));
        assert_eq!(byte_at(text, 0), Some(97));
        assert_eq!(byte_at(text, 99), None);
        assert_eq!(slice_utf8(text, 1, 6), Some("é€".as_bytes()));
        assert_eq!(slice_utf8(text, 2, 6), None);
        let (buf, len) = encode_char('€' as u32);
        assert_eq!(&buf[..len], "€".as_bytes());
    }

    #[test]
    fn byte_ranges_convert_only_when_valid_utf8() {
        let text = "héllo".as_bytes();
        assert_eq!(from_utf8_range(text, 0, 6), Some(text));
        assert_eq!(from_utf8_range(text, 0, 2), None);
        assert_eq!(from_utf8_range(text, 3, 6), Some("llo".as_bytes()));
        assert_eq!(from_utf8_range(text, 4, 3), None);
        assert_eq!(from_utf8_range(text, 0, 7), None);
        assert_eq!(hash_bytes(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash_bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(hash_bytes(b"ab"), hash_bytes(b"ba"));
    }

    #[test]
    fn scalars_render_like_the_language_prints_them() {
        assert_eq!(int_to_string(-42), "-42");
        assert_eq!(uint_to_string(u64::MAX), "18446744073709551615");
        assert_eq!(bool_to_string(true), "true");
        assert_eq!(crate::format_float_code(0.1 + 0.2, crate::FLOAT_CODE_F64), "0.30000000000000004");
        assert_eq!(crate::format_float_code(65504.0, 0), "65500");
        assert_eq!(crate::float_to_f64(crate::float_from_f64(1.0 / 3.0, 1), 1), 0.333984375);
    }
}
