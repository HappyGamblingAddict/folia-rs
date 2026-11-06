//! Port of `net.minecraft.world.level.levelgen.carver` for the overworld.
//!
//! Vanilla carves a target chunk by replaying the carver starts of every chunk
//! in a 17x17 neighbourhood: a tunnel that starts in a neighbouring chunk can
//! still reach into this one. The algorithms, random draws, and float/Double
//! evaluation order below follow `WorldCarver`, `CaveWorldCarver`, and
//! `CanyonWorldCarver` one-to-one, including the legacy large-feature seed.

use crate::chunk::{BlockState, Chunk};
use crate::worldgen::aquifer::{Aquifer, AquiferSubstance};
use crate::worldgen::biome::BiomeSampler;
use crate::worldgen::density::DensityFunction;
use crate::worldgen::surface::system::SurfaceSystem;
use crate::worldgen::surface::{self, RuleSource};
use folia_random::{LegacyRandomSource, RandomSource};

/// Java's `Mth.SIN` table, generated from `(float)Math.sin(i / 10430.378...)`.
/// Embedding it keeps carver shapes bit-identical across platforms instead of
/// depending on the host libm's rounding.
const SIN_TABLE: &[u8; 65536 * 4] = include_bytes!("../../assets/mth_sin.bin");
const SIN_SCALE: f64 = 10430.378350470453;
const SIN_MASK: i64 = 65535;
const COS_OFFSET: f64 = 16384.0;

const PI_F32: f32 = std::f32::consts::PI;
const HALF_PI_F32: f32 = std::f32::consts::FRAC_PI_2;
const TWO_PI_F32: f32 = std::f32::consts::TAU;

fn sin_at(index: i64) -> f32 {
    let index = (index & SIN_MASK) as usize;
    let offset = index * 4;
    f32::from_le_bytes([
        SIN_TABLE[offset],
        SIN_TABLE[offset + 1],
        SIN_TABLE[offset + 2],
        SIN_TABLE[offset + 3],
    ])
}

/// Java: `Mth.sin(double)`.
pub(crate) fn mth_sin(value: f64) -> f32 {
    sin_at((value * SIN_SCALE) as i64)
}

/// Java: `Mth.cos(double)`.
pub(crate) fn mth_cos(value: f64) -> f32 {
    sin_at((value * SIN_SCALE + COS_OFFSET) as i64)
}

/// Java: `Mth.randomBetween(RandomSource, float, float)`.
fn mth_random_between(random: &mut impl RandomSource, min: f32, max: f32) -> f32 {
    random.next_float() * (max - min) + min
}

/// Java: `Mth.floor(double)`.
fn mth_floor(value: f64) -> i32 {
    let truncated = value as i32;
    if value < f64::from(truncated) {
        truncated - 1
    } else {
        truncated
    }
}

/// A `FloatProvider` from the carver data pack. Only the providers used by the
/// overworld carvers are implemented.
#[derive(Debug, Clone, Copy)]
enum FloatProvider {
    Constant(f32),
    Uniform { min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatProvider {
    fn sample(self, random: &mut impl RandomSource) -> f32 {
        match self {
            Self::Constant(value) => value,
            Self::Uniform { min, max } => mth_random_between(random, min, max),
            Self::Trapezoid { min, max, plateau } => {
                let span = max - min;
                let slope = (span - plateau) / 2.0;
                let flat_side = span - slope;
                min + random.next_float() * flat_side + random.next_float() * slope
            }
        }
    }
}

/// `UniformHeight` reduced to its resolved absolute block range.
#[derive(Debug, Clone, Copy)]
struct UniformHeight {
    min: i32,
    max: i32,
}

impl UniformHeight {
    fn sample(self, random: &mut impl RandomSource) -> i32 {
        if self.min > self.max {
            self.min
        } else {
            random.next_int_between_inclusive(self.min, self.max)
        }
    }
}

/// Reproduces `WorldgenRandom.setLargeFeatureSeed`.
fn set_large_feature_seed(
    random: &mut LegacyRandomSource,
    base_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
) {
    random.set_seed(base_seed);
    let first = random.next_long();
    let second = random.next_long();
    let seed = i64::from(chunk_x).wrapping_mul(first)
        ^ i64::from(chunk_z).wrapping_mul(second)
        ^ base_seed;
    random.set_seed(seed);
}

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn air() -> BlockState {
    block("minecraft:air")
}

fn lava() -> BlockState {
    BlockState {
        name: "minecraft:lava".into(),
        properties: vec![("level".into(), "0".into())],
    }
}

fn is_fluid(state: &BlockState) -> bool {
    matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
}

/// Expanded `#minecraft:overworld_carver_replaceables`.
fn can_replace(name: &str) -> bool {
    matches!(
        name,
        "minecraft:stone"
            | "minecraft:deepslate"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:tuff"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:grass_block"
            | "minecraft:podzol"
            | "minecraft:mycelium"
            | "minecraft:moss_block"
            | "minecraft:pale_moss_block"
            | "minecraft:mud"
            | "minecraft:muddy_mangrove_roots"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:sandstone"
            | "minecraft:red_sandstone"
            | "minecraft:suspicious_sand"
            | "minecraft:gravel"
            | "minecraft:suspicious_gravel"
            | "minecraft:terracotta"
            | "minecraft:white_terracotta"
            | "minecraft:orange_terracotta"
            | "minecraft:magenta_terracotta"
            | "minecraft:light_blue_terracotta"
            | "minecraft:yellow_terracotta"
            | "minecraft:lime_terracotta"
            | "minecraft:pink_terracotta"
            | "minecraft:gray_terracotta"
            | "minecraft:light_gray_terracotta"
            | "minecraft:cyan_terracotta"
            | "minecraft:purple_terracotta"
            | "minecraft:blue_terracotta"
            | "minecraft:brown_terracotta"
            | "minecraft:green_terracotta"
            | "minecraft:red_terracotta"
            | "minecraft:black_terracotta"
            | "minecraft:iron_ore"
            | "minecraft:deepslate_iron_ore"
            | "minecraft:copper_ore"
            | "minecraft:deepslate_copper_ore"
            | "minecraft:raw_iron_block"
            | "minecraft:raw_copper_block"
            | "minecraft:snow"
            | "minecraft:snow_block"
            | "minecraft:powder_snow"
            | "minecraft:calcite"
            | "minecraft:packed_ice"
            | "minecraft:water"
    )
}

/// `CarvingMask`: a bitset over a 16x16xheight chunk volume. The mask is shared
/// by all carver starts that write into the target chunk.
struct CarvingMask {
    min_y: i32,
    height: i32,
    bits: Vec<u64>,
}

impl CarvingMask {
    fn new(min_y: i32, height: i32) -> Self {
        let blocks = 16 * 16 * height as usize;
        Self {
            min_y,
            height,
            bits: vec![0; blocks.div_ceil(64)],
        }
    }

    fn index(&self, x: usize, y: i32, z: usize) -> Option<usize> {
        let local_y = y - self.min_y;
        if local_y < 0 || local_y >= self.height {
            return None;
        }
        Some((x & 15) | ((z & 15) << 4) | ((local_y as usize) << 8))
    }

    fn get(&self, x: usize, y: i32, z: usize) -> bool {
        let Some(index) = self.index(x, y, z) else {
            return false;
        };
        let word = self.bits.get(index >> 6).copied().unwrap_or(0);
        word & (1u64 << (index & 63)) != 0
    }

    fn set(&mut self, x: usize, y: i32, z: usize) {
        let Some(index) = self.index(x, y, z) else {
            return;
        };
        if let Some(word) = self.bits.get_mut(index >> 6) {
            *word |= 1u64 << (index & 63);
        }
    }
}

/// State shared by every carver replay: the generation bounds plus the surface
/// machinery needed for `WorldCarver.carveBlock`'s `topMaterial` call.
pub struct CarverContext<'a> {
    min_y: i32,
    height: i32,
    surface: &'a SurfaceSystem,
    rules: &'a RuleSource,
    preliminary_surface: &'a DensityFunction,
    biomes: &'a BiomeSampler,
    biome_indices: &'a [usize],
}

impl<'a> CarverContext<'a> {
    pub fn new(
        surface: &'a SurfaceSystem,
        rules: &'a RuleSource,
        preliminary_surface: &'a DensityFunction,
        biomes: &'a BiomeSampler,
        biome_indices: &'a [usize],
        min_y: i32,
        height: i32,
    ) -> Self {
        Self {
            min_y,
            height,
            surface,
            rules,
            preliminary_surface,
            biomes,
            biome_indices,
        }
    }

    fn top_material(
        &self,
        chunk: &Chunk,
        x: i32,
        y: i32,
        z: i32,
        has_fluid: bool,
    ) -> Option<BlockState> {
        surface::top_material(
            self.surface,
            self.rules,
            self.preliminary_surface,
            self.biomes,
            self.biome_indices,
            chunk,
            x,
            y,
            z,
            has_fluid,
        )
    }
}

/// Chunk coordinates a carver is applied to. Grouped so the shared carve
/// signatures stay readable.
#[derive(Debug, Clone, Copy)]
struct ChunkOrigin {
    x: i32,
    z: i32,
}

#[derive(Debug, Clone, Copy)]
struct CaveCarver {
    probability: f32,
    y: UniformHeight,
    y_scale: FloatProvider,
    lava_level: i32,
    horizontal_radius_multiplier: FloatProvider,
    vertical_radius_multiplier: FloatProvider,
    floor_level: FloatProvider,
}

impl CaveCarver {
    fn carve(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        random: &mut impl RandomSource,
        origin: ChunkOrigin,
        mask: &mut CarvingMask,
    ) {
        // Java: `SectionPos.sectionToBlockCoord(4 * 2 - 1)`.
        const BRANCH_COORD: i32 = 7 * 16;

        let inner = random.next_int_bound(15) + 1;
        let middle = random.next_int_bound(inner) + 1;
        let count = random.next_int_bound(middle);

        for _ in 0..count {
            let x = f64::from(origin.x * 16 + random.next_int_bound(16));
            let y = f64::from(self.y.sample(random));
            let z = f64::from(origin.z * 16 + random.next_int_bound(16));
            let horizontal_multiplier = f64::from(self.horizontal_radius_multiplier.sample(random));
            let vertical_multiplier = f64::from(self.vertical_radius_multiplier.sample(random));
            let floor_level = f64::from(self.floor_level.sample(random));
            let skip = CaveSkip { floor_level };

            let mut branches = 1;
            if random.next_int_bound(4) == 0 {
                let ratio = f64::from(self.y_scale.sample(random));
                let radius = 1.0 + random.next_float() * 6.0;
                self.create_room(context, chunk, aquifer, x, y, z, radius, ratio, mask, &skip);
                branches += random.next_int_bound(4);
            }

            for _ in 0..branches {
                let yaw = random.next_float() * TWO_PI_F32;
                let pitch = (random.next_float() - 0.5) / 4.0;
                let thickness = self.thickness(random);
                let branch_count = BRANCH_COORD - random.next_int_bound(BRANCH_COORD / 4);
                let seed = random.next_long();
                self.create_tunnel(
                    context,
                    chunk,
                    aquifer,
                    seed,
                    x,
                    y,
                    z,
                    horizontal_multiplier,
                    vertical_multiplier,
                    thickness,
                    yaw,
                    pitch,
                    0,
                    branch_count,
                    1.0,
                    mask,
                    &skip,
                );
            }
        }
    }

    fn thickness(&self, random: &mut impl RandomSource) -> f32 {
        let mut thickness = random.next_float() * 2.0 + random.next_float();
        if random.next_int_bound(10) == 0 {
            thickness *= random.next_float() * random.next_float() * 3.0 + 1.0;
        }
        thickness
    }

    #[allow(clippy::too_many_arguments)]
    fn create_room(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        x: f64,
        y: f64,
        z: f64,
        radius: f32,
        ratio: f64,
        mask: &mut CarvingMask,
        skip: &CaveSkip,
    ) {
        let horizontal = 1.5 + f64::from(mth_sin(f64::from(HALF_PI_F32)) * radius);
        let vertical = horizontal * ratio;
        carve_ellipsoid(
            context,
            self.lava_level,
            chunk,
            aquifer,
            x + 1.0,
            y,
            z,
            horizontal,
            vertical,
            mask,
            |rx, ry, rz, _| skip.should_skip(rx, ry, rz),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn create_tunnel(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        horizontal_multiplier: f64,
        vertical_multiplier: f64,
        thickness: f32,
        mut yaw: f32,
        mut pitch: f32,
        branch_index: i32,
        branch_count: i32,
        ratio: f64,
        mask: &mut CarvingMask,
        skip: &CaveSkip,
    ) {
        let mut random = LegacyRandomSource::new(seed);
        let split = random.next_int_bound(branch_count / 2) + branch_count / 4;
        let steep = random.next_int_bound(6) == 0;
        let mut yaw_drift = 0.0f32;
        let mut pitch_drift = 0.0f32;

        for branch in branch_index..branch_count {
            let horizontal = 1.5
                + f64::from(
                    mth_sin(f64::from((PI_F32 * branch as f32) / branch_count as f32)) * thickness,
                );
            let vertical = horizontal * ratio;
            let cos_pitch = mth_cos(f64::from(pitch));
            x += f64::from(mth_cos(f64::from(yaw)) * cos_pitch);
            y += f64::from(mth_sin(f64::from(pitch)));
            z += f64::from(mth_sin(f64::from(yaw)) * cos_pitch);
            pitch *= if steep { 0.92 } else { 0.7 };
            pitch += pitch_drift * 0.1;
            yaw += yaw_drift * 0.1;
            pitch_drift *= 0.9;
            yaw_drift *= 0.75;
            pitch_drift += (random.next_float() - random.next_float()) * random.next_float() * 2.0;
            yaw_drift += (random.next_float() - random.next_float()) * random.next_float() * 4.0;

            if branch == split && thickness > 1.0 {
                let first_seed = random.next_long();
                let first_thickness = random.next_float() * 0.5 + 0.5;
                self.create_tunnel(
                    context,
                    chunk,
                    aquifer,
                    first_seed,
                    x,
                    y,
                    z,
                    horizontal_multiplier,
                    vertical_multiplier,
                    first_thickness,
                    yaw - HALF_PI_F32,
                    pitch / 3.0,
                    branch,
                    branch_count,
                    1.0,
                    mask,
                    skip,
                );
                let second_seed = random.next_long();
                let second_thickness = random.next_float() * 0.5 + 0.5;
                self.create_tunnel(
                    context,
                    chunk,
                    aquifer,
                    second_seed,
                    x,
                    y,
                    z,
                    horizontal_multiplier,
                    vertical_multiplier,
                    second_thickness,
                    yaw + HALF_PI_F32,
                    pitch / 3.0,
                    branch,
                    branch_count,
                    1.0,
                    mask,
                    skip,
                );
                return;
            }

            if random.next_int_bound(4) != 0 {
                if !can_reach(chunk, x, z, branch, branch_count, thickness) {
                    return;
                }
                carve_ellipsoid(
                    context,
                    self.lava_level,
                    chunk,
                    aquifer,
                    x,
                    y,
                    z,
                    horizontal * horizontal_multiplier,
                    vertical * vertical_multiplier,
                    mask,
                    |rx, ry, rz, _| skip.should_skip(rx, ry, rz),
                );
            }
        }
    }
}

struct CaveSkip {
    floor_level: f64,
}

impl CaveSkip {
    fn should_skip(&self, relative_x: f64, relative_y: f64, relative_z: f64) -> bool {
        relative_y <= self.floor_level
            || relative_x * relative_x + relative_y * relative_y + relative_z * relative_z >= 1.0
    }
}

#[derive(Debug, Clone, Copy)]
struct CanyonShape {
    distance_factor: FloatProvider,
    thickness: FloatProvider,
    width_smoothness: i32,
    horizontal_radius_factor: FloatProvider,
    vertical_radius_default_factor: f32,
    vertical_radius_center_factor: f32,
}

#[derive(Debug, Clone, Copy)]
struct CanyonCarver {
    probability: f32,
    y: UniformHeight,
    y_scale: FloatProvider,
    lava_level: i32,
    vertical_rotation: FloatProvider,
    shape: CanyonShape,
}

impl CanyonCarver {
    fn carve(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        random: &mut impl RandomSource,
        origin: ChunkOrigin,
        mask: &mut CarvingMask,
    ) {
        // Java: `(4 * 2 - 1) * 16`.
        const DISTANCE: i32 = 112;

        let x = f64::from(origin.x * 16 + random.next_int_bound(16));
        let y = f64::from(self.y.sample(random));
        let z = f64::from(origin.z * 16 + random.next_int_bound(16));
        let yaw = random.next_float() * TWO_PI_F32;
        let pitch = self.vertical_rotation.sample(random);
        let ratio = f64::from(self.y_scale.sample(random));
        let thickness = self.shape.thickness.sample(random);
        let branch_count = (DISTANCE as f32 * self.shape.distance_factor.sample(random)) as i32;
        let seed = random.next_long();
        self.do_carve(
            context,
            chunk,
            aquifer,
            seed,
            x,
            y,
            z,
            thickness,
            yaw,
            pitch,
            0,
            branch_count,
            ratio,
            mask,
        );
    }

    fn init_width_factors(&self, random: &mut impl RandomSource, height: i32) -> Vec<f32> {
        let mut factors = vec![0.0f32; height as usize];
        let mut factor = 1.0f32;
        for (index, slot) in factors.iter_mut().enumerate() {
            if index == 0 || random.next_int_bound(self.shape.width_smoothness) == 0 {
                factor = 1.0 + random.next_float() * random.next_float();
            }
            *slot = factor * factor;
        }
        factors
    }

    fn update_vertical_radius(
        &self,
        random: &mut impl RandomSource,
        vertical_radius: f64,
        branch_count: f32,
        current_branch: f32,
    ) -> f64 {
        let f = 1.0 - (0.5 - current_branch / branch_count).abs() * 2.0;
        let f1 = self.shape.vertical_radius_default_factor
            + self.shape.vertical_radius_center_factor * f;
        f64::from(f1) * vertical_radius * f64::from(mth_random_between(random, 0.75, 1.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn do_carve(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        thickness: f32,
        mut yaw: f32,
        mut pitch: f32,
        branch_index: i32,
        branch_count: i32,
        ratio: f64,
        mask: &mut CarvingMask,
    ) {
        let mut random = LegacyRandomSource::new(seed);
        let widths = self.init_width_factors(&mut random, context.height);
        let mut yaw_drift = 0.0f32;
        let mut pitch_drift = 0.0f32;

        for branch in branch_index..branch_count {
            let mut horizontal = 1.5
                + f64::from(
                    mth_sin(f64::from((branch as f32 * PI_F32) / branch_count as f32)) * thickness,
                );
            let mut vertical = horizontal * ratio;
            horizontal *= f64::from(self.shape.horizontal_radius_factor.sample(&mut random));
            vertical = self.update_vertical_radius(
                &mut random,
                vertical,
                branch_count as f32,
                branch as f32,
            );
            let cos_pitch = mth_cos(f64::from(pitch));
            let sin_pitch = mth_sin(f64::from(pitch));
            x += f64::from(mth_cos(f64::from(yaw)) * cos_pitch);
            y += f64::from(sin_pitch);
            z += f64::from(mth_sin(f64::from(yaw)) * cos_pitch);
            pitch *= 0.7;
            pitch += pitch_drift * 0.05;
            yaw += yaw_drift * 0.05;
            pitch_drift *= 0.8;
            yaw_drift *= 0.5;
            pitch_drift += (random.next_float() - random.next_float()) * random.next_float() * 2.0;
            yaw_drift += (random.next_float() - random.next_float()) * random.next_float() * 4.0;

            if random.next_int_bound(4) != 0 {
                if !can_reach(chunk, x, z, branch, branch_count, thickness) {
                    return;
                }
                let min_y = context.min_y;
                carve_ellipsoid(
                    context,
                    self.lava_level,
                    chunk,
                    aquifer,
                    x,
                    y,
                    z,
                    horizontal,
                    vertical,
                    mask,
                    |relative_x, relative_y, relative_z, skip_y| {
                        let index = skip_y - min_y - 1;
                        if index < 0 || index as usize >= widths.len() {
                            return true;
                        }
                        (relative_x * relative_x + relative_z * relative_z)
                            * f64::from(widths[index as usize])
                            + relative_y * relative_y / 6.0
                            >= 1.0
                    },
                );
            }
        }
    }
}

/// Java: `WorldCarver.canReach`.
fn can_reach(
    chunk: &Chunk,
    x: f64,
    z: f64,
    branch_index: i32,
    branch_count: i32,
    width: f32,
) -> bool {
    let middle_x = f64::from(chunk.x * 16 + 8);
    let middle_z = f64::from(chunk.z * 16 + 8);
    let dx = x - middle_x;
    let dz = z - middle_z;
    let remaining = f64::from(branch_count - branch_index);
    let reach = f64::from(width + 2.0 + 16.0);
    dx * dx + dz * dz - remaining * remaining <= reach * reach
}

/// Java: `WorldCarver.carveEllipsoid`.
#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    context: &CarverContext<'_>,
    lava_level: i32,
    chunk: &mut Chunk,
    aquifer: &mut Aquifer,
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    mask: &mut CarvingMask,
    should_skip: impl Fn(f64, f64, f64, i32) -> bool,
) {
    let middle_x = f64::from(chunk.x * 16 + 8);
    let middle_z = f64::from(chunk.z * 16 + 8);
    let extent = 16.0 + horizontal_radius * 2.0;
    if (x - middle_x).abs() > extent || (z - middle_z).abs() > extent {
        return;
    }

    let min_block_x = chunk.x * 16;
    let min_block_z = chunk.z * 16;
    let min_x = (mth_floor(x - horizontal_radius) - min_block_x - 1).max(0);
    let max_x = (mth_floor(x + horizontal_radius) - min_block_x).min(15);
    let min_z = (mth_floor(z - horizontal_radius) - min_block_z - 1).max(0);
    let max_z = (mth_floor(z + horizontal_radius) - min_block_z).min(15);
    let min_y = (mth_floor(y - vertical_radius) - 1).max(context.min_y + 1);
    // `ProtoChunk.isUpgrading()` is false during normal generation, so the
    // top seven blocks are excluded from carving.
    let max_y = (mth_floor(y + vertical_radius) + 1).min(context.min_y + context.height - 1 - 7);

    for local_x in min_x..=max_x {
        let block_x = min_block_x + local_x;
        let relative_x = (f64::from(block_x) + 0.5 - x) / horizontal_radius;
        for local_z in min_z..=max_z {
            let block_z = min_block_z + local_z;
            let relative_z = (f64::from(block_z) + 0.5 - z) / horizontal_radius;
            if relative_x * relative_x + relative_z * relative_z >= 1.0 {
                continue;
            }
            let mut reached_surface = false;
            // Java loops `for (i3 = min1; i3 > max1; i3--)`, so the lower
            // bound is exclusive. `min_y` here is Java's `max1`.
            for block_y in ((min_y + 1)..=max_y).rev() {
                let relative_y = (f64::from(block_y) - 0.5 - y) / vertical_radius;
                if should_skip(relative_x, relative_y, relative_z, block_y)
                    || mask.get(local_x as usize, block_y, local_z as usize)
                {
                    continue;
                }
                mask.set(local_x as usize, block_y, local_z as usize);
                carve_block(
                    context,
                    lava_level,
                    chunk,
                    aquifer,
                    block_x,
                    block_y,
                    block_z,
                    &mut reached_surface,
                );
            }
        }
    }
}

/// Java: `WorldCarver.carveBlock`.
#[allow(clippy::too_many_arguments)]
fn carve_block(
    context: &CarverContext<'_>,
    lava_level: i32,
    chunk: &mut Chunk,
    aquifer: &mut Aquifer,
    block_x: i32,
    block_y: i32,
    block_z: i32,
    reached_surface: &mut bool,
) -> bool {
    let local_x = block_x.rem_euclid(16) as usize;
    let local_z = block_z.rem_euclid(16) as usize;
    let Some(block_state) = chunk.block_state(local_x, block_y, local_z) else {
        return false;
    };
    if matches!(
        block_state.name.as_str(),
        "minecraft:grass_block" | "minecraft:mycelium"
    ) {
        *reached_surface = true;
    }
    if !can_replace(&block_state.name) {
        return false;
    }

    let carve_state = if block_y <= lava_level {
        lava()
    } else {
        match aquifer.compute_substance(block_x, block_y, block_z, 0.0) {
            AquiferSubstance::Solid => return false,
            AquiferSubstance::Air => air(),
            AquiferSubstance::Fluid(kind) => kind.block_state(),
        }
    };
    let fluid = is_fluid(&carve_state);
    if !chunk.set_block_state(local_x, block_y, local_z, carve_state) {
        return false;
    }

    if *reached_surface {
        let check_y = block_y - 1;
        let is_dirt = chunk
            .block_state(local_x, check_y, local_z)
            .is_some_and(|state| state.name == "minecraft:dirt");
        if is_dirt
            && let Some(state) = context.top_material(chunk, block_x, check_y, block_z, fluid)
        {
            chunk.set_block_state(local_x, check_y, local_z, state);
        }
    }
    true
}

enum Carver {
    Cave(CaveCarver),
    Canyon(CanyonCarver),
}

impl Carver {
    fn probability(&self) -> f32 {
        match self {
            Self::Cave(carver) => carver.probability,
            Self::Canyon(carver) => carver.probability,
        }
    }

    fn carve(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        random: &mut impl RandomSource,
        origin: ChunkOrigin,
        mask: &mut CarvingMask,
    ) {
        match self {
            Self::Cave(carver) => carver.carve(context, chunk, aquifer, random, origin, mask),
            Self::Canyon(carver) => carver.carve(context, chunk, aquifer, random, origin, mask),
        }
    }
}

/// The overworld's carver list: every overworld biome uses the same
/// `[cave, cave_extra_underground, canyon]` order, and the index is part of the
/// large-feature seed.
pub struct CarverSet {
    carvers: [Carver; 3],
}

impl CarverSet {
    pub fn overworld(min_y: i32) -> Self {
        let cave = CaveCarver {
            probability: 0.15,
            y: UniformHeight {
                min: min_y + 8,
                max: 180,
            },
            y_scale: FloatProvider::Uniform { min: 0.1, max: 0.9 },
            lava_level: min_y + 8,
            horizontal_radius_multiplier: FloatProvider::Uniform { min: 0.7, max: 1.4 },
            vertical_radius_multiplier: FloatProvider::Uniform { min: 0.8, max: 1.3 },
            floor_level: FloatProvider::Uniform {
                min: -1.0,
                max: -0.4,
            },
        };
        let cave_extra_underground = CaveCarver {
            probability: 0.07,
            y: UniformHeight {
                min: min_y + 8,
                max: 47,
            },
            ..cave
        };
        let canyon = CanyonCarver {
            probability: 0.01,
            y: UniformHeight { min: 10, max: 67 },
            y_scale: FloatProvider::Constant(3.0),
            lava_level: min_y + 8,
            vertical_rotation: FloatProvider::Uniform {
                min: -0.125,
                max: 0.125,
            },
            shape: CanyonShape {
                distance_factor: FloatProvider::Uniform {
                    min: 0.75,
                    max: 1.0,
                },
                thickness: FloatProvider::Trapezoid {
                    min: 0.0,
                    max: 6.0,
                    plateau: 2.0,
                },
                width_smoothness: 3,
                horizontal_radius_factor: FloatProvider::Uniform {
                    min: 0.75,
                    max: 1.0,
                },
                vertical_radius_default_factor: 1.0,
                vertical_radius_center_factor: 0.0,
            },
        };
        Self {
            carvers: [
                Carver::Cave(cave),
                Carver::Cave(cave_extra_underground),
                Carver::Canyon(canyon),
            ],
        }
    }

    /// Java: `NoiseBasedChunkGenerator.applyCarvers`.
    pub fn apply(
        &self,
        context: &CarverContext<'_>,
        chunk: &mut Chunk,
        aquifer: &mut Aquifer,
        seed: i64,
    ) {
        let mut mask = CarvingMask::new(context.min_y, context.height);
        let target_x = chunk.x;
        let target_z = chunk.z;
        for offset_x in -8..=8 {
            for offset_z in -8..=8 {
                let chunk_x = target_x + offset_x;
                let chunk_z = target_z + offset_z;
                let biome = context.biomes.sample_quart(chunk_x * 4, 0, chunk_z * 4);
                if !biome_has_carvers(biome) {
                    continue;
                }
                for (index, carver) in self.carvers.iter().enumerate() {
                    let mut random = LegacyRandomSource::new(0);
                    set_large_feature_seed(
                        &mut random,
                        seed.wrapping_add(index as i64),
                        chunk_x,
                        chunk_z,
                    );
                    if random.next_float() <= carver.probability() {
                        carver.carve(
                            context,
                            chunk,
                            aquifer,
                            &mut random,
                            ChunkOrigin {
                                x: chunk_x,
                                z: chunk_z,
                            },
                            &mut mask,
                        );
                    }
                }
            }
        }
    }
}

/// The End and void biomes carry no configured carvers; every overworld biome
/// carries the three overworld carvers.
fn biome_has_carvers(biome: &str) -> bool {
    !matches!(
        biome,
        "minecraft:the_end"
            | "minecraft:end_highlands"
            | "minecraft:end_midlands"
            | "minecraft:end_barrens"
            | "minecraft:small_end_islands"
            | "minecraft:the_void"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_table_is_available() {
        assert_eq!(mth_sin(0.0), 0.0);
        assert!((mth_sin(f64::from(HALF_PI_F32)) - 1.0).abs() < 1.0e-6);
        assert!((mth_cos(0.0) - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn large_feature_seed_is_deterministic() {
        let mut first = LegacyRandomSource::new(0);
        let mut second = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut first, 12345, 0, -1);
        set_large_feature_seed(&mut second, 12345, 0, -1);
        for _ in 0..8 {
            assert_eq!(first.next_long(), second.next_long());
        }
    }

    #[test]
    fn carving_mask_round_trips() {
        let mut mask = CarvingMask::new(-64, 384);
        assert!(!mask.get(3, -40, 7));
        mask.set(3, -40, 7);
        assert!(mask.get(3, -40, 7));
        assert!(!mask.get(3, -40, 8));
    }
}
