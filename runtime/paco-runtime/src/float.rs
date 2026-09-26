/// Formats a float with the fewest significant digits that read back to the
/// same value in its own format; `round` maps an `f64` to the nearest value of
/// that format. Plain decimal for exponents in `-7 < e < 21`, `1e21` style
/// outside it.
pub fn format_float(value: f64, round: impl Fn(f64) -> f64) -> String {
    if !value.is_finite() || value == 0.0 {
        return value.to_string();
    }
    let shortest = (0..17)
        .flat_map(|precision| {
            let nearest = format!("{value:.precision$e}");
            let (mantissa, exponent) = nearest.split_once('e').expect("`{:e}` has an exponent");
            let digits: i64 = mantissa.replace('.', "").parse().expect("integer digits");
            let exponent = exponent.parse::<i32>().expect("integer exponent") - precision as i32;
            [digits, digits - 1, digits + 1].map(|digits| format!("{digits}e{exponent}").parse::<f64>().expect("decimal"))
        })
        .find(|candidate| round(*candidate).to_bits() == value.to_bits())
        .unwrap_or(value);
    let scientific = format!("{shortest:e}");
    let exponent: i32 = scientific.split_once('e').expect("`{:e}` has an exponent").1.parse().expect("integer exponent");
    if (-7 < exponent) && (exponent < 21) { shortest.to_string() } else { scientific }
}

/// Formats `value` with the shortest digits for the format `code` names: a
/// small-format code, [`FLOAT_CODE_F32`] or [`FLOAT_CODE_F64`].
pub fn format_float_code(value: f64, code: i32) -> String {
    format_float(value, |x| match code {
        FLOAT_CODE_F64 => x,
        FLOAT_CODE_F32 => f64::from(x as f32),
        small => float_to_f64(float_from_f64(x, small), small),
    })
}

pub const FLOAT_CODE_F32: i32 = 4;
pub const FLOAT_CODE_F64: i32 = 5;

/// Exponent bits, mantissa bits and whether the format has infinities, for
/// the codes f16, bf16, f8e4m3, f8e5m2.
fn float_format(code: i32) -> (i32, i32, bool) {
    [(5, 10, true), (8, 7, true), (4, 3, false), (5, 2, true)][code as usize]
}

/// Widens the bits of a small float (f16, bf16, f8e4m3, f8e5m2: codes 0-3).
pub fn float_to_f64(bits: u32, code: i32) -> f64 {
    let (e, m, has_infinity) = float_format(code);
    let negative = (bits >> (e + m)) & 1 == 1;
    let all_ones = (1u32 << e) - 1;
    let biased = (bits >> m) & all_ones;
    let fraction = bits & ((1u32 << m) - 1);
    let bias = (1 << (e - 1)) - 1;
    let magnitude = if biased == all_ones && has_infinity {
        if fraction == 0 { f64::INFINITY } else { f64::NAN }
    } else if biased == all_ones && fraction == (1u32 << m) - 1 {
        f64::NAN
    } else if biased == 0 {
        fraction as f64 * 2f64.powi(1 - bias - m)
    } else {
        ((1u32 << m) | fraction) as f64 * 2f64.powi(biased as i32 - bias - m)
    };
    if negative { -magnitude } else { magnitude }
}

/// Rounds `value` to the nearest small float of `code`, returning its bits.
pub fn float_from_f64(value: f64, code: i32) -> u32 {
    let (e, m, has_infinity) = float_format(code);
    let sign = if value.is_sign_negative() { 1u32 << (e + m) } else { 0 };
    let all_ones = (1u32 << e) - 1;
    let nan = sign | (all_ones << m) | ((1u32 << m) - 1);
    if value.is_nan() {
        return nan & !(1u32 << (e + m));
    }
    let overflow = if has_infinity { sign | (all_ones << m) } else { nan };
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return sign;
    }
    if magnitude.is_infinite() {
        return overflow;
    }
    let bias = (1 << (e - 1)) - 1;
    let mut exponent = (((magnitude.to_bits() >> 52) & 0x7ff) as i32 - 1023).max(1 - bias);
    let mut scaled = (magnitude * 2f64.powi(m - exponent)).round_ties_even() as u64;
    if scaled >= 1u64 << (m + 1) {
        scaled >>= 1;
        exponent += 1;
    }
    let biased = if scaled < 1u64 << m { 0 } else { exponent + bias };
    let max_biased = if has_infinity { all_ones as i32 - 1 } else { all_ones as i32 };
    if biased > max_biased {
        return overflow;
    }
    let bits = sign | ((biased as u32) << m) | (scaled as u32 & ((1u32 << m) - 1));
    if !has_infinity && (bits & !sign) == (nan & !sign) {
        return overflow;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::format_float;

    fn f64(value: f64) -> String {
        format_float(value, |x| x)
    }

    #[test]
    fn f64_uses_the_shortest_round_trip_digits() {
        let cases = [
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.0f64.cos(), "0.5403023058681398"),
            (1e21, "1e21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (1.5e-6, "0.0000015"),
            (-0.0, "-0"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "NaN"),
            (9007199254740993.0, "9007199254740992"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e308"),
        ];
        for (value, expected) in cases {
            assert_eq!(f64(value), expected);
        }
    }

    #[test]
    fn narrow_formats_use_their_own_precision() {
        let f32 = |value: f32| format_float(f64::from(value), |x| f64::from(x as f32));
        assert_eq!(f32(0.1), "0.1");
        assert_eq!(f32(16777216.0), "16777216");
        assert_eq!(f32(1.2621775e-29), "1.2621775e-29");
    }
}
