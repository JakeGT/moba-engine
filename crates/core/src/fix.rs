//! Deterministic Q32.32 fixed-point arithmetic.
//!
//! The simulation never uses `f32` for authoritative state: floating-point results can
//! differ between CPUs, compiler flags and platforms. Every value that feeds a gameplay
//! decision is a [`Fix`], which is an `i64` holding a signed 32.32 bit fixed-point number.
//!
//! Properties that matter for the simulation:
//! - addition/subtraction are exact unless they saturate;
//! - multiplication/division round toward negative infinity and saturate;
//! - [`Fix::sqrt`] is implemented with an integer Newton iteration;
//! - [`Fix::sin_cos`] uses CORDIC with compile-time constants (no libm, no tables built
//!   from floating point at runtime);
//! - every operation is branch-deterministic and safe to use on any target.
//!
//! Range is approximately `[-2^31, 2^31)` with a resolution of `2^-32`. MOBA maps are a
//! few hundred metres across, so that range is effectively unbounded for gameplay.

use core::fmt;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// Number of fractional bits in a [`Fix`].
pub const FRAC_BITS: u32 = 32;
/// `1.0` in fixed-point representation.
pub const ONE_BITS: i64 = 1 << FRAC_BITS;

/// Q32.32 fixed-point number.
///
/// All operators saturate instead of panicking. That keeps the simulation running even
/// if a designer enters an absurd value; tests and debug assertions catch bad data long
/// before it reaches production.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Fix(i64);

const PI: i64 = 13_493_037_705;
const HALF_PI: i64 = 6_746_518_852;
const TAU: i64 = 26_986_075_409;

/// `atan(2^-i)` in Q32.32. Precomputed constants keep the result identical on every
/// platform, including targets where `f64::atan` is a platform libm call.
const ATAN: [i64; 32] = [
    3_373_259_426,
    1_991_351_318,
    1_052_175_346,
    534_100_635,
    268_086_748,
    134_174_063,
    67_103_403,
    33_553_749,
    16_777_131,
    8_388_597,
    4_194_303,
    2_097_152,
    1_048_576,
    524_288,
    262_144,
    131_072,
    65_536,
    32_768,
    16_384,
    8_192,
    4_096,
    2_048,
    1_024,
    512,
    256,
    128,
    64,
    32,
    16,
    8,
    4,
    2,
];

/// `1 / CORDIC gain` (`0.6072529350088813`) in Q32.32.
const CORDIC_K: i64 = 2_608_131_496;

const MAX: i64 = i64::MAX;
const MIN: i64 = i64::MIN;

/// Integer square root of a `u128`, rounded down.
///
/// Newton iteration starting from a power of two guaranteed to be at or above the true
/// root, so the sequence is strictly decreasing until it lands. Exact for every `u128`.
const fn isqrt_u128(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let bits = 128 - n.leading_zeros();
    let mut x = 1u128 << bits.div_ceil(2);
    loop {
        let y = x.midpoint(n / x);
        if y >= x {
            return x;
        }
        x = y;
    }
}

impl Fix {
    /// Zero.
    pub const ZERO: Self = Self(0);
    /// One.
    pub const ONE: Self = Self(ONE_BITS);
    /// One half.
    pub const HALF: Self = Self(ONE_BITS / 2);
    /// The smallest representable value.
    pub const MIN: Self = Self(MIN);
    /// The largest representable value.
    pub const MAX: Self = Self(MAX);
    /// `1e-9`, the finest useful gameplay epsilon.
    pub const EPSILON: Self = Self(4);

    /// Creates a value from its raw bit representation.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: i64) -> Self {
        Self(bits)
    }

    /// Returns the raw bit representation.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> i64 {
        self.0
    }

    /// Converts an `i32` into fixed-point. Exact.
    #[inline]
    #[must_use]
    pub const fn from_i32(value: i32) -> Self {
        Self((value as i64) << FRAC_BITS)
    }

    /// Converts an `i64` into fixed-point, saturating if it does not fit.
    #[inline]
    #[must_use]
    pub const fn from_i64(value: i64) -> Self {
        Self(value.saturating_mul(ONE_BITS))
    }

    /// Converts to an `i64`, rounding toward zero.
    #[inline]
    #[must_use]
    pub const fn to_i64(self) -> i64 {
        self.0 / ONE_BITS
    }

    /// Converts to an `i32`, saturating.
    #[inline]
    #[must_use]
    pub const fn to_i32(self) -> i32 {
        let v = self.to_i64();
        if v > i32::MAX as i64 {
            i32::MAX
        } else if v < i32::MIN as i64 {
            i32::MIN
        } else {
            v as i32
        }
    }

    /// Converts an `f32` into fixed-point.
    ///
    /// This is a *non-authoritative* conversion: use it at the platform boundary (input,
    /// config, animation), never to derive simulation state from a float.
    #[allow(clippy::cast_possible_truncation)]
    #[must_use]
    pub fn from_f32(value: f32) -> Self {
        Self::from_f64(f64::from(value))
    }

    /// Converts an `f64` into fixed-point, saturating and mapping NaN to zero.
    #[allow(clippy::cast_possible_truncation)]
    #[must_use]
    pub fn from_f64(value: f64) -> Self {
        if value.is_nan() {
            return Self::ZERO;
        }
        let scaled = (value * ONE_BITS as f64).round();
        if scaled >= MAX as f64 {
            Self(MAX)
        } else if scaled <= MIN as f64 {
            Self(MIN)
        } else {
            Self(scaled as i64)
        }
    }

    /// Converts to `f32`. Only for presentation and debug code.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn to_f32(self) -> f32 {
        (self.0 as f64 / ONE_BITS as f64) as f32
    }

    /// Converts to `f64`. Only for presentation and debug code.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / ONE_BITS as f64
    }

    /// Absolute value, saturating at [`Fix::MAX`] for [`Fix::MIN`].
    #[inline]
    #[must_use]
    pub const fn abs(self) -> Self {
        Self(if self.0 == MIN { MAX } else { self.0.abs() })
    }

    /// Sign of the value: `-1`, `0` or `1`.
    #[inline]
    #[must_use]
    pub const fn signum(self) -> i32 {
        if self.0 > 0 {
            1
        } else if self.0 < 0 {
            -1
        } else {
            0
        }
    }

    /// Smaller of two values.
    #[inline]
    #[must_use]
    pub const fn min(self, other: Self) -> Self {
        if self.0 < other.0 { self } else { other }
    }

    /// Larger of two values.
    #[inline]
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        if self.0 > other.0 { self } else { other }
    }

    /// Clamps into `[min, max]`.
    #[inline]
    #[must_use]
    pub const fn clamp(self, min: Self, max: Self) -> Self {
        self.max(min).min(max)
    }

    /// Truncates toward negative infinity.
    #[inline]
    #[must_use]
    pub const fn floor(self) -> Self {
        Self(self.0 & !(ONE_BITS - 1))
    }

    /// Truncates toward positive infinity.
    #[inline]
    #[must_use]
    pub const fn ceil(self) -> Self {
        let floor = self.floor();
        if floor.0 == self.0 {
            floor
        } else {
            Self(floor.0 + ONE_BITS)
        }
    }

    /// Rounds to the nearest integer, halfway away from zero.
    #[inline]
    #[must_use]
    pub const fn round(self) -> Self {
        if self.0 >= 0 {
            Self((self.0 + ONE_BITS / 2) & !(ONE_BITS - 1))
        } else {
            Self(-(((-self.0) + ONE_BITS / 2) & !(ONE_BITS - 1)))
        }
    }

    /// Truncates toward zero.
    #[inline]
    #[must_use]
    pub const fn trunc(self) -> Self {
        Self(self.0 / ONE_BITS * ONE_BITS)
    }

    /// Fractional part in `(-1, 1)`.
    #[inline]
    #[must_use]
    pub const fn fract(self) -> Self {
        Self(self.0 % ONE_BITS)
    }

    /// Saturating addition.
    #[inline]
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction.
    #[inline]
    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// Saturating multiplication.
    #[inline]
    #[must_use]
    pub const fn saturating_mul(self, rhs: Self) -> Self {
        let product = (self.0 as i128 * rhs.0 as i128) >> FRAC_BITS;
        if product > MAX as i128 {
            Self(MAX)
        } else if product < MIN as i128 {
            Self(MIN)
        } else {
            Self(product as i64)
        }
    }

    /// Saturating division.
    ///
    /// Division by zero saturates: positive or negative infinity depending on signs;
    /// `0 / 0` is zero.
    #[inline]
    #[must_use]
    pub const fn saturating_div(self, rhs: Self) -> Self {
        if rhs.0 == 0 {
            return if self.0 > 0 {
                Self(MAX)
            } else if self.0 < 0 {
                Self(MIN)
            } else {
                Self(0)
            };
        }
        let numerator = (self.0 as i128) << FRAC_BITS;
        let denominator = rhs.0 as i128;
        let mut quotient = numerator / denominator;
        if numerator % denominator != 0 && (numerator < 0) != (denominator < 0) {
            quotient -= 1;
        }
        if quotient > MAX as i128 {
            Self(MAX)
        } else if quotient < MIN as i128 {
            Self(MIN)
        } else {
            Self(quotient as i64)
        }
    }

    /// Multiplies by an integer without going through 128-bit arithmetic.
    #[inline]
    #[must_use]
    pub const fn mul_i32(self, rhs: i32) -> Self {
        Self(self.0.saturating_mul(rhs as i64))
    }

    /// Linear interpolation: `self + (other - self) * t`.
    #[inline]
    #[must_use]
    pub const fn lerp(self, other: Self, t: Self) -> Self {
        self.saturating_add(other.saturating_sub(self).saturating_mul(t))
    }

    /// Square root, rounded down. Negative inputs return zero.
    #[must_use]
    pub const fn sqrt(self) -> Self {
        if self.0 <= 0 {
            return Self::ZERO;
        }
        // sqrt(x / 2^32) * 2^32 == sqrt(x * 2^32), all in integer space.
        let scaled = (self.0 as u128) << FRAC_BITS;
        Self(isqrt_u128(scaled) as i64)
    }

    /// Returns `(sin, cos)` of an angle in radians.
    ///
    /// The angle is reduced modulo `2*pi` and evaluated with 32 CORDIC iterations,
    /// giving roughly 9 decimal digits of accuracy while staying entirely in integer
    /// arithmetic.
    #[must_use]
    pub const fn sin_cos(self) -> (Self, Self) {
        let mut z = self.0 % TAU;
        if z > PI {
            z -= TAU;
        } else if z < -PI {
            z += TAU;
        }
        // CORDIC converges for |z| <= ~1.743 rad, so fold into [-pi/2, pi/2].
        let mut negate_cos = false;
        if z > HALF_PI {
            z = PI - z;
            negate_cos = true;
        } else if z < -HALF_PI {
            z = -PI - z;
            negate_cos = true;
        }

        let mut x = CORDIC_K;
        let mut y = 0i64;
        let mut i = 0;
        while i < 32 {
            let x_shr = x >> i;
            let y_shr = y >> i;
            if z >= 0 {
                x -= y_shr;
                y += x_shr;
                z -= ATAN[i];
            } else {
                x += y_shr;
                y -= x_shr;
                z += ATAN[i];
            }
            i += 1;
        }
        if negate_cos {
            x = -x;
        }
        (Self(y), Self(x))
    }

    /// Sine of an angle in radians. See [`Fix::sin_cos`].
    #[inline]
    #[must_use]
    pub const fn sin(self) -> Self {
        self.sin_cos().0
    }

    /// Cosine of an angle in radians. See [`Fix::sin_cos`].
    #[inline]
    #[must_use]
    pub const fn cos(self) -> Self {
        self.sin_cos().1
    }
}

impl Add for Fix {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        self.saturating_add(rhs)
    }
}

impl Sub for Fix {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        self.saturating_sub(rhs)
    }
}

impl Mul for Fix {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        self.saturating_mul(rhs)
    }
}

impl Div for Fix {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        self.saturating_div(rhs)
    }
}

impl Neg for Fix {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        if self.0 == MIN {
            Self(MAX)
        } else {
            Self(-self.0)
        }
    }
}

impl AddAssign for Fix {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Fix {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign for Fix {
    #[inline]
    fn mul_assign(&mut self, rhs: Self) {
        *self = *self * rhs;
    }
}

impl DivAssign for Fix {
    #[inline]
    fn div_assign(&mut self, rhs: Self) {
        *self = *self / rhs;
    }
}

impl From<i32> for Fix {
    #[inline]
    fn from(value: i32) -> Self {
        Self::from_i32(value)
    }
}

impl From<Fix> for f32 {
    #[inline]
    fn from(value: Fix) -> Self {
        value.to_f32()
    }
}

impl From<Fix> for f64 {
    #[inline]
    fn from(value: Fix) -> Self {
        value.to_f64()
    }
}

impl fmt::Debug for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fix({:.9})", self.to_f64())
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.9}", self.to_f64())
    }
}

/// A 2D point/vector in fixed-point space: the authoritative MOBA plane.
///
/// The game uses a right-handed XZ ground plane with Y up on the wire/GPU side; the
/// simulation only needs the ground plane, so it stores `(x, z)` as `(x, y)` here.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug, Hash)]
pub struct FixVec2 {
    /// X coordinate.
    pub x: Fix,
    /// Z (ground-plane second) coordinate.
    pub y: Fix,
}

impl FixVec2 {
    /// The zero vector.
    pub const ZERO: Self = Self::splat(Fix::ZERO);

    /// Creates a vector from components.
    #[inline]
    #[must_use]
    pub const fn new(x: Fix, y: Fix) -> Self {
        Self { x, y }
    }

    /// Creates a vector with both components set to `value`.
    #[inline]
    #[must_use]
    pub const fn splat(value: Fix) -> Self {
        Self { x: value, y: value }
    }

    /// Creates a vector from integer coordinates.
    #[inline]
    #[must_use]
    pub const fn from_ints(x: i32, y: i32) -> Self {
        Self {
            x: Fix::from_i32(x),
            y: Fix::from_i32(y),
        }
    }

    /// Converts from a presentation-space `Vec2`.
    #[inline]
    #[must_use]
    pub fn from_vec2(v: glam::Vec2) -> Self {
        Self::new(Fix::from_f32(v.x), Fix::from_f32(v.y))
    }

    /// Converts to a presentation-space `Vec2`.
    #[inline]
    #[must_use]
    pub fn to_vec2(self) -> glam::Vec2 {
        glam::Vec2::new(self.x.to_f32(), self.y.to_f32())
    }

    /// Dot product.
    #[inline]
    #[must_use]
    pub fn dot(self, rhs: Self) -> Fix {
        self.x * rhs.x + self.y * rhs.y
    }

    /// Z component of the 2D cross product (positive when `rhs` is counter-clockwise).
    #[inline]
    #[must_use]
    pub fn cross(self, rhs: Self) -> Fix {
        self.x * rhs.y - self.y * rhs.x
    }

    /// Squared length. Avoids the square root in comparisons.
    #[inline]
    #[must_use]
    pub fn length_sq(self) -> Fix {
        self.dot(self)
    }

    /// Euclidean length.
    #[inline]
    #[must_use]
    pub fn length(self) -> Fix {
        self.length_sq().sqrt()
    }

    /// Squared distance to another point.
    #[inline]
    #[must_use]
    pub fn distance_sq(self, rhs: Self) -> Fix {
        (rhs - self).length_sq()
    }

    /// Distance to another point.
    #[inline]
    #[must_use]
    pub fn distance(self, rhs: Self) -> Fix {
        (rhs - self).length()
    }

    /// Returns a unit vector, or [`FixVec2::ZERO`] if the vector is zero.
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let len = self.length();
        if len == Fix::ZERO {
            Self::ZERO
        } else {
            self / len
        }
    }

    /// Returns a unit vector. The caller must guarantee a non-zero vector.
    ///
    /// # Panics
    /// Panics in debug builds if the vector is zero; release builds return zero.
    #[must_use]
    pub fn normalize(self) -> Self {
        debug_assert!(self != Self::ZERO, "normalize() called on a zero vector");
        self.normalize_or_zero()
    }

    /// Component-wise minimum.
    #[inline]
    #[must_use]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y))
    }

    /// Component-wise maximum.
    #[inline]
    #[must_use]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y))
    }

    /// Component-wise absolute value.
    #[inline]
    #[must_use]
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs())
    }

    /// Linear interpolation from `self` to `rhs` by `t`.
    #[inline]
    #[must_use]
    pub fn lerp(self, rhs: Self, t: Fix) -> Self {
        Self::new(self.x.lerp(rhs.x, t), self.y.lerp(rhs.y, t))
    }

    /// Moves `self` toward `target` by at most `max_delta` units.
    #[must_use]
    pub fn move_towards(self, target: Self, max_delta: Fix) -> Self {
        let delta = target - self;
        let distance = delta.length();
        if distance <= max_delta || distance == Fix::ZERO {
            target
        } else {
            self + delta / distance * max_delta
        }
    }

    /// Left-hand perpendicular vector.
    #[inline]
    #[must_use]
    pub fn perpendicular(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Rotates by an angle, given its sine and cosine.
    #[inline]
    #[must_use]
    pub fn rotate(self, sin: Fix, cos: Fix) -> Self {
        Self::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }
}

impl Add for FixVec2 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for FixVec2 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<Fix> for FixVec2 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Fix) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<Fix> for FixVec2 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Fix) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for FixVec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl AddAssign for FixVec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for FixVec2 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: [f64; 11] = [
        -1234.5678, -100.0, -1.5, -1.0, -0.25, 0.0, 0.25, 1.0, 1.5, 100.0, 1234.5678,
    ];

    fn approx_bits(a: Fix, b: Fix, tolerance: i64) -> bool {
        (a.to_bits() - b.to_bits()).abs() <= tolerance
    }

    /// Relative comparison for operations where a fixed absolute tolerance is wrong at
    /// large magnitudes (a 1e-6 relative error is ~160 bit steps on a 1.5M value).
    fn approx_relative(a: Fix, b: Fix, per_million: i128) -> bool {
        let diff = (i128::from(a.to_bits()) - i128::from(b.to_bits())).abs();
        let magnitude = i128::from(b.to_bits().unsigned_abs()).max(1);
        diff <= 8 || diff * 1_000_000 <= magnitude * per_million
    }

    #[test]
    fn conversions_round_trip() {
        for v in VALUES {
            let f = Fix::from_f64(v);
            assert!((f.to_f64() - v).abs() < 1e-8, "{v} -> {f:?}");
        }
        assert_eq!(Fix::from_i32(7).to_i64(), 7);
        assert_eq!(Fix::from_i32(-7).to_i64(), -7);
        assert_eq!(Fix::from_f64(f64::NAN), Fix::ZERO);
        assert_eq!(Fix::from_f64(f64::INFINITY), Fix::MAX);
        assert_eq!(Fix::from_f64(f64::NEG_INFINITY), Fix::MIN);
    }

    #[test]
    fn arithmetic_matches_float_within_one_ulp() {
        for &a in &VALUES {
            for &b in &VALUES {
                let fa = Fix::from_f64(a);
                let fb = Fix::from_f64(b);
                let expect = |x: f64| Fix::from_f64(x);
                assert!(approx_bits(fa + fb, expect(a + b), 2), "{a}+{b}");
                assert!(approx_bits(fa - fb, expect(a - b), 2), "{a}-{b}");
                assert!(
                    approx_relative(fa * fb, expect(a * b), 1),
                    "{a}*{b}: {:?} vs {:?}",
                    fa * fb,
                    expect(a * b)
                );
                if b != 0.0 {
                    assert!(
                        approx_relative(fa / fb, expect(a / b), 1),
                        "{a}/{b}: {:?} vs {:?}",
                        fa / fb,
                        expect(a / b)
                    );
                }
            }
        }
    }

    #[test]
    fn arithmetic_saturates() {
        assert_eq!(Fix::MAX + Fix::ONE, Fix::MAX);
        assert_eq!(Fix::MIN - Fix::ONE, Fix::MIN);
        assert_eq!(Fix::MAX * Fix::MAX, Fix::MAX);
        assert_eq!(Fix::MIN * Fix::MIN, Fix::MAX);
        assert_eq!(Fix::MIN / Fix::ZERO, Fix::MIN);
        assert_eq!(Fix::MAX / Fix::ZERO, Fix::MAX);
        assert_eq!(Fix::ZERO / Fix::ZERO, Fix::ZERO);
        assert_eq!(-Fix::MIN, Fix::MAX);
        assert_eq!(Fix::MIN.abs(), Fix::MAX);
    }

    #[test]
    fn rounding_modes() {
        let cases = [
            (1.25, 1.0, 2.0, 1.0),
            (1.5, 1.0, 2.0, 2.0),
            (-1.25, -2.0, -1.0, -1.0),
            (-1.5, -2.0, -1.0, -2.0),
            (0.0, 0.0, 0.0, 0.0),
        ];
        for (input, floor, ceil, round) in cases {
            let f = Fix::from_f64(input);
            assert_eq!(f.floor(), Fix::from_f64(floor), "floor {input}");
            assert_eq!(f.ceil(), Fix::from_f64(ceil), "ceil {input}");
            assert_eq!(f.round(), Fix::from_f64(round), "round {input}");
            assert_eq!(f.trunc(), Fix::from_f64(input.trunc()), "trunc {input}");
        }
    }

    #[test]
    fn sqrt_is_accurate_and_monotonic() {
        for i in 0..=2000i32 {
            let x = i * i;
            assert_eq!(Fix::from_i32(x).sqrt(), Fix::from_i32(i), "perfect {x}");
        }
        let mut previous = Fix::ZERO;
        for i in 0..5000 {
            let value = Fix::from_f64(f64::from(i) * 0.37);
            let root = value.sqrt();
            assert!((root.to_f64() - value.to_f64().sqrt()).abs() < 2e-8);
            assert!(root >= previous, "monotonic at {i}");
            previous = root;
        }
        assert_eq!(Fix::from_i32(-4).sqrt(), Fix::ZERO);
    }

    #[test]
    fn sin_cos_known_values() {
        let quarter = Fix::from_f64(core::f64::consts::FRAC_PI_2);
        let (sin, cos) = quarter.sin_cos();
        assert!(approx_bits(sin, Fix::ONE, 32), "sin(pi/2) = {sin:?}");
        assert!(cos.abs().to_bits() <= 32, "cos(pi/2) = {cos:?}");

        let pi = Fix::from_f64(core::f64::consts::PI);
        let (sin, cos) = pi.sin_cos();
        assert!(sin.abs().to_bits() <= 64, "sin(pi) = {sin:?}");
        assert!(approx_bits(cos, -Fix::ONE, 64), "cos(pi) = {cos:?}");

        for &angle in &[-6.0, -3.0, -1.0, -0.5, 0.0, 0.3, 1.0, 2.0, 3.0, 7.5] {
            let (sin, cos) = Fix::from_f64(angle).sin_cos();
            let expected_sin = Fix::from_f64(angle.sin());
            let expected_cos = Fix::from_f64(angle.cos());
            assert!(approx_bits(sin, expected_sin, 64), "sin({angle}) = {sin:?}");
            assert!(approx_bits(cos, expected_cos, 64), "cos({angle}) = {cos:?}");
            let identity = sin * sin + cos * cos;
            assert!(
                approx_bits(identity, Fix::ONE, 1 << 12),
                "identity({angle}) = {identity:?}"
            );
        }
    }

    #[test]
    fn sin_cos_is_deterministic_golden() {
        // Golden bits lock the implementation. If these change, replays recorded by an
        // older build will not reproduce: that is a deliberate, reviewed decision.
        let (sin, cos) = Fix::from_f64(0.5).sin_cos();
        assert_eq!(sin.to_bits(), 2_059_117_009);
        assert_eq!(cos.to_bits(), 3_769_188_408);
    }

    #[test]
    fn fix_vec2_geometry() {
        let a = FixVec2::from_ints(3, 4);
        assert_eq!(a.length(), Fix::from_i32(5));
        assert!((a.normalize().length() - Fix::ONE).abs() <= Fix::from_f64(1e-6));
        assert_eq!(a.dot(FixVec2::from_ints(1, 0)), Fix::from_i32(3));
        assert_eq!(a.cross(FixVec2::from_ints(0, 1)), Fix::from_i32(3));
        assert_eq!(a.perpendicular(), FixVec2::from_ints(-4, 3));
        assert_eq!(FixVec2::ZERO.normalize_or_zero(), FixVec2::ZERO);
        assert_eq!(
            FixVec2::from_ints(0, 0).move_towards(FixVec2::from_ints(10, 0), Fix::ONE),
            FixVec2::from_ints(1, 0)
        );
        let (sin, cos) = Fix::from_f64(core::f64::consts::FRAC_PI_2).sin_cos();
        let rotated = FixVec2::from_ints(1, 0).rotate(sin, cos);
        assert!(rotated.distance(FixVec2::from_ints(0, 1)) < Fix::from_f64(1e-6));
    }

    #[test]
    fn division_floors_for_all_signs() {
        let half = Fix::from_bits(Fix::ONE.0 / 2);
        assert_eq!(Fix::from_bits(-1) / half, Fix::from_bits(-2));
        assert_eq!(Fix::from_bits(-3) / Fix::ONE, Fix::from_bits(-3));
        assert_eq!(Fix::from_bits(-1) * half, Fix::from_bits(-1));
        assert_eq!(Fix::from_bits(1) / half, Fix::from_bits(2));
    }
}
