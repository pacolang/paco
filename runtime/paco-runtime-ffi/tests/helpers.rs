#![allow(clippy::approx_constant, clippy::excessive_precision)]

use paco_runtime_ffi::*;

fn show(out: &mut String, s: PacoStr) {
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len as usize) };
    out.push_str(std::str::from_utf8(bytes).unwrap());
    out.push('\n');
    unsafe { paco_free(s.ptr.cast_mut().cast()) };
}

fn empty() -> PacoStr {
    PacoStr { ptr: std::ptr::null(), len: 0 }
}

fn str_of(bytes: &[u8]) -> PacoStr {
    PacoStr { ptr: bytes.as_ptr(), len: bytes.len() as i64 }
}

/// C `printf("%.<precision>g")`, including `nan`/`-nan`/`inf` spelling.
fn format_g(value: f64, precision: usize) -> String {
    if value.is_nan() {
        return if value.is_sign_negative() { "-nan" } else { "nan" }.to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let precision = precision.max(1);
    let scientific = format!("{:.*e}", precision - 1, value);
    let (mantissa, exponent) = scientific.split_once('e').expect("`{:e}` always has an exponent");
    let exponent: i32 = exponent.parse().expect("`{:e}` exponent is an integer");
    let strip = |digits: &str| -> String {
        if digits.contains('.') { digits.trim_end_matches('0').trim_end_matches('.').to_string() } else { digits.to_string() }
    };
    if exponent < -4 || exponent >= precision as i32 {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), exponent.unsigned_abs())
    } else {
        strip(&format!("{:.*}", (precision as i32 - 1 - exponent) as usize, value))
    }
}

#[test]
fn formatting_matches_the_c_runtime_goldens() {
    let floats = [
        0.0, -0.0, 1e-5, 1e-4, 1e16, 1e15, 123456.0, 1234567.0, 0.1 + 0.2, f64::NAN, -f64::NAN, f64::INFINITY,
        f64::NEG_INFINITY, 1.0, -2.5, 3.14159265358979, 1e100, 1e-300, 5e-324, 1.7976931348623157e308, 0.5, 999999.5,
        9999995.0, 0.00001234565, 100000.0, 2.5e-5, 123.456, -0.000123456789, 1e21, 4.9406564584124654e-324,
    ];
    let mut out = String::new();
    for value in floats {
        let mut s = empty();
        unsafe { paco_float_to_string(value, FLOAT_CODE_F64, &mut s) };
        show(&mut out, s);
    }
    for value in [0, -1, 1, i64::MAX, i64::MIN, 42] {
        let mut s = empty();
        unsafe { paco_int_to_string(value, &mut s) };
        show(&mut out, s);
    }
    for code in 0..4 {
        let (limit, step) = if code < 2 { (65536u32, 97) } else { (256, 1) };
        for bits in (0..limit).step_by(step) {
            out.push_str(&format!("{code} {bits} {}\n", format_g(paco_float_to_f64(bits, code), 17)));
        }
        let values = [
            0.0, -0.0, 1.0, -1.0, 0.1, 65504.0, 65520.0, 1e10, -1e10, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 448.0,
            464.0, 57344.0, 1e-8, 6e-8, 3.0e-5, 0.3333333, 2.5, -0.75, 1e-40, 240.0, 300.0,
        ];
        for value in values {
            out.push_str(&format!("{code} {} -> {}\n", format_g(value, 17), paco_float_from_f64(value, code)));
        }
    }
    let golden = include_str!("c_formatting.golden");
    for (line, (got, want)) in out.lines().zip(golden.lines()).enumerate() {
        assert_eq!(got, want, "line {}", line + 1);
    }
    assert_eq!(out.lines().count(), golden.lines().count());
}

#[test]
fn string_helpers_follow_utf8_boundaries() {
    let text = "aé€😀".as_bytes();
    let s = str_of(text);
    let mut pos = 0;
    let mut boundaries = vec![];
    while pos < s.len {
        pos = unsafe { paco_string_next_char_boundary(&s, pos) };
        boundaries.push(pos);
    }
    assert_eq!(boundaries, [1, 3, 6, 10]);
    let mut c = 0;
    assert_eq!(unsafe { paco_string_char_at(&s, 6, &mut c) }, 1);
    assert_eq!(c, '😀' as u32);
    assert_eq!(unsafe { paco_string_char_at(&s, 2, &mut c) }, 0);
    let mut byte = 0;
    assert_eq!(unsafe { paco_string_byte_at(&s, 1, &mut byte) }, 1);
    assert_eq!(byte, 0xC3);
    let mut out = String::new();
    let mut slice = empty();
    assert_eq!(unsafe { paco_string_slice_utf8(&s, 1, 6, &mut slice) }, 1);
    show(&mut out, slice);
    assert_eq!(unsafe { paco_string_slice_utf8(&s, 2, 6, &mut slice) }, 0);
    let mut joined = empty();
    unsafe { paco_string_concat(&str_of(b"ab"), &str_of(b"cd"), &mut joined) };
    show(&mut out, joined);
    for c in ['x', 'é', '😀'] {
        let mut s = empty();
        unsafe { paco_char_to_string(c as u32, &mut s) };
        show(&mut out, s);
    }
    let mut b = empty();
    unsafe { paco_bool_to_string(1, &mut b) };
    show(&mut out, b);
    assert_eq!(out, "é€\nabcd\nx\né\n😀\ntrue\n");
    assert_eq!(unsafe { paco_string_eq(&str_of(b"ab"), &str_of(b"ab")) }, 1);
    assert_eq!(unsafe { paco_string_eq(&str_of(b"ab"), &str_of(b"ac")) }, 0);
}

#[test]
fn read_to_string_rejects_invalid_utf8_and_missing_files() {
    let dir = std::env::temp_dir().join(format!("paco-helpers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good.txt");
    let bad = dir.join("bad.txt");
    std::fs::write(&good, "olá").unwrap();
    std::fs::write(&bad, [0xFF, 0xFE]).unwrap();
    let mut out = empty();
    let path = good.to_str().unwrap().as_bytes();
    assert_eq!(unsafe { paco_fs_read_to_string(&str_of(path), &mut out) }, 1);
    let mut text = String::new();
    show(&mut text, out);
    assert_eq!(text, "olá\n");
    assert_eq!(unsafe { paco_fs_read_to_string(&str_of(bad.to_str().unwrap().as_bytes()), &mut out) }, 0);
    assert_eq!(unsafe { paco_fs_read_to_string(&str_of(dir.join("none").to_str().unwrap().as_bytes()), &mut out) }, 0);
    assert_eq!(unsafe { paco_fs_read_to_string(&str_of(b"a\0b"), &mut out) }, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn allocation_functions_allocate_grow_and_free() {
    let p = paco_alloc(16).cast::<u8>();
    assert!(!p.is_null());
    unsafe { p.write_bytes(7, 16) };
    let grown = unsafe { paco_realloc(p.cast(), 4096) }.cast::<u8>();
    assert_eq!(unsafe { std::slice::from_raw_parts(grown, 16) }, [7; 16]);
    let zeroed = paco_calloc(8, 8).cast::<u64>();
    assert_eq!(unsafe { std::slice::from_raw_parts(zeroed, 8) }, [0; 8]);
    unsafe {
        paco_free(grown.cast());
        paco_free(zeroed.cast());
    }
}

#[test]
fn concat_copies_every_length_exactly() {
    let source: Vec<u8> = (0..80u8).map(|byte| b'a' + byte % 26).collect();
    for len in 0..source.len() {
        let mut joined = empty();
        unsafe { paco_string_concat(&str_of(&source[..len]), &str_of(b"|"), &mut joined) };
        let bytes = unsafe { std::slice::from_raw_parts(joined.ptr, joined.len as usize) };
        assert_eq!(&bytes[..len], &source[..len], "length {len}");
        assert_eq!(bytes[len], b'|');
        unsafe { paco_free(joined.ptr.cast_mut().cast()) };
    }
}

#[test]
fn byte_helpers_copy_write_and_validate() {
    let text = "héllo";
    let mut bytes = empty();
    unsafe { paco_string_to_bytes(&str_of(text.as_bytes()), &mut bytes) };
    assert_eq!(unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len as usize) }, text.as_bytes());

    let mut out = empty();
    assert_eq!(unsafe { paco_string_from_bytes(&bytes, 0, 6, &mut out) }, 1);
    let mut shown = String::new();
    show(&mut shown, out);
    assert_eq!(shown, "héllo\n");
    assert_eq!(unsafe { paco_string_from_bytes(&bytes, 0, 2, &mut out) }, 0);

    assert_eq!(unsafe { paco_bytes_write_string(&bytes, 3, &str_of(b"LL")) }, 1);
    assert_eq!(unsafe { std::slice::from_raw_parts(bytes.ptr, 6) }, "héLLo".as_bytes());
    assert_eq!(unsafe { paco_bytes_write_string(&bytes, 5, &str_of(b"xy")) }, 0);
    assert_eq!(unsafe { paco_bytes_write_string(&bytes, -1, &str_of(b"x")) }, 0);
    assert_eq!(unsafe { paco_string_hash(&str_of(b"a")) }, 0xaf63_dc4c_8601_ec8c);
    unsafe { paco_free(bytes.ptr.cast_mut().cast()) };
}
