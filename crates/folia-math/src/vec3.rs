//! Port of `net.minecraft.world.phys.Vec3`.

use std::ops::{Add, Mul, Neg, Sub};

use crate::mth;

/// A three-dimensional double-precision vector, mirroring `Vec3`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const X_AXIS: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    pub const Y_AXIS: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const Z_AXIS: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    /// Java's `Vec3.normalize()` treats lengths below `1.0E-5F` as zero.
    ///
    /// The constant is deliberately written as an `f32` literal converted to
    /// `f64`, matching the JVM promotion of the Java `float` constant.
    const NORMALIZE_EPSILON: f64 = 1.0e-5_f32 as f64;

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }

    #[inline]
    pub const fn x(self) -> f64 {
        self.x
    }

    #[inline]
    pub const fn y(self) -> f64 {
        self.y
    }

    #[inline]
    pub const fn z(self) -> f64 {
        self.z
    }

    #[inline]
    pub const fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    #[inline]
    pub const fn subtract(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    #[inline]
    pub const fn scale(self, factor: f64) -> Vec3 {
        Vec3::new(self.x * factor, self.y * factor, self.z * factor)
    }

    #[inline]
    pub const fn dot(self, other: Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    #[inline]
    pub const fn cross(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.length_sqr().sqrt()
    }

    #[inline]
    pub const fn length_sqr(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[inline]
    pub fn horizontal_distance(self) -> f64 {
        self.horizontal_distance_sqr().sqrt()
    }

    #[inline]
    pub const fn horizontal_distance_sqr(self) -> f64 {
        self.x * self.x + self.z * self.z
    }

    /// Normalizes the vector; vectors below Java's epsilon become [`Vec3::ZERO`].
    #[inline]
    pub fn normalize(self) -> Vec3 {
        let square_root = self.length();
        if square_root < Self::NORMALIZE_EPSILON {
            Vec3::ZERO
        } else {
            Vec3::new(
                self.x / square_root,
                self.y / square_root,
                self.z / square_root,
            )
        }
    }

    #[inline]
    pub const fn distance_to_sqr(self, other: Vec3) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        let dz = other.z - self.z;
        dx * dx + dy * dy + dz * dz
    }

    #[inline]
    pub fn distance_to(self, other: Vec3) -> f64 {
        self.distance_to_sqr(other).sqrt()
    }

    /// Java's `Vec3.closerThan(Position, double)`: strict distance comparison.
    #[inline]
    pub fn closer_than(self, other: Vec3, distance: f64) -> bool {
        self.distance_to_sqr(other) < distance * distance
    }

    /// Linear interpolation towards `to`, matching `Vec3.lerp`.
    #[inline]
    pub fn lerp(self, to: Vec3, delta: f64) -> Vec3 {
        Vec3::new(
            mth::lerp(delta, self.x, to.x),
            mth::lerp(delta, self.y, to.y),
            mth::lerp(delta, self.z, to.z),
        )
    }

    #[inline]
    pub const fn with_x(self, x: f64) -> Vec3 {
        Vec3::new(x, self.y, self.z)
    }

    #[inline]
    pub const fn with_y(self, y: f64) -> Vec3 {
        Vec3::new(self.x, y, self.z)
    }

    #[inline]
    pub const fn with_z(self, z: f64) -> Vec3 {
        Vec3::new(self.x, self.y, z)
    }

    /// Component-wise approximate equality using an inclusive absolute tolerance.
    #[inline]
    pub fn close_to(self, other: Vec3, epsilon: f64) -> bool {
        (self.x - other.x).abs() <= epsilon
            && (self.y - other.y).abs() <= epsilon
            && (self.z - other.z).abs() <= epsilon
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    #[inline]
    pub const fn reverse(self) -> Vec3 {
        self.scale(-1.0)
    }
}

impl Add for Vec3 {
    type Output = Vec3;

    #[inline]
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3::add(self, rhs)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;

    #[inline]
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3::subtract(self, rhs)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Vec3;

    #[inline]
    fn mul(self, rhs: f64) -> Vec3 {
        Vec3::scale(self, rhs)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;

    #[inline]
    fn neg(self) -> Vec3 {
        Vec3::scale(self, -1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_constants() {
        assert_eq!(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(Vec3::ZERO, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(Vec3::X_AXIS, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(Vec3::Y_AXIS, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(Vec3::Z_AXIS, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(Vec3::default(), Vec3::ZERO);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).x, 1.0);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).y, 2.0);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).z, 3.0);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).x(), 1.0);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).y(), 2.0);
        assert_eq!(Vec3::new(1.0, 2.0, 3.0).z(), 3.0);
    }

    #[test]
    fn add_subtract_scale() {
        let a = Vec3::new(1.0, -2.0, 3.5);
        let b = Vec3::new(-4.0, 5.0, 0.5);
        assert_eq!(a.add(b), Vec3::new(-3.0, 3.0, 4.0));
        assert_eq!(a.subtract(b), Vec3::new(5.0, -7.0, 3.0));
        assert_eq!(a.scale(2.0), Vec3::new(2.0, -4.0, 7.0));
        assert_eq!(a + b, Vec3::new(-3.0, 3.0, 4.0));
        assert_eq!(a - b, Vec3::new(5.0, -7.0, 3.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, -4.0, 7.0));
        assert_eq!(-a, Vec3::new(-1.0, 2.0, -3.5));
        assert_eq!(a.reverse(), Vec3::new(-1.0, 2.0, -3.5));
    }

    #[test]
    fn dot_and_cross() {
        let x = Vec3::X_AXIS;
        let y = Vec3::Y_AXIS;
        let z = Vec3::Z_AXIS;
        assert_eq!(x.dot(y), 0.0);
        assert_eq!(x.dot(x), 1.0);
        assert_eq!(
            Vec3::new(1.0, 2.0, 3.0).dot(Vec3::new(4.0, -5.0, 6.0)),
            12.0
        );
        assert_eq!(x.cross(y), z);
        assert_eq!(y.cross(z), x);
        assert_eq!(z.cross(x), y);
        assert_eq!(y.cross(x), -z);
        assert_eq!(
            Vec3::new(1.0, 2.0, 3.0).cross(Vec3::new(4.0, 5.0, 6.0)),
            Vec3::new(-3.0, 6.0, -3.0)
        );
    }

    #[test]
    fn length_and_distance() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(v.length_sqr(), 25.0);
        assert_eq!(v.length(), 5.0);
        assert_eq!(Vec3::new(1.0, 2.0, 2.0).length(), 3.0);
        assert_eq!(Vec3::new(3.0, 4.0, 12.0).length(), 13.0);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).horizontal_distance_sqr(), 9.0);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).horizontal_distance(), 3.0);
        assert_eq!(Vec3::new(3.0, 100.0, 4.0).horizontal_distance(), 5.0);

        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 6.0, 3.0);
        assert_eq!(a.distance_to_sqr(b), 25.0);
        assert_eq!(a.distance_to_sqr(b), b.distance_to_sqr(a));
        assert_eq!(a.distance_to(b), 5.0);
        assert_eq!(a.distance_to(a), 0.0);
        assert!(a.closer_than(b, 5.1));
        assert!(!a.closer_than(b, 5.0));
        assert!(!a.closer_than(b, 4.999));
    }

    #[test]
    fn normalize_zero_and_tiny_vectors_stay_zero() {
        assert_eq!(Vec3::ZERO.normalize(), Vec3::ZERO);
        assert_eq!(Vec3::new(0.0, 0.0, 0.0).normalize(), Vec3::ZERO);
        assert_eq!(Vec3::new(1.0e-6, 0.0, 0.0).normalize(), Vec3::ZERO);
        assert_eq!(Vec3::new(1.0e-6, -1.0e-6, 1.0e-6).normalize(), Vec3::ZERO);
    }

    #[test]
    fn normalize_unit_and_general_vectors() {
        assert_eq!(Vec3::X_AXIS.normalize(), Vec3::X_AXIS);
        assert_eq!(
            Vec3::new(0.0, 0.0, -4.0).normalize(),
            Vec3::new(0.0, 0.0, -1.0)
        );
        let normalized = Vec3::new(1.0, 2.0, 3.0).normalize();
        assert!((normalized.length() - 1.0).abs() < 1.0e-15);
        assert!(normalized.x > 0.0 && normalized.y > 0.0 && normalized.z > 0.0);
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let start = Vec3::new(0.0, 10.0, -2.0);
        let end = Vec3::new(4.0, 20.0, 6.0);
        assert_eq!(start.lerp(end, 0.0), start);
        assert_eq!(start.lerp(end, 1.0), end);
        assert_eq!(start.lerp(end, 0.5), Vec3::new(2.0, 15.0, 2.0));
        assert_eq!(start.lerp(end, 0.25), Vec3::new(1.0, 12.5, 0.0));
    }

    #[test]
    fn with_axis_replaces_only_that_component() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(v.with_x(-4.0), Vec3::new(-4.0, 2.0, 3.0));
        assert_eq!(v.with_y(-5.0), Vec3::new(1.0, -5.0, 3.0));
        assert_eq!(v.with_z(-6.0), Vec3::new(1.0, 2.0, -6.0));
    }

    #[test]
    fn close_to_uses_inclusive_component_tolerance() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert!(v.close_to(v, 0.0));
        assert!(v.close_to(Vec3::new(1.4, 2.5, 3.4), 0.5));
        assert!(!v.close_to(Vec3::new(1.6, 2.0, 3.0), 0.5));
        assert!(!v.close_to(Vec3::new(1.0, 3.0, 3.0), 0.5));
        assert!(v.is_finite());
        assert!(!Vec3::new(f64::INFINITY, 0.0, 0.0).is_finite());
        assert!(!Vec3::new(f64::NAN, 0.0, 0.0).is_finite());
    }
}
