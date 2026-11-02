//! Port of `net.minecraft.core.BlockPos`.

use crate::direction::Direction;

/// Number of bits used for packed block X and Z coordinates.
pub const PACKED_HORIZONTAL_LENGTH: u32 = 26;
/// Number of bits used for a packed block Y coordinate.
pub const PACKED_Y_LENGTH: u32 = 12;
/// Largest legal horizontal block coordinate (`2^25 - 1`).
pub const MAX_HORIZONTAL_COORDINATE: i32 = 33_554_431;

const PACKED_X_MASK: i64 = (1_i64 << PACKED_HORIZONTAL_LENGTH) - 1;
const PACKED_Y_MASK: i64 = (1_i64 << PACKED_Y_LENGTH) - 1;
const Z_OFFSET: u32 = PACKED_Y_LENGTH;
const X_OFFSET: u32 = PACKED_Y_LENGTH + PACKED_HORIZONTAL_LENGTH;

/// An immutable integer block position, mirroring `BlockPos`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockPos {
    x: i32,
    y: i32,
    z: i32,
}

impl BlockPos {
    pub const ZERO: BlockPos = BlockPos::new(0, 0, 0);

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos { x, y, z }
    }

    #[inline]
    pub const fn x(self) -> i32 {
        self.x
    }

    #[inline]
    pub const fn y(self) -> i32 {
        self.y
    }

    #[inline]
    pub const fn z(self) -> i32 {
        self.z
    }

    /// Packs coordinates exactly like `BlockPos.asLong(int, int, int)`:
    /// X in bits 38..63 (26 bits), Y in bits 0..11 (12 bits), Z in bits
    /// 12..37 (26 bits).
    #[inline]
    pub const fn pack(x: i32, y: i32, z: i32) -> i64 {
        ((x as i64 & PACKED_X_MASK) << X_OFFSET)
            | (y as i64 & PACKED_Y_MASK)
            | ((z as i64 & PACKED_X_MASK) << Z_OFFSET)
    }

    /// Unpacks a position exactly like `BlockPos.of(long)`.
    ///
    /// Coordinates are sign-extended from their packed widths, so
    /// `from_long(-1)` is `(-1, -1, -1)`.
    #[inline]
    pub const fn from_long(packed: i64) -> BlockPos {
        BlockPos::new(
            Self::x_from_long(packed),
            Self::y_from_long(packed),
            Self::z_from_long(packed),
        )
    }

    #[inline]
    pub const fn x_from_long(packed: i64) -> i32 {
        (packed >> X_OFFSET) as i32
    }

    #[inline]
    pub const fn y_from_long(packed: i64) -> i32 {
        ((packed << (64 - PACKED_Y_LENGTH)) >> (64 - PACKED_Y_LENGTH)) as i32
    }

    #[inline]
    pub const fn z_from_long(packed: i64) -> i32 {
        ((packed << PACKED_HORIZONTAL_LENGTH) >> (PACKED_HORIZONTAL_LENGTH + Z_OFFSET)) as i32
    }

    #[inline]
    pub const fn as_long(self) -> i64 {
        Self::pack(self.x, self.y, self.z)
    }

    /// `BlockPos.offset(int, int, int)`; additions wrap like Java `int` math.
    #[inline]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> BlockPos {
        if dx == 0 && dy == 0 && dz == 0 {
            self
        } else {
            BlockPos::new(
                self.x.wrapping_add(dx),
                self.y.wrapping_add(dy),
                self.z.wrapping_add(dz),
            )
        }
    }

    /// `BlockPos.offset(Vec3i)`, using another position as the delta.
    #[inline]
    pub const fn offset_by(self, other: BlockPos) -> BlockPos {
        self.offset(other.x, other.y, other.z)
    }

    /// `BlockPos.subtract(Vec3i)`.
    #[inline]
    pub const fn subtract(self, other: BlockPos) -> BlockPos {
        self.offset(
            other.x.wrapping_neg(),
            other.y.wrapping_neg(),
            other.z.wrapping_neg(),
        )
    }

    #[inline]
    pub const fn above(self) -> BlockPos {
        self.above_n(1)
    }

    #[inline]
    pub const fn above_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x, self.y.wrapping_add(distance), self.z)
        }
    }

    #[inline]
    pub const fn below(self) -> BlockPos {
        self.below_n(1)
    }

    #[inline]
    pub const fn below_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x, self.y.wrapping_sub(distance), self.z)
        }
    }

    #[inline]
    pub const fn north(self) -> BlockPos {
        self.north_n(1)
    }

    #[inline]
    pub const fn north_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x, self.y, self.z.wrapping_sub(distance))
        }
    }

    #[inline]
    pub const fn south(self) -> BlockPos {
        self.south_n(1)
    }

    #[inline]
    pub const fn south_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x, self.y, self.z.wrapping_add(distance))
        }
    }

    #[inline]
    pub const fn west(self) -> BlockPos {
        self.west_n(1)
    }

    #[inline]
    pub const fn west_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x.wrapping_sub(distance), self.y, self.z)
        }
    }

    #[inline]
    pub const fn east(self) -> BlockPos {
        self.east_n(1)
    }

    #[inline]
    pub const fn east_n(self, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(self.x.wrapping_add(distance), self.y, self.z)
        }
    }

    /// `BlockPos.relative(Direction)`.
    #[inline]
    pub const fn relative(self, direction: Direction) -> BlockPos {
        BlockPos::new(
            self.x.wrapping_add(direction.step_x()),
            self.y.wrapping_add(direction.step_y()),
            self.z.wrapping_add(direction.step_z()),
        )
    }

    /// `BlockPos.relative(Direction, int)`.
    #[inline]
    pub const fn relative_n(self, direction: Direction, distance: i32) -> BlockPos {
        if distance == 0 {
            self
        } else {
            BlockPos::new(
                self.x
                    .wrapping_add(direction.step_x().wrapping_mul(distance)),
                self.y
                    .wrapping_add(direction.step_y().wrapping_mul(distance)),
                self.z
                    .wrapping_add(direction.step_z().wrapping_mul(distance)),
            )
        }
    }

    /// `BlockPos.relative(Direction.Axis, int)`.
    #[inline]
    pub const fn relative_axis(self, axis: crate::direction::Axis, amount: i32) -> BlockPos {
        if amount == 0 {
            self
        } else {
            match axis {
                crate::direction::Axis::X => {
                    BlockPos::new(self.x.wrapping_add(amount), self.y, self.z)
                }
                crate::direction::Axis::Y => {
                    BlockPos::new(self.x, self.y.wrapping_add(amount), self.z)
                }
                crate::direction::Axis::Z => {
                    BlockPos::new(self.x, self.y, self.z.wrapping_add(amount))
                }
            }
        }
    }

    #[inline]
    pub const fn with_x(self, x: i32) -> BlockPos {
        BlockPos::new(x, self.y, self.z)
    }

    #[inline]
    pub const fn with_y(self, y: i32) -> BlockPos {
        BlockPos::new(self.x, y, self.z)
    }

    #[inline]
    pub const fn with_z(self, z: i32) -> BlockPos {
        BlockPos::new(self.x, self.y, z)
    }

    /// Iterates all positions in the inclusive box, matching
    /// `BlockPos.betweenClosed(BlockPos, BlockPos)`.
    ///
    /// X varies fastest, then Y, then Z. The two corners may be supplied in any
    /// order.
    pub fn between_closed(pos1: BlockPos, pos2: BlockPos) -> impl Iterator<Item = BlockPos> {
        Self::between_closed_coords(
            pos1.x.min(pos2.x),
            pos1.y.min(pos2.y),
            pos1.z.min(pos2.z),
            pos1.x.max(pos2.x),
            pos1.y.max(pos2.y),
            pos1.z.max(pos2.z),
        )
    }

    /// Coordinate form of [`BlockPos::between_closed`].
    pub fn between_closed_coords(
        min_x: i32,
        min_y: i32,
        min_z: i32,
        max_x: i32,
        max_y: i32,
        max_z: i32,
    ) -> impl Iterator<Item = BlockPos> {
        (min_z..=max_z).flat_map(move |z| {
            (min_y..=max_y).flat_map(move |y| (min_x..=max_x).map(move |x| BlockPos::new(x, y, z)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Axis;

    #[test]
    fn new_and_getters() {
        let pos = BlockPos::new(12, -34, 56);
        assert_eq!((pos.x(), pos.y(), pos.z()), (12, -34, 56));
        assert_eq!(BlockPos::ZERO, BlockPos::new(0, 0, 0));
        assert_eq!(BlockPos::default(), BlockPos::ZERO);
    }

    #[test]
    fn offset_by_delta_and_position() {
        let pos = BlockPos::new(10, 20, 30);
        assert_eq!(pos.offset(1, -2, 3), BlockPos::new(11, 18, 33));
        assert_eq!(pos.offset(0, 0, 0), pos);
        assert_eq!(
            pos.offset_by(BlockPos::new(-1, 2, -3)),
            BlockPos::new(9, 22, 27)
        );
        assert_eq!(
            pos.subtract(BlockPos::new(1, 2, 3)),
            BlockPos::new(9, 18, 27)
        );
    }

    #[test]
    fn directional_helpers_match_java() {
        let pos = BlockPos::new(0, 0, 0);
        assert_eq!(pos.above(), BlockPos::new(0, 1, 0));
        assert_eq!(pos.below(), BlockPos::new(0, -1, 0));
        assert_eq!(pos.north(), BlockPos::new(0, 0, -1));
        assert_eq!(pos.south(), BlockPos::new(0, 0, 1));
        assert_eq!(pos.west(), BlockPos::new(-1, 0, 0));
        assert_eq!(pos.east(), BlockPos::new(1, 0, 0));

        assert_eq!(pos.above_n(5), BlockPos::new(0, 5, 0));
        assert_eq!(pos.below_n(5), BlockPos::new(0, -5, 0));
        assert_eq!(pos.north_n(5), BlockPos::new(0, 0, -5));
        assert_eq!(pos.south_n(5), BlockPos::new(0, 0, 5));
        assert_eq!(pos.west_n(5), BlockPos::new(-5, 0, 0));
        assert_eq!(pos.east_n(5), BlockPos::new(5, 0, 0));

        for helper in [
            pos.above_n(0),
            pos.below_n(0),
            pos.north_n(0),
            pos.south_n(0),
            pos.west_n(0),
            pos.east_n(0),
        ] {
            assert_eq!(helper, pos);
        }
    }

    #[test]
    fn relative_and_axis_offsets() {
        let pos = BlockPos::new(3, 4, 5);
        assert_eq!(pos.relative(Direction::Up), BlockPos::new(3, 5, 5));
        assert_eq!(pos.relative(Direction::Down), BlockPos::new(3, 3, 5));
        assert_eq!(pos.relative(Direction::North), BlockPos::new(3, 4, 4));
        assert_eq!(pos.relative(Direction::South), BlockPos::new(3, 4, 6));
        assert_eq!(pos.relative(Direction::West), BlockPos::new(2, 4, 5));
        assert_eq!(pos.relative(Direction::East), BlockPos::new(4, 4, 5));

        assert_eq!(pos.relative_n(Direction::East, 3), BlockPos::new(6, 4, 5));
        assert_eq!(pos.relative_n(Direction::West, 3), BlockPos::new(0, 4, 5));
        assert_eq!(pos.relative_n(Direction::Up, 3), BlockPos::new(3, 7, 5));
        assert_eq!(pos.relative_n(Direction::South, 3), BlockPos::new(3, 4, 8));
        assert_eq!(pos.relative_n(Direction::East, 0), pos);

        assert_eq!(pos.relative_axis(Axis::X, 7), BlockPos::new(10, 4, 5));
        assert_eq!(pos.relative_axis(Axis::Y, -2), BlockPos::new(3, 2, 5));
        assert_eq!(pos.relative_axis(Axis::Z, 4), BlockPos::new(3, 4, 9));
        assert_eq!(pos.relative_axis(Axis::Z, 0), pos);
    }

    #[test]
    fn coordinate_replacement() {
        let pos = BlockPos::new(1, 2, 3);
        assert_eq!(pos.with_x(9), BlockPos::new(9, 2, 3));
        assert_eq!(pos.with_y(9), BlockPos::new(1, 9, 3));
        assert_eq!(pos.with_z(9), BlockPos::new(1, 2, 9));
    }

    #[test]
    fn arithmetic_wraps_like_java() {
        assert_eq!(
            BlockPos::new(i32::MAX, 0, 0).east(),
            BlockPos::new(i32::MIN, 0, 0)
        );
        assert_eq!(
            BlockPos::new(i32::MIN, 0, 0).west(),
            BlockPos::new(i32::MAX, 0, 0)
        );
        assert_eq!(
            BlockPos::new(0, i32::MAX, 0).above(),
            BlockPos::new(0, i32::MIN, 0)
        );
    }

    #[test]
    fn as_long_known_values() {
        assert_eq!(BlockPos::new(0, 0, 0).as_long(), 0);
        assert_eq!(BlockPos::new(1, 0, 0).as_long(), 274_877_906_944);
        assert_eq!(BlockPos::new(0, 1, 0).as_long(), 1);
        assert_eq!(BlockPos::new(0, 0, 1).as_long(), 4_096);
        assert_eq!(BlockPos::new(-1, 0, 0).as_long(), -274_877_906_944);
        assert_eq!(BlockPos::new(0, -1, 0).as_long(), 4_095);
        assert_eq!(BlockPos::new(0, 0, -1).as_long(), 274_877_902_848);
        assert_eq!(BlockPos::new(1, 2, 3).as_long(), 274_877_919_234);
        assert_eq!(BlockPos::new(-1, -2, -3).as_long(), -8_194);
    }

    #[test]
    fn as_long_extreme_values() {
        assert_eq!(
            BlockPos::new(33_554_431, 2_047, 33_554_431).as_long(),
            9_223_371_899_415_820_287
        );
        assert_eq!(
            BlockPos::new(-33_554_432, -2_048, -33_554_432).as_long(),
            -9_223_371_899_415_820_288
        );
        assert_eq!(
            BlockPos::new(33_554_431, 0, 0).as_long(),
            9_223_371_761_976_868_864
        );
        assert_eq!(
            BlockPos::new(-33_554_432, 0, 0).as_long(),
            -9_223_372_036_854_775_808
        );
        assert_eq!(
            BlockPos::new(123_456, -789, -987_654).as_long(),
            33_935_597_712_157_931
        );
        assert_eq!(
            BlockPos::new(-123_456, 789, 987_654).as_long(),
            -33_935_322_834_246_891
        );
    }

    #[test]
    fn from_long_known_values() {
        assert_eq!(BlockPos::from_long(0), BlockPos::new(0, 0, 0));
        assert_eq!(BlockPos::from_long(1), BlockPos::new(0, 1, 0));
        assert_eq!(BlockPos::from_long(4_096), BlockPos::new(0, 0, 1));
        assert_eq!(BlockPos::from_long(274_877_906_944), BlockPos::new(1, 0, 0));
        assert_eq!(
            BlockPos::from_long(-274_877_906_944),
            BlockPos::new(-1, 0, 0)
        );
        assert_eq!(BlockPos::from_long(4_095), BlockPos::new(0, -1, 0));
        assert_eq!(
            BlockPos::from_long(274_877_902_848),
            BlockPos::new(0, 0, -1)
        );
        assert_eq!(BlockPos::from_long(274_877_919_234), BlockPos::new(1, 2, 3));
        assert_eq!(BlockPos::from_long(-8_194), BlockPos::new(-1, -2, -3));
        assert_eq!(BlockPos::from_long(-1), BlockPos::new(-1, -1, -1));
    }

    #[test]
    fn unpack_helpers_match_from_long() {
        let packed = BlockPos::pack(-12_345, 678, 90_123);
        assert_eq!(BlockPos::x_from_long(packed), -12_345);
        assert_eq!(BlockPos::y_from_long(packed), 678);
        assert_eq!(BlockPos::z_from_long(packed), 90_123);
        assert_eq!(
            BlockPos::from_long(packed),
            BlockPos::new(-12_345, 678, 90_123)
        );
    }

    #[test]
    fn packed_round_trip_for_negative_and_extreme_coordinates() {
        let cases = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 2, 3),
            BlockPos::new(-1, -2, -3),
            BlockPos::new(33_554_431, 2_047, 33_554_431),
            BlockPos::new(-33_554_432, -2_048, -33_554_432),
            BlockPos::new(123_456, -789, -987_654),
            BlockPos::new(-123_456, 789, 987_654),
            BlockPos::new(33_554_431, -2_048, -33_554_432),
        ];
        for pos in cases {
            assert_eq!(BlockPos::from_long(pos.as_long()), pos, "pos={pos:?}");
            assert_eq!(
                BlockPos::from_long(BlockPos::pack(pos.x(), pos.y(), pos.z())),
                pos
            );
        }
    }

    #[test]
    fn between_closed_is_inclusive_and_x_major() {
        let positions: Vec<_> =
            BlockPos::between_closed(BlockPos::new(1, 2, 3), BlockPos::new(3, 4, 5)).collect();
        assert_eq!(positions.len(), 27);
        assert_eq!(positions[0], BlockPos::new(1, 2, 3));
        assert_eq!(positions[1], BlockPos::new(2, 2, 3));
        assert_eq!(positions[2], BlockPos::new(3, 2, 3));
        assert_eq!(positions[3], BlockPos::new(1, 3, 3));
        assert_eq!(positions[9], BlockPos::new(1, 2, 4));
        assert_eq!(positions[26], BlockPos::new(3, 4, 5));
    }

    #[test]
    fn between_closed_accepts_reversed_corners_and_single_block() {
        let forward: Vec<_> =
            BlockPos::between_closed(BlockPos::new(1, 2, 3), BlockPos::new(3, 4, 5)).collect();
        let reverse: Vec<_> =
            BlockPos::between_closed(BlockPos::new(3, 4, 5), BlockPos::new(1, 2, 3)).collect();
        assert_eq!(forward, reverse);

        let single = BlockPos::new(-4, 7, 11);
        assert_eq!(
            BlockPos::between_closed(single, single).collect::<Vec<_>>(),
            vec![single]
        );

        let coords: Vec<_> = BlockPos::between_closed_coords(0, 0, 0, 1, 1, 1).collect();
        assert_eq!(coords.len(), 8);
        assert_eq!(coords[0], BlockPos::new(0, 0, 0));
        assert_eq!(coords[7], BlockPos::new(1, 1, 1));
    }
}
