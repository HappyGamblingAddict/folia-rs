//! Port of `net.minecraft.core.SectionPos`.

use crate::block_pos::BlockPos;
use crate::direction::Direction;

/// Bits used for section-local coordinates.
pub const SECTION_BITS: u32 = 4;
/// Width/height/depth of a section in blocks.
pub const SECTION_SIZE: i32 = 16;
/// Mask for section-local coordinates.
pub const SECTION_MASK: i32 = 15;
/// Half the width/height/depth of a section.
pub const SECTION_HALF_SIZE: i32 = 8;
/// Largest section-local index.
pub const SECTION_MAX_INDEX: i32 = 15;

/// Number of bits used for packed section X and Z coordinates.
pub const PACKED_X_LENGTH: u32 = 22;
/// Number of bits used for a packed section Y coordinate.
pub const PACKED_Y_LENGTH: u32 = 20;
/// Number of bits used for packed section Z coordinates.
pub const PACKED_Z_LENGTH: u32 = 22;

const PACKED_X_MASK: i64 = (1_i64 << PACKED_X_LENGTH) - 1;
const PACKED_Y_MASK: i64 = (1_i64 << PACKED_Y_LENGTH) - 1;
const PACKED_Z_MASK: i64 = (1_i64 << PACKED_Z_LENGTH) - 1;
const Z_OFFSET: u32 = PACKED_Y_LENGTH;
const X_OFFSET: u32 = PACKED_Y_LENGTH + PACKED_Z_LENGTH;

/// Converts a block coordinate to a section coordinate (`block >> 4`).
#[inline]
pub const fn block_to_section(block_coord: i32) -> i32 {
    block_coord >> SECTION_BITS
}

/// Converts a section coordinate to the block coordinate of its minimum corner
/// (`section << 4`).
#[inline]
pub const fn section_to_block(section_coord: i32) -> i32 {
    section_coord.wrapping_shl(SECTION_BITS)
}

/// Extracts a section-local coordinate (`block & 15`).
#[inline]
pub const fn section_relative(block_coord: i32) -> i32 {
    block_coord & SECTION_MASK
}

/// Converts a block-space double position to a section coordinate.
#[inline]
pub fn pos_to_section(pos: f64) -> i32 {
    block_to_section(crate::mth::floor(pos))
}

/// A section (16x16x16) position, mirroring `SectionPos`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SectionPos {
    x: i32,
    y: i32,
    z: i32,
}

impl SectionPos {
    #[inline]
    pub const fn of(x: i32, y: i32, z: i32) -> SectionPos {
        SectionPos { x, y, z }
    }

    /// `SectionPos.of(BlockPos)`.
    #[inline]
    pub const fn of_block(pos: BlockPos) -> SectionPos {
        SectionPos::of(
            block_to_section(pos.x()),
            block_to_section(pos.y()),
            block_to_section(pos.z()),
        )
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

    /// Packs section coordinates exactly like `SectionPos.asLong(int, int, int)`:
    /// X in bits 42..63 (22 bits), Y in bits 0..19 (20 bits), Z in bits
    /// 20..41 (22 bits).
    #[inline]
    pub const fn pack(x: i32, y: i32, z: i32) -> i64 {
        ((x as i64 & PACKED_X_MASK) << X_OFFSET)
            | (y as i64 & PACKED_Y_MASK)
            | ((z as i64 & PACKED_Z_MASK) << Z_OFFSET)
    }

    /// Unpacks a section position exactly like `SectionPos.of(long)`.
    #[inline]
    pub const fn from_long(packed: i64) -> SectionPos {
        SectionPos::of(
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
        ((packed << (64 - X_OFFSET)) >> (PACKED_Z_LENGTH + Z_OFFSET)) as i32
    }

    #[inline]
    pub const fn as_long(self) -> i64 {
        Self::pack(self.x, self.y, self.z)
    }

    /// Packed long for the section containing `pos`.
    #[inline]
    pub const fn as_long_of_block(pos: BlockPos) -> i64 {
        Self::of_block(pos).as_long()
    }

    #[inline]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> SectionPos {
        if dx == 0 && dy == 0 && dz == 0 {
            self
        } else {
            SectionPos::of(
                self.x.wrapping_add(dx),
                self.y.wrapping_add(dy),
                self.z.wrapping_add(dz),
            )
        }
    }

    /// Moves one section in the given direction.
    #[inline]
    pub const fn relative(self, direction: Direction) -> SectionPos {
        self.offset(direction.step_x(), direction.step_y(), direction.step_z())
    }

    /// Moves `distance` sections in the given direction.
    #[inline]
    pub const fn relative_n(self, direction: Direction, distance: i32) -> SectionPos {
        if distance == 0 {
            self
        } else {
            self.offset(
                direction.step_x().wrapping_mul(distance),
                direction.step_y().wrapping_mul(distance),
                direction.step_z().wrapping_mul(distance),
            )
        }
    }

    /// Minimum block position of this section (`SectionPos.origin()`).
    #[inline]
    pub const fn origin(self) -> BlockPos {
        BlockPos::new(
            section_to_block(self.x),
            section_to_block(self.y),
            section_to_block(self.z),
        )
    }

    /// Center block position of this section (`SectionPos.center()`).
    #[inline]
    pub const fn center(self) -> BlockPos {
        BlockPos::new(
            section_to_block(self.x).wrapping_add(SECTION_HALF_SIZE),
            section_to_block(self.y).wrapping_add(SECTION_HALF_SIZE),
            section_to_block(self.z).wrapping_add(SECTION_HALF_SIZE),
        )
    }

    #[inline]
    pub const fn block_to_section(block_coord: i32) -> i32 {
        crate::section_pos::block_to_section(block_coord)
    }

    #[inline]
    pub const fn section_to_block(section_coord: i32) -> i32 {
        crate::section_pos::section_to_block(section_coord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_section_conversions() {
        assert_eq!(block_to_section(0), 0);
        assert_eq!(block_to_section(15), 0);
        assert_eq!(block_to_section(16), 1);
        assert_eq!(block_to_section(-1), -1);
        assert_eq!(block_to_section(-16), -1);
        assert_eq!(block_to_section(-17), -2);
        assert_eq!(SectionPos::block_to_section(33), 2);

        assert_eq!(section_to_block(0), 0);
        assert_eq!(section_to_block(1), 16);
        assert_eq!(section_to_block(-1), -16);
        assert_eq!(section_to_block(-2), -32);
        assert_eq!(SectionPos::section_to_block(3), 48);
    }

    #[test]
    fn section_relative_coordinates() {
        assert_eq!(section_relative(0), 0);
        assert_eq!(section_relative(15), 15);
        assert_eq!(section_relative(16), 0);
        assert_eq!(section_relative(-1), 15);
        assert_eq!(section_relative(-16), 0);
        assert_eq!(section_relative(-17), 15);

        for block in -64..=64 {
            assert_eq!(
                block_to_section(block) * 16 + section_relative(block),
                block
            );
        }
    }

    #[test]
    fn of_block_and_origin_round_trip() {
        let positions = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(15, -1, -16),
            BlockPos::new(16, 31, 32),
            BlockPos::new(-1, -17, -33),
        ];
        for pos in positions {
            let section = SectionPos::of_block(pos);
            assert_eq!(section.x(), block_to_section(pos.x()));
            assert_eq!(section.y(), block_to_section(pos.y()));
            assert_eq!(section.z(), block_to_section(pos.z()));
            assert_eq!(section.as_long(), SectionPos::as_long_of_block(pos));

            let origin = section.origin();
            assert_eq!(origin.x(), section.x() * 16);
            assert_eq!(origin.y(), section.y() * 16);
            assert_eq!(origin.z(), section.z() * 16);
            assert_eq!(
                section.center(),
                BlockPos::new(
                    section.x() * 16 + 8,
                    section.y() * 16 + 8,
                    section.z() * 16 + 8
                )
            );

            // The original block is inside the section's block range.
            assert_eq!(block_to_section(pos.x()), section.x());
            assert_eq!(block_to_section(pos.y()), section.y());
            assert_eq!(block_to_section(pos.z()), section.z());
        }
    }

    #[test]
    fn as_long_known_values() {
        assert_eq!(SectionPos::of(0, 0, 0).as_long(), 0);
        assert_eq!(SectionPos::of(1, 0, 0).as_long(), 4_398_046_511_104);
        assert_eq!(SectionPos::of(0, 1, 0).as_long(), 1);
        assert_eq!(SectionPos::of(0, 0, 1).as_long(), 1_048_576);
        assert_eq!(SectionPos::of(-1, 0, 0).as_long(), -4_398_046_511_104);
        assert_eq!(SectionPos::of(0, -1, 0).as_long(), 1_048_575);
        assert_eq!(SectionPos::of(0, 0, -1).as_long(), 4_398_045_462_528);
        assert_eq!(SectionPos::of(1, 2, 3).as_long(), 4_398_049_656_834);
        assert_eq!(SectionPos::of(-1, -2, -3).as_long(), -2_097_154);
    }

    #[test]
    fn as_long_extreme_values() {
        assert_eq!(
            SectionPos::of(2_097_151, 524_287, 2_097_151).as_long(),
            9_223_369_837_830_995_967
        );
        assert_eq!(
            SectionPos::of(-2_097_152, -524_288, -2_097_152).as_long(),
            -9_223_369_837_830_995_968
        );
    }

    #[test]
    fn from_long_known_values() {
        assert_eq!(SectionPos::from_long(0), SectionPos::of(0, 0, 0));
        assert_eq!(SectionPos::from_long(1), SectionPos::of(0, 1, 0));
        assert_eq!(SectionPos::from_long(1_048_576), SectionPos::of(0, 0, 1));
        assert_eq!(
            SectionPos::from_long(4_398_046_511_104),
            SectionPos::of(1, 0, 0)
        );
        assert_eq!(
            SectionPos::from_long(-4_398_046_511_104),
            SectionPos::of(-1, 0, 0)
        );
        assert_eq!(SectionPos::from_long(1_048_575), SectionPos::of(0, -1, 0));
        assert_eq!(
            SectionPos::from_long(4_398_045_462_528),
            SectionPos::of(0, 0, -1)
        );
        assert_eq!(
            SectionPos::from_long(4_398_049_656_834),
            SectionPos::of(1, 2, 3)
        );
        assert_eq!(
            SectionPos::from_long(-2_097_154),
            SectionPos::of(-1, -2, -3)
        );
        assert_eq!(SectionPos::from_long(-1), SectionPos::of(-1, -1, -1));
    }

    #[test]
    fn packed_round_trip_for_negative_and_extreme_coordinates() {
        let cases = [
            SectionPos::of(0, 0, 0),
            SectionPos::of(1, 2, 3),
            SectionPos::of(-1, -2, -3),
            SectionPos::of(2_097_151, 524_287, 2_097_151),
            SectionPos::of(-2_097_152, -524_288, -2_097_152),
            SectionPos::of(123_456, -789, -987_654),
            SectionPos::of(-123_456, 789, 987_654),
        ];
        for pos in cases {
            assert_eq!(SectionPos::from_long(pos.as_long()), pos, "pos={pos:?}");
            assert_eq!(
                SectionPos::from_long(SectionPos::pack(pos.x(), pos.y(), pos.z())),
                pos
            );
        }
    }

    #[test]
    fn unpack_helpers_match_from_long() {
        let packed = SectionPos::pack(-12_345, 678, 90_123);
        assert_eq!(SectionPos::x_from_long(packed), -12_345);
        assert_eq!(SectionPos::y_from_long(packed), 678);
        assert_eq!(SectionPos::z_from_long(packed), 90_123);
        assert_eq!(
            SectionPos::from_long(packed),
            SectionPos::of(-12_345, 678, 90_123)
        );
    }

    #[test]
    fn offset_and_relative() {
        let pos = SectionPos::of(4, 5, 6);
        assert_eq!(pos.offset(1, -2, 3), SectionPos::of(5, 3, 9));
        assert_eq!(pos.offset(0, 0, 0), pos);
        assert_eq!(pos.relative(Direction::Up), SectionPos::of(4, 6, 6));
        assert_eq!(pos.relative(Direction::Down), SectionPos::of(4, 4, 6));
        assert_eq!(pos.relative(Direction::North), SectionPos::of(4, 5, 5));
        assert_eq!(pos.relative(Direction::South), SectionPos::of(4, 5, 7));
        assert_eq!(pos.relative(Direction::West), SectionPos::of(3, 5, 6));
        assert_eq!(pos.relative(Direction::East), SectionPos::of(5, 5, 6));
        assert_eq!(pos.relative_n(Direction::East, 4), SectionPos::of(8, 5, 6));
        assert_eq!(pos.relative_n(Direction::West, 4), SectionPos::of(0, 5, 6));
        assert_eq!(pos.relative_n(Direction::Up, 0), pos);
    }

    #[test]
    fn offset_wraps_like_java() {
        assert_eq!(
            SectionPos::of(i32::MAX, 0, 0).offset(1, 0, 0),
            SectionPos::of(i32::MIN, 0, 0)
        );
    }

    #[test]
    fn pos_to_section_floors_before_shifting() {
        assert_eq!(pos_to_section(0.0), 0);
        assert_eq!(pos_to_section(15.99), 0);
        assert_eq!(pos_to_section(16.0), 1);
        assert_eq!(pos_to_section(-0.01), -1);
        assert_eq!(pos_to_section(-16.0), -1);
        assert_eq!(pos_to_section(-16.01), -2);
    }
}
