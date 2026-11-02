//! Port of `net.minecraft.world.phys.AABB`.

use crate::block_pos::BlockPos;
use crate::mth;
use crate::vec3::Vec3;

/// Java's `Math.min` semantics, including NaN propagation and signed zero.
#[inline]
fn java_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// Java's `Math.max` semantics, including NaN propagation and signed zero.
#[inline]
fn java_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// An axis-aligned bounding box, mirroring `AABB`.
///
/// The constructor normalizes the corners, so `min_* <= max_*` for all
/// non-NaN boxes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AABB {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl AABB {
    /// `new AABB(double x1, double y1, double z1, double x2, double y2, double z2)`.
    #[inline]
    pub fn new(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> AABB {
        AABB {
            min_x: java_min(x1, x2),
            min_y: java_min(y1, y2),
            min_z: java_min(z1, z2),
            max_x: java_max(x1, x2),
            max_y: java_max(y1, y2),
            max_z: java_max(z1, z2),
        }
    }

    /// Unit cube occupied by `pos` (`new AABB(BlockPos)`).
    #[inline]
    pub fn of(pos: BlockPos) -> AABB {
        AABB::new(
            pos.x() as f64,
            pos.y() as f64,
            pos.z() as f64,
            pos.x().wrapping_add(1) as f64,
            pos.y().wrapping_add(1) as f64,
            pos.z().wrapping_add(1) as f64,
        )
    }

    /// `new AABB(Vec3 start, Vec3 end)`; corners may be given in any order.
    #[inline]
    pub fn from(start: Vec3, end: Vec3) -> AABB {
        AABB::new(start.x, start.y, start.z, end.x, end.y, end.z)
    }

    /// Strict overlap test. Boxes that only touch at a face, edge, or corner do
    /// **not** intersect, matching Java.
    #[inline]
    pub fn intersects(&self, other: &AABB) -> bool {
        self.intersects_coords(
            other.min_x,
            other.min_y,
            other.min_z,
            other.max_x,
            other.max_y,
            other.max_z,
        )
    }

    /// Coordinate form of [`AABB::intersects`].
    #[inline]
    pub fn intersects_coords(&self, x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> bool {
        self.min_x < x2
            && self.max_x > x1
            && self.min_y < y2
            && self.max_y > y1
            && self.min_z < z2
            && self.max_z > z1
    }

    /// Point containment with an inclusive minimum and exclusive maximum.
    #[inline]
    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min_x
            && point.x < self.max_x
            && point.y >= self.min_y
            && point.y < self.max_y
            && point.z >= self.min_z
            && point.z < self.max_z
    }

    /// Extends the box in the direction of `vector` (`AABB.expandTowards`).
    #[inline]
    pub fn expand_towards(&self, vector: Vec3) -> AABB {
        self.expand_towards_xyz(vector.x, vector.y, vector.z)
    }

    /// Coordinate form of [`AABB::expand_towards`].
    pub fn expand_towards_xyz(&self, x: f64, y: f64, z: f64) -> AABB {
        let mut min_x = self.min_x;
        let mut min_y = self.min_y;
        let mut min_z = self.min_z;
        let mut max_x = self.max_x;
        let mut max_y = self.max_y;
        let mut max_z = self.max_z;

        if x < 0.0 {
            min_x += x;
        } else if x > 0.0 {
            max_x += x;
        }

        if y < 0.0 {
            min_y += y;
        } else if y > 0.0 {
            max_y += y;
        }

        if z < 0.0 {
            min_z += z;
        } else if z > 0.0 {
            max_z += z;
        }

        AABB::new(min_x, min_y, min_z, max_x, max_y, max_z)
    }

    /// Grows the box by the given amounts on each axis (`AABB.inflate`).
    ///
    /// Negative amounts shrink the box. The result is re-normalized exactly as
    /// by the Java constructor.
    #[inline]
    pub fn inflate(&self, x: f64, y: f64, z: f64) -> AABB {
        AABB::new(
            self.min_x - x,
            self.min_y - y,
            self.min_z - z,
            self.max_x + x,
            self.max_y + y,
            self.max_z + z,
        )
    }

    /// Uniform form of [`AABB::inflate`] (`AABB.inflate(double)`).
    #[inline]
    pub fn inflate_uniform(&self, value: f64) -> AABB {
        self.inflate(value, value, value)
    }

    /// Translates the box (`AABB.move(double, double, double)`).
    ///
    /// Named `translate` because `move` is a Rust keyword.
    #[inline]
    pub fn translate(&self, x: f64, y: f64, z: f64) -> AABB {
        AABB::new(
            self.min_x + x,
            self.min_y + y,
            self.min_z + z,
            self.max_x + x,
            self.max_y + y,
            self.max_z + z,
        )
    }

    /// Translates the box by a vector (`AABB.move(Vec3)`).
    #[inline]
    pub fn translate_vec(&self, vector: Vec3) -> AABB {
        self.translate(vector.x, vector.y, vector.z)
    }

    /// Translates the box by a block position (`AABB.move(BlockPos)`).
    #[inline]
    pub fn translate_pos(&self, pos: BlockPos) -> AABB {
        self.translate(pos.x() as f64, pos.y() as f64, pos.z() as f64)
    }

    /// Box center, computed with `Mth.lerp(0.5, min, max)`.
    #[inline]
    pub fn get_center(&self) -> Vec3 {
        Vec3::new(
            mth::lerp(0.5, self.min_x, self.max_x),
            mth::lerp(0.5, self.min_y, self.max_y),
            mth::lerp(0.5, self.min_z, self.max_z),
        )
    }

    /// Average of the three side lengths (`AABB.getSize()`).
    #[inline]
    pub fn get_size(&self) -> f64 {
        let x_size = self.max_x - self.min_x;
        let y_size = self.max_y - self.min_y;
        let z_size = self.max_z - self.min_z;
        (x_size + y_size + z_size) / 3.0
    }

    #[inline]
    pub fn get_min_position(&self) -> Vec3 {
        Vec3::new(self.min_x, self.min_y, self.min_z)
    }

    #[inline]
    pub fn get_max_position(&self) -> Vec3 {
        Vec3::new(self.max_x, self.max_y, self.max_z)
    }

    #[inline]
    pub fn has_nan(&self) -> bool {
        self.min_x.is_nan()
            || self.min_y.is_nan()
            || self.min_z.is_nan()
            || self.max_x.is_nan()
            || self.max_y.is_nan()
            || self.max_z.is_nan()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_cube() -> AABB {
        AABB::new(0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
    }

    #[test]
    fn constructor_normalizes_corners() {
        let a = AABB::new(2.0, 4.0, 6.0, 0.0, 0.0, 0.0);
        assert_eq!(a.min_x, 0.0);
        assert_eq!(a.min_y, 0.0);
        assert_eq!(a.min_z, 0.0);
        assert_eq!(a.max_x, 2.0);
        assert_eq!(a.max_y, 4.0);
        assert_eq!(a.max_z, 6.0);

        let b = AABB::from(Vec3::new(2.0, 4.0, 6.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(a, b);
    }

    #[test]
    fn of_block_pos_is_a_unit_cube() {
        let a = AABB::of(BlockPos::new(-3, 7, 12));
        assert_eq!(a.min_x, -3.0);
        assert_eq!(a.min_y, 7.0);
        assert_eq!(a.min_z, 12.0);
        assert_eq!(a.max_x, -2.0);
        assert_eq!(a.max_y, 8.0);
        assert_eq!(a.max_z, 13.0);
    }

    #[test]
    fn intersects_requires_strict_overlap() {
        let a = unit_cube();
        assert!(a.intersects(&AABB::new(0.5, 0.5, 0.5, 1.5, 1.5, 1.5)));
        assert!(a.intersects(&AABB::new(0.9, 0.9, 0.9, 1.1, 1.1, 1.1)));
        assert!(a.intersects(&AABB::new(0.25, 0.25, 0.25, 0.75, 0.75, 0.75)));
        assert!(a.intersects(&a));

        // Touching a face is not an intersection.
        assert!(!a.intersects(&AABB::new(1.0, 0.0, 0.0, 2.0, 1.0, 1.0)));
        assert!(!a.intersects(&AABB::new(-1.0, 0.0, 0.0, 0.0, 1.0, 1.0)));
        assert!(!a.intersects(&AABB::new(0.0, 1.0, 0.0, 1.0, 2.0, 1.0)));

        // Touching only along an edge or at a corner is also excluded.
        assert!(!a.intersects(&AABB::new(1.0, 1.0, 0.0, 2.0, 2.0, 1.0)));
        assert!(!a.intersects(&AABB::new(1.0, 1.0, 1.0, 2.0, 2.0, 2.0)));

        // Complete separation.
        assert!(!a.intersects(&AABB::new(2.0, 2.0, 2.0, 3.0, 3.0, 3.0)));
        assert!(!a.intersects(&AABB::new(-2.0, -2.0, -2.0, -1.0, -1.0, -1.0)));
    }

    #[test]
    fn intersects_coords_matches_reference_form() {
        let a = unit_cube();
        assert!(a.intersects_coords(-0.5, 0.2, 0.2, 0.5, 0.8, 0.8));
        assert!(!a.intersects_coords(1.0, 0.0, 0.0, 2.0, 1.0, 1.0));
    }

    #[test]
    fn contains_is_min_inclusive_max_exclusive() {
        let a = unit_cube();
        assert!(a.contains(Vec3::new(0.0, 0.0, 0.0)));
        assert!(a.contains(Vec3::new(0.5, 0.5, 0.5)));
        assert!(a.contains(Vec3::new(0.999, 0.999, 0.999)));
        assert!(!a.contains(Vec3::new(1.0, 0.5, 0.5)));
        assert!(!a.contains(Vec3::new(0.5, 1.0, 0.5)));
        assert!(!a.contains(Vec3::new(0.5, 0.5, 1.0)));
        assert!(!a.contains(Vec3::new(-0.001, 0.5, 0.5)));
        assert!(!a.contains(Vec3::new(0.5, -0.001, 0.5)));
        assert!(!a.contains(Vec3::new(0.5, 0.5, -0.001)));
    }

    #[test]
    fn expand_towards_moves_only_the_appropriate_bound() {
        let a = unit_cube();
        assert_eq!(
            a.expand_towards(Vec3::new(2.0, 3.0, 4.0)),
            AABB::new(0.0, 0.0, 0.0, 3.0, 4.0, 5.0)
        );
        assert_eq!(
            a.expand_towards(Vec3::new(-2.0, -3.0, -4.0)),
            AABB::new(-2.0, -3.0, -4.0, 1.0, 1.0, 1.0)
        );
        assert_eq!(
            a.expand_towards_xyz(2.0, -3.0, 0.0),
            AABB::new(0.0, -3.0, 0.0, 3.0, 1.0, 1.0)
        );
        assert_eq!(a.expand_towards(Vec3::ZERO), a);
        assert_eq!(
            a.expand_towards(Vec3::new(f64::NAN, 1.0, 1.0)),
            AABB::new(0.0, 0.0, 0.0, 1.0, 2.0, 2.0)
        );
    }

    #[test]
    fn inflate_and_deflate() {
        let a = unit_cube();
        assert_eq!(
            a.inflate(1.0, 2.0, 3.0),
            AABB::new(-1.0, -2.0, -3.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(
            a.inflate_uniform(0.5),
            AABB::new(-0.5, -0.5, -0.5, 1.5, 1.5, 1.5)
        );

        let big = AABB::new(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        assert_eq!(
            big.inflate(-1.0, -2.0, -3.0),
            AABB::new(1.0, 2.0, 3.0, 9.0, 8.0, 7.0)
        );
    }

    #[test]
    fn translate_forms() {
        let a = unit_cube();
        let expected = AABB::new(2.0, 4.0, 6.0, 3.0, 5.0, 7.0);
        assert_eq!(a.translate(2.0, 4.0, 6.0), expected);
        assert_eq!(a.translate_vec(Vec3::new(2.0, 4.0, 6.0)), expected);
        assert_eq!(a.translate_pos(BlockPos::new(2, 4, 6)), expected);
        assert_eq!(a.translate(0.0, 0.0, 0.0), a);
    }

    #[test]
    fn center_size_and_positions() {
        let a = AABB::new(0.0, 0.0, 0.0, 2.0, 4.0, 6.0);
        assert_eq!(a.get_center(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(a.get_size(), 4.0);
        assert_eq!(a.get_min_position(), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(a.get_max_position(), Vec3::new(2.0, 4.0, 6.0));

        let unit = unit_cube();
        assert_eq!(unit.get_center(), Vec3::new(0.5, 0.5, 0.5));
        assert_eq!(unit.get_size(), 1.0);
    }

    #[test]
    fn nan_propagation_and_detection() {
        let a = unit_cube();
        assert!(!a.has_nan());
        assert!(AABB::new(f64::NAN, 0.0, 0.0, 1.0, 1.0, 1.0).has_nan());
        assert!(AABB::new(0.0, 0.0, 0.0, 1.0, f64::NAN, 1.0).has_nan());

        let nan_box = AABB::new(f64::NAN, 0.0, 0.0, 1.0, 1.0, 1.0);
        assert!(nan_box.min_x.is_nan());
        assert!(nan_box.max_x.is_nan());
    }
}
