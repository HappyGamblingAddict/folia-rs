//! Port of `net.minecraft.core.Direction`.

use crate::vec3::Vec3;

/// The three coordinate axes, mirroring `Direction.Axis`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    #[inline]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Axis::Y)
    }

    #[inline]
    pub const fn is_horizontal(self) -> bool {
        matches!(self, Axis::X | Axis::Z)
    }

    #[inline]
    pub const fn positive(self) -> Direction {
        match self {
            Axis::X => Direction::East,
            Axis::Y => Direction::Up,
            Axis::Z => Direction::South,
        }
    }

    #[inline]
    pub const fn negative(self) -> Direction {
        match self {
            Axis::X => Direction::West,
            Axis::Y => Direction::Down,
            Axis::Z => Direction::North,
        }
    }

    /// Equivalent to `Direction.Axis.choose(int, int, int)`.
    #[inline]
    pub fn choose<T>(self, x: T, y: T, z: T) -> T {
        match self {
            Axis::X => x,
            Axis::Y => y,
            Axis::Z => z,
        }
    }
}

/// A block face, in the same declaration order as Java's `Direction`.
///
/// The discriminants are the `get3DDataValue()` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Direction {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Direction {
    /// Directions in Java declaration (3D data) order.
    pub const ALL: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];

    /// Horizontal directions ordered by their 2D data value:
    /// south (0), west (1), north (2), east (3).
    pub const BY_2D_DATA: [Direction; 4] = [
        Direction::South,
        Direction::West,
        Direction::North,
        Direction::East,
    ];

    #[inline]
    pub const fn data_3d(self) -> i32 {
        self as i32
    }

    #[inline]
    pub const fn data_2d(self) -> i32 {
        match self {
            Direction::Down | Direction::Up => -1,
            Direction::North => 2,
            Direction::South => 0,
            Direction::West => 1,
            Direction::East => 3,
        }
    }

    #[inline]
    pub const fn axis(self) -> Axis {
        match self {
            Direction::Down | Direction::Up => Axis::Y,
            Direction::North | Direction::South => Axis::Z,
            Direction::West | Direction::East => Axis::X,
        }
    }

    #[inline]
    pub const fn opposite(self) -> Direction {
        match self {
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::East => Direction::West,
        }
    }

    #[inline]
    pub const fn step_x(self) -> i32 {
        match self {
            Direction::West => -1,
            Direction::East => 1,
            _ => 0,
        }
    }

    #[inline]
    pub const fn step_y(self) -> i32 {
        match self {
            Direction::Down => -1,
            Direction::Up => 1,
            _ => 0,
        }
    }

    #[inline]
    pub const fn step_z(self) -> i32 {
        match self {
            Direction::North => -1,
            Direction::South => 1,
            _ => 0,
        }
    }

    #[inline]
    pub const fn is_horizontal(self) -> bool {
        self.axis().is_horizontal()
    }

    /// Equivalent to `Direction.from3DDataValue(int)`.
    ///
    /// Java wraps out-of-range indices with `Math.abs(index % 6)`; this keeps
    /// that asymmetric behavior (for example `-1` maps to `UP`, not `EAST`).
    #[inline]
    pub const fn from_3d_data(index: i32) -> Direction {
        Direction::ALL[index.wrapping_rem(6).wrapping_abs() as usize]
    }

    /// Equivalent to `Direction.from2DDataValue(int)`.
    ///
    /// Wraps with `Math.abs(index % 4)` exactly like Java.
    #[inline]
    pub const fn from_2d_data(index: i32) -> Direction {
        Direction::BY_2D_DATA[index.wrapping_rem(4).wrapping_abs() as usize]
    }

    /// Equivalent to `Direction.fromYRot(double)`.
    ///
    /// `y_rot` is in degrees. Only horizontal directions are returned.
    #[inline]
    pub fn from_y_rot(y_rot: f64) -> Direction {
        let index = crate::mth::floor(y_rot / 90.0 + 0.5) & 3;
        Direction::from_2d_data(index)
    }

    /// Finds the direction closest to a normal vector.
    ///
    /// This is `Direction.getApproximateNearest(double, double, double)`:
    /// it picks the member of [`Direction::ALL`] with the largest dot product,
    /// using Java's `float` arithmetic and `Float.MIN_VALUE` initial best score.
    /// A zero vector therefore resolves to `NORTH`, exactly as in Java.
    #[inline]
    pub fn from_normal(x: f64, y: f64, z: f64) -> Direction {
        let x = x as f32;
        let y = y as f32;
        let z = z as f32;
        let mut result = Direction::North;
        // Java's Float.MIN_VALUE is the smallest positive subnormal, not
        // f32::MIN_POSITIVE (the smallest positive normal).
        let mut best = f32::from_bits(1);
        for direction in Direction::ALL {
            let dot = x * direction.step_x() as f32
                + y * direction.step_y() as f32
                + z * direction.step_z() as f32;
            if dot > best {
                best = dot;
                result = direction;
            }
        }
        result
    }

    /// Convenience wrapper around [`Direction::from_normal`].
    #[inline]
    pub fn from_normal_vec(normal: Vec3) -> Direction {
        Direction::from_normal(normal.x, normal.y, normal.z)
    }
}

impl Axis {
    /// Directions along this axis, positive first, matching `Axis.getDirections()`.
    #[inline]
    pub const fn directions(self) -> [Direction; 2] {
        [self.positive(), self.negative()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_is_an_involution() {
        for direction in Direction::ALL {
            assert_eq!(direction.opposite().opposite(), direction);
            assert_ne!(direction.opposite(), direction);
        }
        assert_eq!(Direction::Down.opposite(), Direction::Up);
        assert_eq!(Direction::North.opposite(), Direction::South);
        assert_eq!(Direction::West.opposite(), Direction::East);
    }

    #[test]
    fn axis_assignment_matches_java() {
        assert_eq!(Direction::Down.axis(), Axis::Y);
        assert_eq!(Direction::Up.axis(), Axis::Y);
        assert_eq!(Direction::North.axis(), Axis::Z);
        assert_eq!(Direction::South.axis(), Axis::Z);
        assert_eq!(Direction::West.axis(), Axis::X);
        assert_eq!(Direction::East.axis(), Axis::X);

        assert!(Axis::Y.is_vertical());
        assert!(!Axis::X.is_vertical());
        assert!(Axis::X.is_horizontal());
        assert!(Axis::Z.is_horizontal());
        assert!(!Axis::Y.is_horizontal());
    }

    #[test]
    fn steps_match_java_normals() {
        for direction in Direction::ALL {
            assert_eq!(
                direction.step_x().abs() + direction.step_y().abs() + direction.step_z().abs(),
                1
            );
            assert_eq!(
                direction
                    .axis()
                    .choose(direction.step_x(), direction.step_y(), direction.step_z())
                    .abs(),
                1
            );
            assert_eq!(direction.opposite().step_x(), -direction.step_x());
            assert_eq!(direction.opposite().step_y(), -direction.step_y());
            assert_eq!(direction.opposite().step_z(), -direction.step_z());
        }
        assert_eq!(
            (
                Direction::Down.step_x(),
                Direction::Down.step_y(),
                Direction::Down.step_z()
            ),
            (0, -1, 0)
        );
        assert_eq!(
            (
                Direction::Up.step_x(),
                Direction::Up.step_y(),
                Direction::Up.step_z()
            ),
            (0, 1, 0)
        );
        assert_eq!(
            (
                Direction::North.step_x(),
                Direction::North.step_y(),
                Direction::North.step_z()
            ),
            (0, 0, -1)
        );
        assert_eq!(
            (
                Direction::South.step_x(),
                Direction::South.step_y(),
                Direction::South.step_z()
            ),
            (0, 0, 1)
        );
        assert_eq!(
            (
                Direction::West.step_x(),
                Direction::West.step_y(),
                Direction::West.step_z()
            ),
            (-1, 0, 0)
        );
        assert_eq!(
            (
                Direction::East.step_x(),
                Direction::East.step_y(),
                Direction::East.step_z()
            ),
            (1, 0, 0)
        );
    }

    #[test]
    fn axis_positive_and_negative() {
        assert_eq!(Axis::X.positive(), Direction::East);
        assert_eq!(Axis::X.negative(), Direction::West);
        assert_eq!(Axis::Y.positive(), Direction::Up);
        assert_eq!(Axis::Y.negative(), Direction::Down);
        assert_eq!(Axis::Z.positive(), Direction::South);
        assert_eq!(Axis::Z.negative(), Direction::North);

        for axis in Axis::ALL {
            assert_eq!(axis.positive().axis(), axis);
            assert_eq!(axis.negative().axis(), axis);
            assert_eq!(axis.directions(), [axis.positive(), axis.negative()]);
        }
    }

    #[test]
    fn data_3d_round_trips() {
        for (index, direction) in Direction::ALL.into_iter().enumerate() {
            assert_eq!(direction.data_3d(), index as i32);
            assert_eq!(Direction::from_3d_data(index as i32), direction);
        }
    }

    #[test]
    fn from_3d_data_matches_java_wrapping() {
        assert_eq!(Direction::from_3d_data(6), Direction::Down);
        assert_eq!(Direction::from_3d_data(7), Direction::Up);
        assert_eq!(Direction::from_3d_data(12), Direction::Down);
        assert_eq!(Direction::from_3d_data(-1), Direction::Up);
        assert_eq!(Direction::from_3d_data(-2), Direction::North);
        assert_eq!(Direction::from_3d_data(-6), Direction::Down);
        assert_eq!(Direction::from_3d_data(-7), Direction::Up);
        assert_eq!(Direction::from_3d_data(i32::MIN), Direction::North);
    }

    #[test]
    fn data_2d_and_from_y_rot() {
        assert_eq!(Direction::Down.data_2d(), -1);
        assert_eq!(Direction::Up.data_2d(), -1);
        assert_eq!(Direction::South.data_2d(), 0);
        assert_eq!(Direction::West.data_2d(), 1);
        assert_eq!(Direction::North.data_2d(), 2);
        assert_eq!(Direction::East.data_2d(), 3);

        assert_eq!(Direction::from_y_rot(0.0), Direction::South);
        assert_eq!(Direction::from_y_rot(44.9), Direction::South);
        assert_eq!(Direction::from_y_rot(45.0), Direction::West);
        assert_eq!(Direction::from_y_rot(90.0), Direction::West);
        assert_eq!(Direction::from_y_rot(180.0), Direction::North);
        assert_eq!(Direction::from_y_rot(270.0), Direction::East);
        assert_eq!(Direction::from_y_rot(360.0), Direction::South);
        assert_eq!(Direction::from_y_rot(-90.0), Direction::East);
        assert_eq!(Direction::from_y_rot(-180.0), Direction::North);
    }

    #[test]
    fn from_normal_picks_axes_and_java_default() {
        assert_eq!(Direction::from_normal(1.0, 0.0, 0.0), Direction::East);
        assert_eq!(Direction::from_normal(-1.0, 0.0, 0.0), Direction::West);
        assert_eq!(Direction::from_normal(0.0, 1.0, 0.0), Direction::Up);
        assert_eq!(Direction::from_normal(0.0, -1.0, 0.0), Direction::Down);
        assert_eq!(Direction::from_normal(0.0, 0.0, 1.0), Direction::South);
        assert_eq!(Direction::from_normal(0.0, 0.0, -1.0), Direction::North);

        // Java's tie/default behavior.
        assert_eq!(Direction::from_normal(0.0, 0.0, 0.0), Direction::North);
        assert_eq!(Direction::from_normal(1.0, 1.0, 1.0), Direction::Up);
        assert_eq!(Direction::from_normal(0.1, 0.2, 0.3), Direction::South);
        assert_eq!(
            Direction::from_normal_vec(Vec3::new(-3.0, 0.0, 0.0)),
            Direction::West
        );
    }

    #[test]
    fn from_2d_data_wraps_like_java() {
        assert_eq!(Direction::from_2d_data(0), Direction::South);
        assert_eq!(Direction::from_2d_data(1), Direction::West);
        assert_eq!(Direction::from_2d_data(2), Direction::North);
        assert_eq!(Direction::from_2d_data(3), Direction::East);
        assert_eq!(Direction::from_2d_data(4), Direction::South);
        assert_eq!(Direction::from_2d_data(-1), Direction::West);
        assert_eq!(Direction::from_2d_data(-4), Direction::South);
    }
}
