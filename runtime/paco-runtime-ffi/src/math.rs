//! Float math the compiled code calls: `f64` in and out, from the pure-Rust
//! `libm`, so every link mode and the compile-time evaluator agree to the bit.

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_exp(x: f64) -> f64 {
    libm::exp(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_ln(x: f64) -> f64 {
    libm::log(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_sin(x: f64) -> f64 {
    libm::sin(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_cos(x: f64) -> f64 {
    libm::cos(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_tanh(x: f64) -> f64 {
    libm::tanh(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_math_powf(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
