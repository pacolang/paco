//! Stable sorting of primitive slices, shared by compiled code and the
//! compile-time interpreter.

use std::slice::from_raw_parts_mut as parts;

use crate::float::float_to_f64;

/// The element kinds `sort_raw` understands, as the compiler numbers them.
pub const SORT_I8: i32 = 0;
pub const SORT_I16: i32 = 1;
pub const SORT_I32: i32 = 2;
pub const SORT_I64: i32 = 3;
pub const SORT_U8: i32 = 4;
pub const SORT_U16: i32 = 5;
pub const SORT_U32: i32 = 6;
pub const SORT_U64: i32 = 7;
pub const SORT_F32: i32 = 8;
pub const SORT_F64: i32 = 9;
pub const SORT_F16: i32 = 10;
pub const SORT_BF16: i32 = 11;

/// Byte size of one element of `kind`.
pub fn sort_element_size(kind: i32) -> usize {
    match kind {
        SORT_I8 | SORT_U8 => 1,
        SORT_I16 | SORT_U16 | SORT_F16 | SORT_BF16 => 2,
        SORT_I32 | SORT_U32 | SORT_F32 => 4,
        _ => 8,
    }
}

/// Sorts `len` elements of `kind` in place, stably. Floats follow the IEEE
/// total order: `-NaN < -inf < ... < -0.0 < 0.0 < ... < inf < NaN`.
///
/// # Safety
/// `data` must point to `len` initialized, suitably aligned elements of `kind`.
pub unsafe fn sort_raw(data: *mut u8, len: usize, kind: i32) {
    unsafe {
        match kind {
            SORT_I8 => parts(data.cast::<i8>(), len).sort(),
            SORT_I16 => parts(data.cast::<i16>(), len).sort(),
            SORT_I32 => parts(data.cast::<i32>(), len).sort(),
            SORT_I64 => parts(data.cast::<i64>(), len).sort(),
            SORT_U8 => parts(data, len).sort(),
            SORT_U16 => parts(data.cast::<u16>(), len).sort(),
            SORT_U32 => parts(data.cast::<u32>(), len).sort(),
            SORT_U64 => parts(data.cast::<u64>(), len).sort(),
            SORT_F32 => parts(data.cast::<f32>(), len).sort_by(f32::total_cmp),
            SORT_F64 => parts(data.cast::<f64>(), len).sort_by(f64::total_cmp),
            SORT_F16 | SORT_BF16 => {
                let code = kind - SORT_F16;
                parts(data.cast::<u16>(), len)
                    .sort_by(|a, b| float_to_f64(u32::from(*a), code).total_cmp(&float_to_f64(u32::from(*b), code)));
            }
            _ => {}
        }
    }
}

/// `sort_raw` over little-endian bytes, which need not be aligned.
pub fn sort_bytes(bytes: &mut [u8], kind: i32) {
    let size = sort_element_size(kind);
    let len = bytes.len() / size;
    let mut words = vec![0u64; bytes.len().div_ceil(8)];
    let aligned = words.as_mut_ptr().cast::<u8>();
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), aligned, len * size);
        sort_raw(aligned, len, kind);
        std::ptr::copy_nonoverlapping(aligned, bytes.as_mut_ptr(), len * size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_every_kind() {
        let mut ints: Vec<u8> = [5i64, -3, 9, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        sort_bytes(&mut ints, SORT_I64);
        let sorted: Vec<i64> = ints.chunks(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(sorted, [-3, 0, 5, 9]);

        let mut floats: Vec<u8> = [2.0f64, f64::NAN, -0.0, 0.0, -1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        sort_bytes(&mut floats, SORT_F64);
        let sorted: Vec<f64> = floats.chunks(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(sorted[..4], [-1.0, -0.0, 0.0, 2.0]);
        assert!(sorted[1].is_sign_negative() && sorted[4].is_nan());

        let mut bytes = vec![3u8, 1, 2];
        sort_bytes(&mut bytes, SORT_U8);
        assert_eq!(bytes, [1, 2, 3]);
    }
}
