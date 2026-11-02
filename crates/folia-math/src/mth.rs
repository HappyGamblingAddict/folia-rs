//! Selected helpers ported from `net.minecraft.util.Mth`.
//!
//! The functions here keep the JVM integer semantics (wrapping arithmetic and
//! floor-based division/modulo) and the Java evaluation order for floats.

/// A numeric type supported by [`square`] and [`cube`].
///
/// This mirrors the JVM overloads for `int`, `long`, `float`, and `double`.
/// Integer multiplication wraps on overflow, matching Java.
pub trait MthNumber: Copy {
    fn square(self) -> Self;
    fn cube(self) -> Self;
}

macro_rules! impl_mth_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl MthNumber for $ty {
                #[inline]
                fn square(self) -> Self {
                    self.wrapping_mul(self)
                }

                #[inline]
                fn cube(self) -> Self {
                    self.wrapping_mul(self).wrapping_mul(self)
                }
            }
        )*
    };
}

macro_rules! impl_mth_float {
    ($($ty:ty),* $(,)?) => {
        $(
            impl MthNumber for $ty {
                #[inline]
                fn square(self) -> Self {
                    self * self
                }

                #[inline]
                fn cube(self) -> Self {
                    self * self * self
                }
            }
        )*
    };
}

impl_mth_integer!(i32, i64);
impl_mth_float!(f32, f64);

/// Clamps `value` to the inclusive range `min..=max`.
///
/// Uses the same operation order as Java's `Mth.clamp`:
/// `Math.min(Math.max(value, min), max)`.
#[inline]
pub fn clamp<T: PartialOrd>(value: T, min: T, max: T) -> T {
    let upper = if min > value { min } else { value };
    if max < upper { max } else { upper }
}

/// Linear interpolation, matching `Mth.lerp(double delta, double start, double end)`.
///
/// The result is `start + delta * (end - start)`, including extrapolation for
/// deltas outside `0.0..=1.0`.
#[inline]
pub fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// Single-precision variant of [`lerp`].
#[inline]
pub fn lerp_f32(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// Java's `Mth.floor(double)`.
///
/// Casting first and then adjusting is significant at the infinities:
/// `-inf` casts to `i32::MIN` and then wraps to `i32::MAX`, exactly as the
/// JVM implementation does.
#[inline]
pub(crate) fn floor(value: f64) -> i32 {
    let truncated = value as i32;
    if value < truncated as f64 {
        truncated.wrapping_sub(1)
    } else {
        truncated
    }
}

/// Integer floor division, matching Java's `Math.floorDiv(int, int)`.
///
/// Unlike Rust's `/`, this rounds towards negative infinity:
/// `floor_div(-7, 3) == -3`. Division by zero panics, as in Java.
#[inline]
pub fn floor_div(dividend: i32, divisor: i32) -> i32 {
    let quotient = dividend.wrapping_div(divisor);
    let remainder = dividend.wrapping_rem(divisor);
    if remainder != 0 && ((dividend < 0) != (divisor < 0)) {
        quotient - 1
    } else {
        quotient
    }
}

/// Integer ceiling division: `-floor_div(-dividend, divisor)`.
///
/// This is the exact expression used by Java's `Mth.positiveCeilDiv`, and it
/// also behaves as ceiling division for negative divisors. Division by zero
/// panics.
#[inline]
pub fn ceil_div(dividend: i32, divisor: i32) -> i32 {
    floor_div(dividend.wrapping_neg(), divisor).wrapping_neg()
}

/// Alias for [`ceil_div`], named after Java's `Mth.positiveCeilDiv`.
#[inline]
pub fn positive_ceil_div(dividend: i32, divisor: i32) -> i32 {
    ceil_div(dividend, divisor)
}

/// Floor modulo, matching Java's `Math.floorMod(int, int)`.
///
/// The result has the sign of the divisor: `positive_modulo(-1, 3) == 2` and
/// `positive_modulo(1, -3) == -2`. A zero divisor panics, as in Java.
#[inline]
pub fn positive_modulo(value: i32, modulus: i32) -> i32 {
    let remainder = value.wrapping_rem(modulus);
    if remainder != 0 && ((remainder < 0) != (modulus < 0)) {
        remainder + modulus
    } else {
        remainder
    }
}

/// Floating-point variant of `positive_modulo(double, double)`.
///
/// Java implements this as `(numerator % denominator + denominator) % denominator`.
#[inline]
pub fn positive_modulo_f64(numerator: f64, denominator: f64) -> f64 {
    (numerator % denominator + denominator) % denominator
}

/// Floating-point variant of `positive_modulo(float, float)`.
#[inline]
pub fn positive_modulo_f32(numerator: f32, denominator: f32) -> f32 {
    (numerator % denominator + denominator) % denominator
}

/// Squares `value`, preserving Java overflow semantics for integers.
#[inline]
pub fn square<T: MthNumber>(value: T) -> T {
    value.square()
}

/// Cubes `value`, preserving Java overflow semantics for integers.
#[inline]
pub fn cube<T: MthNumber>(value: T) -> T {
    value.cube()
}

/// Triangle wave matching Java's `Mth.triangleWave(float input, float period)`.
#[inline]
pub fn triangle_wave(input: f32, period: f32) -> f32 {
    ((input % period - period * 0.5).abs() - period * 0.25) / (period * 0.25)
}

/// Alias for [`triangle_wave`].
///
/// The decompiled `Mth` method is named `triangleWave`; `triangle` is provided
/// as the short, conventional name requested by this crate's API.
#[inline]
pub fn triangle(input: f32, period: f32) -> f32 {
    triangle_wave(input, period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_ints_and_floats() {
        assert_eq!(clamp(5, 0, 10), 5);
        assert_eq!(clamp(-5, 0, 10), 0);
        assert_eq!(clamp(15, 0, 10), 10);
        assert_eq!(clamp(0, 0, 10), 0);
        assert_eq!(clamp(10, 0, 10), 10);

        assert_eq!(clamp(1.5, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-0.5, 0.0, 1.0), 0.0);
        assert!(clamp(f64::NAN, 0.0, 1.0).is_nan());

        // Java's Math.min(Math.max(value, min), max) order also defines the
        // result for inverted bounds.
        assert_eq!(clamp(5, 10, 0), 0);
        assert_eq!(clamp(-5, 10, 0), 0);
        assert_eq!(clamp(5, 0, -10), -10);
    }

    #[test]
    fn floor_matches_java_including_infinities() {
        assert_eq!(floor(2.9), 2);
        assert_eq!(floor(-2.1), -3);
        assert_eq!(floor(-0.0), 0);
        assert_eq!(floor(f64::NAN), 0);
        assert_eq!(floor(f64::INFINITY), i32::MAX);
        assert_eq!(floor(f64::NEG_INFINITY), i32::MAX);
    }

    #[test]
    fn lerp_matches_java_formula() {
        assert_eq!(lerp(0.0, 2.0, 6.0), 2.0);
        assert_eq!(lerp(0.5, 2.0, 6.0), 4.0);
        assert_eq!(lerp(1.0, 2.0, 6.0), 6.0);
        assert_eq!(lerp(0.25, -4.0, 4.0), -2.0);
        assert_eq!(lerp(2.0, 1.0, 3.0), 5.0);

        assert_eq!(lerp_f32(0.5, 2.0, 6.0), 4.0);
    }

    #[test]
    fn floor_div_handles_all_sign_combinations() {
        assert_eq!(floor_div(7, 3), 2);
        assert_eq!(floor_div(-7, 3), -3);
        assert_eq!(floor_div(7, -3), -3);
        assert_eq!(floor_div(-7, -3), 2);
        assert_eq!(floor_div(9, 3), 3);
        assert_eq!(floor_div(-9, 3), -3);
        assert_eq!(floor_div(0, 5), 0);
    }

    #[test]
    fn floor_div_wraps_like_java_min_value() {
        assert_eq!(floor_div(i32::MIN, -1), i32::MIN);
        assert_eq!(floor_div(i32::MIN, 1), i32::MIN);
        assert_eq!(floor_div(i32::MAX, 1), i32::MAX);
    }

    #[test]
    fn ceil_div_handles_all_sign_combinations() {
        assert_eq!(ceil_div(7, 3), 3);
        assert_eq!(ceil_div(-7, 3), -2);
        assert_eq!(ceil_div(7, -3), -2);
        assert_eq!(ceil_div(-7, -3), 3);
        assert_eq!(ceil_div(9, 3), 3);
        assert_eq!(ceil_div(-9, 3), -3);
        assert_eq!(positive_ceil_div(9, 8), 2);
        assert_eq!(ceil_div(i32::MIN, 1), i32::MIN);
    }

    #[test]
    fn positive_modulo_uses_divisor_sign() {
        assert_eq!(positive_modulo(5, 3), 2);
        assert_eq!(positive_modulo(-1, 3), 2);
        assert_eq!(positive_modulo(-4, 3), 2);
        assert_eq!(positive_modulo(1, -3), -2);
        assert_eq!(positive_modulo(-1, -3), -1);
        assert_eq!(positive_modulo(9, 3), 0);
        assert_eq!(positive_modulo(-9, 3), 0);
        assert_eq!(positive_modulo(0, 7), 0);
    }

    #[test]
    fn positive_modulo_float_variants() {
        assert_eq!(positive_modulo_f64(-1.5, 1.0), 0.5);
        assert_eq!(positive_modulo_f32(-1.5, 1.0), 0.5);
    }

    #[test]
    fn square_and_cube_match_java_for_ints_and_floats() {
        assert_eq!(square(7), 49);
        assert_eq!(square(-7), 49);
        assert_eq!(cube(3), 27);
        assert_eq!(cube(-3), -27);
        assert_eq!(square(1.5), 2.25);
        assert_eq!(cube(1.5), 3.375);
        assert_eq!(square(2_i64), 4_i64);
        assert_eq!(cube(-2_i64), -8_i64);

        // Java integer overflow wraps rather than panicking.
        assert_eq!(square(i32::MAX), 1);
        assert_eq!(cube(i32::MAX), i32::MAX);
    }

    #[test]
    fn triangle_wave_peaks_and_zero_crossings() {
        assert_eq!(triangle(0.0, 1.0), 1.0);
        assert_eq!(triangle(0.25, 1.0), 0.0);
        assert_eq!(triangle(0.5, 1.0), -1.0);
        assert_eq!(triangle(0.75, 1.0), 0.0);
        assert_eq!(triangle(1.0, 1.0), 1.0);
        assert_eq!(triangle_wave(0.0, 1.0), 1.0);
        assert_eq!(triangle_wave(0.0, 4.0), 1.0);
        assert_eq!(triangle_wave(1.0, 4.0), 0.0);
        assert_eq!(triangle_wave(2.0, 4.0), -1.0);
        assert_eq!(triangle_wave(3.0, 4.0), 0.0);
    }
}
