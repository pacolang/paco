//! Bit encoding of the reduced-precision float formats. Narrowing rounds to
//! nearest, ties to even; overflow becomes infinity (NaN for `f8e4m3`).

use crate::FloatWidth;

struct Format {
    exponent_bits: u32,
    mantissa_bits: u32,
    has_infinity: bool,
}

impl FloatWidth {
    fn format(self) -> Option<Format> {
        let (exponent_bits, mantissa_bits, has_infinity) = match self {
            FloatWidth::F64 | FloatWidth::F32 => return None,
            FloatWidth::F16 => (5, 10, true),
            FloatWidth::BF16 => (8, 7, true),
            FloatWidth::F8E4M3 => (4, 3, false),
            FloatWidth::F8E5M2 => (5, 2, true),
        };
        Some(Format { exponent_bits, mantissa_bits, has_infinity })
    }

    /// The nearest value of this format, as an `f64` (exact for every format).
    pub fn round(self, value: f64) -> f64 {
        match self {
            FloatWidth::F64 => value,
            FloatWidth::F32 => f64::from(value as f32),
            _ => self.decode(self.encode(value)),
        }
    }

    /// The storage bit pattern of `value` rounded to this format.
    pub fn encode(self, value: f64) -> u64 {
        match self {
            FloatWidth::F64 => value.to_bits(),
            FloatWidth::F32 => u64::from((value as f32).to_bits()),
            _ => u64::from(encode_small(value, &self.format().expect("small format"))),
        }
    }

    pub fn decode(self, bits: u64) -> f64 {
        match self {
            FloatWidth::F64 => f64::from_bits(bits),
            FloatWidth::F32 => f64::from(f32::from_bits(bits as u32)),
            _ => decode_small(bits as u32, &self.format().expect("small format")),
        }
    }
}

fn encode_small(value: f64, format: &Format) -> u32 {
    let m = format.mantissa_bits;
    let e = format.exponent_bits;
    let sign = if value.is_sign_negative() { 1u32 << (e + m) } else { 0 };
    let all_ones_exponent = (1u32 << e) - 1;
    let nan = sign | (all_ones_exponent << m) | ((1 << m) - 1);
    if value.is_nan() {
        return nan & !(1 << (e + m));
    }
    let overflow = if format.has_infinity { sign | (all_ones_exponent << m) } else { nan };
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return sign;
    }
    if magnitude.is_infinite() {
        return overflow;
    }
    let bias = (1i32 << (e - 1)) - 1;
    let min_exponent = 1 - bias;
    let exponent = (((magnitude.to_bits() >> 52) & 0x7ff) as i32 - 1023).max(min_exponent);
    let quantum = 2f64.powi(exponent - m as i32);
    let mut scaled = (magnitude / quantum).round_ties_even() as u64;
    let mut exponent = exponent;
    if scaled >= 1 << (m + 1) {
        scaled >>= 1;
        exponent += 1;
    }
    let biased = if scaled < 1 << m { 0 } else { exponent + bias };
    let max_biased = if format.has_infinity { all_ones_exponent as i32 - 1 } else { all_ones_exponent as i32 };
    if biased > max_biased {
        return overflow;
    }
    let fraction = (scaled as u32) & ((1 << m) - 1);
    let bits = sign | ((biased as u32) << m) | fraction;
    if !format.has_infinity && (bits & !sign) == (nan & !sign) {
        return overflow;
    }
    bits
}

fn decode_small(bits: u32, format: &Format) -> f64 {
    let m = format.mantissa_bits;
    let e = format.exponent_bits;
    let negative = bits >> (e + m) & 1 == 1;
    let all_ones_exponent = (1u32 << e) - 1;
    let biased = (bits >> m) & all_ones_exponent;
    let fraction = bits & ((1 << m) - 1);
    let bias = (1i32 << (e - 1)) - 1;
    let magnitude = if biased == all_ones_exponent && format.has_infinity {
        if fraction == 0 { f64::INFINITY } else { f64::NAN }
    } else if biased == all_ones_exponent && fraction == (1 << m) - 1 {
        f64::NAN
    } else if biased == 0 {
        f64::from(fraction) * 2f64.powi(1 - bias - m as i32)
    } else {
        f64::from((1 << m) | fraction) * 2f64.powi(biased as i32 - bias - m as i32)
    };
    if negative { -magnitude } else { magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_matches_ieee_binary16() {
        assert_eq!(FloatWidth::F16.encode(1.0), 0x3c00);
        assert_eq!(FloatWidth::F16.encode(-2.0), 0xc000);
        assert_eq!(FloatWidth::F16.encode(65504.0), 0x7bff);
        assert_eq!(FloatWidth::F16.encode(65520.0), 0x7c00);
        assert_eq!(FloatWidth::F16.encode(5.960464477539063e-8), 0x0001);
        assert_eq!(FloatWidth::F16.round(0.1), 0.0999755859375);
    }

    #[test]
    fn bf16_keeps_the_f32_exponent_range() {
        assert_eq!(FloatWidth::BF16.encode(1.0), 0x3f80);
        assert_eq!(FloatWidth::BF16.round(3.0e38), 226.0 / 128.0 * 2f64.powi(127));
        assert_eq!(FloatWidth::BF16.round(1.00390625), 1.0);
        assert_eq!(FloatWidth::BF16.round(1.01171875), 1.015625);
    }

    #[test]
    fn narrowing_rounds_ties_to_even() {
        assert_eq!(FloatWidth::F16.round(1.0 + 2f64.powi(-11)), 1.0);
        assert_eq!(FloatWidth::F16.round(1.0 + 3.0 * 2f64.powi(-11)), 1.0 + 2f64.powi(-9));
    }

    #[test]
    fn fp8_e4m3_saturates_to_nan_and_e5m2_to_infinity() {
        assert_eq!(FloatWidth::F8E4M3.round(448.0), 448.0);
        assert!(FloatWidth::F8E4M3.round(500.0).is_nan());
        assert_eq!(FloatWidth::F8E5M2.round(57344.0), 57344.0);
        assert_eq!(FloatWidth::F8E5M2.round(1.0e6), f64::INFINITY);
        assert_eq!(FloatWidth::F8E4M3.round(0.3), 0.3125);
    }

    #[test]
    fn every_bit_pattern_round_trips() {
        for width in [FloatWidth::F8E4M3, FloatWidth::F8E5M2] {
            for bits in 0u64..256 {
                let value = width.decode(bits);
                if !value.is_nan() {
                    assert_eq!(width.encode(value), bits, "{width:?} {bits:#x} ({value})");
                }
            }
        }
        for bits in (0u64..65536).step_by(7) {
            for width in [FloatWidth::F16, FloatWidth::BF16] {
                let value = width.decode(bits);
                if !value.is_nan() {
                    assert_eq!(width.encode(value), bits, "{width:?} {bits:#x}");
                }
            }
        }
    }
}
