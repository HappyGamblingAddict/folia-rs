//! Port of `Aquifer.NoiseBasedAquifer` (1.21.11).
//!
//! Aquifers decide whether a position below the density surface is air, water
//! or lava. The implementation follows the vanilla grid, distance blending and
//! pressure rules, using the embedded overworld noise router.

use crate::chunk::BlockState;
use crate::worldgen::density::{DensityFunction, NoiseRouter};
use folia_random::{RandomSource, XoroshiroPositionalRandomFactory};
use std::collections::HashMap;
use std::sync::Arc;

const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;
const SAMPLE_OFFSET_X: i32 = -5;
const SAMPLE_OFFSET_Y: i32 = 1;
const SAMPLE_OFFSET_Z: i32 = -5;

const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [
    [0, 0],
    [-2, -1],
    [-1, -1],
    [0, -1],
    [1, -1],
    [-3, 0],
    [-2, 0],
    [-1, 0],
    [1, 0],
    [-2, 1],
    [-1, 1],
    [0, 1],
    [1, 1],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidKind {
    Water,
    Lava,
}

impl FluidKind {
    fn block_state(self) -> BlockState {
        match self {
            FluidKind::Water => BlockState {
                name: "minecraft:water".into(),
                properties: vec![("level".into(), "0".into())],
            },
            FluidKind::Lava => BlockState {
                name: "minecraft:lava".into(),
                properties: vec![("level".into(), "0".into())],
            },
        }
    }
}

fn block_state_for(kind: Option<FluidKind>) -> BlockState {
    kind.map(FluidKind::block_state)
        .unwrap_or_else(|| BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStatus {
    pub fluid_level: i32,
    pub fluid_type: FluidKind,
}

impl FluidStatus {
    fn at(self, y: i32) -> Option<FluidKind> {
        if y < self.fluid_level {
            Some(self.fluid_type)
        } else {
            None
        }
    }
}

pub struct Aquifer {
    barrier: Arc<DensityFunction>,
    floodedness: Arc<DensityFunction>,
    spread: Arc<DensityFunction>,
    lava: Arc<DensityFunction>,
    erosion: Arc<DensityFunction>,
    depth: Arc<DensityFunction>,
    preliminary_surface: Arc<DensityFunction>,
    positional: XoroshiroPositionalRandomFactory,
    global_sea_level: i32,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    aquifer_cache: Vec<Option<FluidStatus>>,
    aquifer_locations: Vec<i64>,
    skip_sampling_above_y: i32,
    preliminary_cache: HashMap<(i32, i32), i32>,
}

impl Aquifer {
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        router: &NoiseRouter,
        positional: &XoroshiroPositionalRandomFactory,
        min_y: i32,
        height: i32,
        sea_level: i32,
    ) -> Self {
        let barrier = router.function("barrier").expect("barrier").clone();
        let floodedness = router
            .function("fluid_level_floodedness")
            .expect("floodedness")
            .clone();
        let spread = router
            .function("fluid_level_spread")
            .expect("spread")
            .clone();
        let lava = router.function("lava").expect("lava").clone();
        let erosion = router.function("erosion").expect("erosion").clone();
        let depth = router.function("depth").expect("depth").clone();
        let preliminary_surface = router
            .function("preliminary_surface_level")
            .expect("preliminary surface")
            .clone();

        let min_block_x = chunk_x * 16;
        let min_block_z = chunk_z * 16;
        let max_block_x = min_block_x + 15;
        let max_block_z = min_block_z + 15;

        let min_grid_x = grid_x(min_block_x + SAMPLE_OFFSET_X);
        let max_grid_x = grid_x(max_block_x + SAMPLE_OFFSET_X) + 1;
        let grid_size_x = max_grid_x - min_grid_x + 1;

        let min_grid_y = grid_y(min_y + SAMPLE_OFFSET_Y) - 1;
        let max_grid_y = grid_y(min_y + height + SAMPLE_OFFSET_Y) + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;

        let min_grid_z = grid_z(min_block_z + SAMPLE_OFFSET_Z);
        let max_grid_z = grid_z(max_block_z + SAMPLE_OFFSET_Z) + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;

        let cache_size = (grid_size_x * grid_size_y * grid_size_z) as usize;
        let mut aquifer = Self {
            barrier,
            floodedness,
            spread,
            lava,
            erosion,
            depth,
            preliminary_surface,
            positional: *positional,
            global_sea_level: sea_level,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_z,
            aquifer_cache: vec![None; cache_size],
            aquifer_locations: vec![i64::MAX; cache_size],
            skip_sampling_above_y: 0,
            preliminary_cache: HashMap::new(),
        };

        let max_surface = aquifer.max_preliminary_surface_level(
            from_grid_x(min_grid_x, 0),
            from_grid_z(min_grid_z, 0),
            from_grid_x(max_grid_x, 9),
            from_grid_z(max_grid_z, 9),
        );
        let adjusted = aquifer.adjust_surface_level(max_surface);
        let max_y_grid = grid_y(adjusted + 12) + 1;
        aquifer.skip_sampling_above_y = from_grid_y(max_y_grid, 11) - 1;
        aquifer
    }

    /// Returns `None` for solid terrain, and `Some(air/water/lava)` otherwise.
    pub fn compute_substance(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        substance: f64,
    ) -> Option<BlockState> {
        if substance > 0.0 {
            return None;
        }

        let global = self.global_fluid(x, y, z);
        if y > self.skip_sampling_above_y {
            return Some(block_state_for(global.at(y)));
        }
        if global.at(y) == Some(FluidKind::Lava) {
            return Some(FluidKind::Lava.block_state());
        }

        let grid_x = grid_x(x - 5);
        let grid_y = grid_y(y + 1);
        let grid_z = grid_z(z - 5);

        let mut first_distance = i32::MAX;
        let mut second_distance = i32::MAX;
        let mut third_distance = i32::MAX;
        let mut fourth_distance = i32::MAX;
        let mut first_index = 0usize;
        let mut second_index = 0usize;
        let mut third_index = 0usize;
        let mut fourth_index = 0usize;

        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let gx = grid_x + dx;
                    let gy = grid_y + dy;
                    let gz = grid_z + dz;
                    let index = self.index(gx, gy, gz);
                    let mut location = self.aquifer_locations[index];
                    if location == i64::MAX {
                        let mut random = self.positional.at(gx, gy, gz);
                        let bx = from_grid_x(gx, random.next_int_bound(X_RANGE));
                        let by = from_grid_y(gy, random.next_int_bound(Y_RANGE));
                        let bz = from_grid_z(gz, random.next_int_bound(Z_RANGE));
                        location = pack_block_pos(bx, by, bz);
                        self.aquifer_locations[index] = location;
                    }
                    let dx2 = unpack_x(location) - x;
                    let dy2 = unpack_y(location) - y;
                    let dz2 = unpack_z(location) - z;
                    let distance = dx2 * dx2 + dy2 * dy2 + dz2 * dz2;

                    if first_distance >= distance {
                        fourth_index = third_index;
                        third_index = second_index;
                        second_index = first_index;
                        first_index = index;
                        fourth_distance = third_distance;
                        third_distance = second_distance;
                        second_distance = first_distance;
                        first_distance = distance;
                    } else if second_distance >= distance {
                        fourth_index = third_index;
                        third_index = second_index;
                        second_index = index;
                        fourth_distance = third_distance;
                        third_distance = second_distance;
                        second_distance = distance;
                    } else if third_distance >= distance {
                        fourth_index = third_index;
                        third_index = index;
                        fourth_distance = third_distance;
                        third_distance = distance;
                    } else if fourth_distance >= distance {
                        fourth_index = index;
                        fourth_distance = distance;
                    }
                }
            }
        }

        let status = self.status(first_index);
        let center_similarity = similarity(first_distance, second_distance);
        let block = status.at(y);
        if center_similarity <= 0.0 {
            return Some(block_state_for(block));
        }

        if block == Some(FluidKind::Water)
            && self.global_fluid(x, y - 1, z).at(y - 1) == Some(FluidKind::Lava)
        {
            return Some(FluidKind::Water.block_state());
        }

        let mut barrier = None;
        let second_status = self.status(second_index);
        let pressure = center_similarity
            * self.calculate_pressure(x, y, z, &mut barrier, status, second_status);
        if substance + pressure > 0.0 {
            return None;
        }

        let third_status = self.status(third_index);
        let second_similarity = similarity(first_distance, third_distance);
        if second_similarity > 0.0 {
            let pressure = center_similarity
                * second_similarity
                * self.calculate_pressure(x, y, z, &mut barrier, status, third_status);
            if substance + pressure > 0.0 {
                return None;
            }
        }

        let third_similarity = similarity(second_distance, third_distance);
        if third_similarity > 0.0 {
            let pressure = center_similarity
                * third_similarity
                * self.calculate_pressure(x, y, z, &mut barrier, second_status, third_status);
            if substance + pressure > 0.0 {
                return None;
            }
        }

        // Flowing-update scheduling is not needed for static chunk generation.
        let _ = fourth_index;
        Some(block_state_for(block))
    }

    fn calculate_pressure(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut Option<f64>,
        first: FluidStatus,
        second: FluidStatus,
    ) -> f64 {
        let first_fluid = first.at(y);
        let second_fluid = second.at(y);
        let opposite = (first_fluid == Some(FluidKind::Lava)
            && second_fluid == Some(FluidKind::Water))
            || (first_fluid == Some(FluidKind::Water) && second_fluid == Some(FluidKind::Lava));
        if opposite {
            return 2.0;
        }

        let difference = (first.fluid_level - second.fluid_level).abs();
        if difference == 0 {
            return 0.0;
        }

        let average_level = 0.5 * f64::from(first.fluid_level + second.fluid_level);
        let delta = f64::from(y) + 0.5 - average_level;
        let half_difference = f64::from(difference) / 2.0;
        let gap = half_difference - delta.abs();
        let pressure = if delta > 0.0 {
            if gap > 0.0 { gap / 1.5 } else { gap / 2.5 }
        } else if gap > 0.0 {
            gap / 3.0
        } else {
            gap / 10.0
        };

        let barrier_value = if (-2.0..=2.0).contains(&pressure) {
            *barrier.get_or_insert_with(|| self.barrier.compute(x, y, z))
        } else {
            0.0
        };

        // The barrier sampling context is the current block position.
        2.0 * (barrier_value + pressure)
    }

    fn status(&mut self, index: usize) -> FluidStatus {
        if let Some(status) = self.aquifer_cache[index] {
            return status;
        }
        let location = self.aquifer_locations[index];
        let status = self.compute_fluid(unpack_x(location), unpack_y(location), unpack_z(location));
        self.aquifer_cache[index] = Some(status);
        status
    }

    fn compute_fluid(&mut self, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.global_fluid(x, y, z);
        let mut min_surface = i32::MAX;
        let above = y + 12;
        let below = y - 12;
        let mut fluid_present = false;

        for offset in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sx = x + offset[0] * 16;
            let sz = z + offset[1] * 16;
            let surface = self.preliminary_surface_level(sx, sz);
            let adjusted_surface = self.adjust_surface_level(surface);
            let center = offset[0] == 0 && offset[1] == 0;
            if center && below > adjusted_surface {
                return global;
            }

            let above_surface = above > adjusted_surface;
            if above_surface || center {
                let status = self.global_fluid(sx, adjusted_surface, sz);
                if status.at(adjusted_surface).is_some() {
                    if center {
                        fluid_present = true;
                    }
                    if above_surface {
                        return status;
                    }
                }
            }
            min_surface = min_surface.min(surface);
        }

        let level = self.compute_surface_level(x, y, z, global, min_surface, fluid_present);
        let kind = self.compute_fluid_type(x, y, z, global, level);
        FluidStatus {
            fluid_level: level,
            fluid_type: kind,
        }
    }

    fn compute_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        max_surface_level: i32,
        fluid_present: bool,
    ) -> i32 {
        let (upper, lower) = if self.is_deep_dark(x, y, z) {
            (-1.0, -1.0)
        } else {
            let distance = max_surface_level + 8 - y;
            let closeness = if fluid_present {
                clamped_map(f64::from(distance), 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let floodedness = self.floodedness.compute(x, y, z).clamp(-1.0, 1.0);
            let d4 = map(closeness, 1.0, 0.0, -0.3, 0.8);
            let d5 = map(closeness, 1.0, 0.0, -0.8, 0.4);
            (floodedness - d5, floodedness - d4)
        };

        if lower > 0.0 {
            global.fluid_level
        } else if upper > 0.0 {
            self.compute_randomized_surface_level(x, y, z, max_surface_level)
        } else {
            -32512
        }
    }

    fn compute_randomized_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        max_surface_level: i32,
    ) -> i32 {
        let grid_x = x.div_euclid(16);
        let grid_y = y.div_euclid(40);
        let grid_z = z.div_euclid(16);
        let base = grid_y * 40 + 20;
        let spread = self.spread.compute(grid_x, grid_y, grid_z) * 10.0;
        let quantized = quantize(spread, 3);
        (base + quantized).min(max_surface_level)
    }

    fn compute_fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        surface: i32,
    ) -> FluidKind {
        if surface <= -10 && surface != -32512 && global.fluid_type != FluidKind::Lava {
            let lava = self
                .lava
                .compute(x.div_euclid(64), y.div_euclid(40), z.div_euclid(64));
            if lava.abs() > 0.3 {
                return FluidKind::Lava;
            }
        }
        global.fluid_type
    }

    fn is_deep_dark(&self, x: i32, y: i32, z: i32) -> bool {
        self.erosion.compute(x, y, z) < -0.225 && self.depth.compute(x, y, z) > 0.9
    }

    fn preliminary_surface_level(&mut self, x: i32, z: i32) -> i32 {
        let qx = x & !3;
        let qz = z & !3;
        if let Some(level) = self.preliminary_cache.get(&(qx, qz)) {
            return *level;
        }
        let level = self.preliminary_surface.compute(qx, 0, qz).floor() as i32;
        self.preliminary_cache.insert((qx, qz), level);
        level
    }

    fn max_preliminary_surface_level(
        &mut self,
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
    ) -> i32 {
        let mut max = i32::MIN;
        let mut z = min_z;
        while z <= max_z {
            let mut x = min_x;
            while x <= max_x {
                max = max.max(self.preliminary_surface_level(x, z));
                x += 4;
            }
            z += 4;
        }
        max
    }

    fn adjust_surface_level(&self, level: i32) -> i32 {
        level + 8
    }

    fn global_fluid(&self, _x: i32, y: i32, _z: i32) -> FluidStatus {
        if y < (-54).min(self.global_sea_level) {
            FluidStatus {
                fluid_level: -54,
                fluid_type: FluidKind::Lava,
            }
        } else {
            FluidStatus {
                fluid_level: self.global_sea_level,
                fluid_type: FluidKind::Water,
            }
        }
    }

    fn index(&self, grid_x: i32, grid_y: i32, grid_z: i32) -> usize {
        let dx = grid_x - self.min_grid_x;
        let dy = grid_y - self.min_grid_y;
        let dz = grid_z - self.min_grid_z;
        ((dy * self.grid_size_z + dz) * self.grid_size_x + dx) as usize
    }
}

fn grid_x(x: i32) -> i32 {
    x >> 4
}

fn from_grid_x(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

fn grid_y(y: i32) -> i32 {
    y.div_euclid(12)
}

fn from_grid_y(grid: i32, offset: i32) -> i32 {
    grid * 12 + offset
}

fn grid_z(z: i32) -> i32 {
    z >> 4
}

fn from_grid_z(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

fn similarity(first: i32, second: i32) -> f64 {
    1.0 - f64::from(second - first) / 25.0
}

fn map(input: f64, input_min: f64, input_max: f64, output_min: f64, output_max: f64) -> f64 {
    let inverse = (input - input_min) / (input_max - input_min);
    output_min + inverse * (output_max - output_min)
}

fn clamped_map(
    input: f64,
    input_min: f64,
    input_max: f64,
    output_min: f64,
    output_max: f64,
) -> f64 {
    let inverse = (input - input_min) / (input_max - input_min);
    if inverse < 0.0 {
        output_min
    } else if inverse > 1.0 {
        output_max
    } else {
        output_min + inverse * (output_max - output_min)
    }
}

fn quantize(value: f64, factor: i32) -> i32 {
    (value / f64::from(factor)).floor() as i32 * factor
}

fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    ((i64::from(x) & 0x3FF_FFFF) << 38)
        | (i64::from(y) & 0xFFF)
        | ((i64::from(z) & 0x3FF_FFFF) << 12)
}

fn unpack_x(packed: i64) -> i32 {
    (packed >> 38) as i32
}

fn unpack_y(packed: i64) -> i32 {
    ((packed << 52) >> 52) as i32
}

fn unpack_z(packed: i64) -> i32 {
    ((packed << 26) >> 38) as i32
}
