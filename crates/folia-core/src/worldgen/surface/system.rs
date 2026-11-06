//! Port of `net.minecraft.world.level.levelgen.SurfaceSystem`.
//!
//! Holds the noise samplers and per-column helpers that the surface-rule
//! engine evaluates. The badlands and frozen-ocean extensions mutate the chunk
//! directly after the density pass, exactly like vanilla.

use crate::chunk::{BlockState, Chunk};
use crate::worldgen::density::WorldgenData;
use crate::worldgen::noise::NormalNoise;
use folia_random::{RandomSource, XoroshiroPositionalRandomFactory};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

pub struct SurfaceSystem {
    default_block: BlockState,
    sea_level: i32,
    min_y: i32,
    height: i32,
    clay_bands: Vec<BlockState>,
    clay_bands_offset_noise: Arc<NormalNoise>,
    badlands_pillar_noise: Arc<NormalNoise>,
    badlands_pillar_roof_noise: Arc<NormalNoise>,
    badlands_surface_noise: Arc<NormalNoise>,
    iceberg_pillar_noise: Arc<NormalNoise>,
    iceberg_pillar_roof_noise: Arc<NormalNoise>,
    iceberg_surface_noise: Arc<NormalNoise>,
    surface_noise: Arc<NormalNoise>,
    surface_secondary_noise: Arc<NormalNoise>,
    noise_random: XoroshiroPositionalRandomFactory,
    noises: HashMap<String, Arc<NormalNoise>>,
}

impl SurfaceSystem {
    pub fn new(
        data: &WorldgenData,
        min_y: i32,
        height: i32,
        sea_level: i32,
        default_block: BlockState,
    ) -> io::Result<Self> {
        let noises: HashMap<String, Arc<NormalNoise>> = data
            .noises()
            .iter()
            .map(|(name, noise)| (name.clone(), Arc::clone(noise)))
            .collect();

        // `RandomState` passes its top-level positional factory straight to
        // `new SurfaceSystem`; it is already the result of `forkPositional()`.
        let noise_random = *data.seed();
        let mut clay_random = noise_random.from_hash_of("minecraft:clay_bands");
        let clay_bands = generate_bands(&mut clay_random);

        Ok(Self {
            default_block,
            sea_level,
            min_y,
            height,
            clay_bands,
            clay_bands_offset_noise: require_noise(&noises, "minecraft:clay_bands_offset")?,
            badlands_pillar_noise: require_noise(&noises, "minecraft:badlands_pillar")?,
            badlands_pillar_roof_noise: require_noise(&noises, "minecraft:badlands_pillar_roof")?,
            badlands_surface_noise: require_noise(&noises, "minecraft:badlands_surface")?,
            iceberg_pillar_noise: require_noise(&noises, "minecraft:iceberg_pillar")?,
            iceberg_pillar_roof_noise: require_noise(&noises, "minecraft:iceberg_pillar_roof")?,
            iceberg_surface_noise: require_noise(&noises, "minecraft:iceberg_surface")?,
            surface_noise: require_noise(&noises, "minecraft:surface")?,
            surface_secondary_noise: require_noise(&noises, "minecraft:surface_secondary")?,
            noise_random,
            noises,
        })
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn noise(&self, name: &str) -> Option<&Arc<NormalNoise>> {
        self.noises.get(name)
    }

    pub fn default_block(&self) -> &BlockState {
        &self.default_block
    }

    /// Java: `SurfaceSystem.getSurfaceDepth`.
    pub fn get_surface_depth(&self, x: i32, z: i32) -> i32 {
        let value = self
            .surface_noise
            .get_value(f64::from(x), 0.0, f64::from(z));
        (value * 2.75 + 3.0 + self.noise_random.at(x, 0, z).next_double() * 0.25) as i32
    }

    /// Java: `SurfaceSystem.getSurfaceSecondary`.
    pub fn get_surface_secondary(&self, x: i32, z: i32) -> f64 {
        self.surface_secondary_noise
            .get_value(f64::from(x), 0.0, f64::from(z))
    }

    /// Java: `SurfaceSystem.getBand`.
    pub fn get_band(&self, x: i32, y: i32, z: i32) -> BlockState {
        let offset = (self
            .clay_bands_offset_noise
            .get_value(f64::from(x), 0.0, f64::from(z))
            * 4.0
            + 0.5)
            .floor() as i32;
        let length = self.clay_bands.len() as i32;
        let index = (y + offset + length).rem_euclid(length) as usize;
        self.clay_bands[index].clone()
    }

    /// `SurfaceSystem.erodedBadlandsExtension`: raises the badlands surface to
    /// the pillar height before the normal surface pass runs.
    pub fn eroded_badlands_extension(&self, chunk: &mut Chunk, x: i32, z: i32, height: i32) {
        let min = f64::min(
            (self
                .badlands_surface_noise
                .get_value(f64::from(x), 0.0, f64::from(z))
                * 8.25)
                .abs(),
            self.badlands_pillar_noise
                .get_value(f64::from(x) * 0.2, 0.0, f64::from(z) * 0.2)
                * 15.0,
        );
        if min <= 0.0 {
            return;
        }

        let abs = (self.badlands_pillar_roof_noise.get_value(
            f64::from(x) * 0.75,
            0.0,
            f64::from(z) * 0.75,
        ) * 1.5)
            .abs();
        let offset = 64.0 + f64::min(min * min * 2.5, (abs * 50.0).ceil() + 24.0);
        let floor = offset.floor() as i32;
        if height > floor {
            return;
        }

        let local_x = x.rem_euclid(16) as usize;
        let local_z = z.rem_euclid(16) as usize;

        for y in (self.min_y..=floor).rev() {
            match chunk.block_state(local_x, y, local_z) {
                Some(block) if block.name == self.default_block.name => break,
                Some(block) if block.name == "minecraft:water" => return,
                _ => {}
            }
        }

        for y in (self.min_y..=floor).rev() {
            let air = chunk.block_state(local_x, y, local_z).is_none_or(is_air);
            if !air {
                break;
            }
            let _ = chunk.set_block_state(local_x, y, local_z, self.default_block.clone());
        }
    }

    /// `SurfaceSystem.frozenOceanExtension`: builds icebergs by replacing air
    /// and water above the preliminary surface.
    #[allow(clippy::too_many_arguments)]
    pub fn frozen_ocean_extension(
        &self,
        chunk: &mut Chunk,
        min_surface_level: i32,
        melts_slightly: bool,
        x: i32,
        z: i32,
        height: i32,
    ) {
        let min = f64::min(
            (self
                .iceberg_surface_noise
                .get_value(f64::from(x), 0.0, f64::from(z))
                * 8.25)
                .abs(),
            self.iceberg_pillar_noise
                .get_value(f64::from(x) * 1.28, 0.0, f64::from(z) * 1.28)
                * 15.0,
        );
        if min <= 1.8 {
            return;
        }

        let abs = (self.iceberg_pillar_roof_noise.get_value(
            f64::from(x) * 1.17,
            0.0,
            f64::from(z) * 1.17,
        ) * 1.5)
            .abs();
        let mut iceberg = f64::min(min * min * 1.2, (abs * 40.0).ceil() + 14.0);
        if melts_slightly {
            iceberg -= 2.0;
        }

        let (bottom, top);
        if iceberg > 2.0 {
            bottom = f64::from(self.sea_level) - iceberg - 7.0;
            iceberg += f64::from(self.sea_level);
            top = iceberg;
        } else {
            bottom = 0.0;
            top = 0.0;
        }

        let mut random = self.noise_random.at(x, 0, z);
        let snow_limit = 2 + random.next_int_bound(4);
        let snow_start = self.sea_level + 18 + random.next_int_bound(10);
        let mut snow_placed = 0;

        let local_x = x.rem_euclid(16) as usize;
        let local_z = z.rem_euclid(16) as usize;
        let start = height.max(top as i32 + 1);
        let mut y = start;
        while y >= min_surface_level {
            let (is_air, is_water) = match chunk.block_state(local_x, y, local_z) {
                Some(block) => (is_air(block), block.name == "minecraft:water"),
                None => (true, false),
            };

            // Reproduce Java's short-circuit draws: the first threshold is only
            // rolled when the block is air below the iceberg top, and the
            // second only when the first clause did not already apply.
            // Java compares the integer block Y against `(int) d4` and
            // `(int) d3`; the casts truncate toward zero, so do not compare
            // against the raw doubles.
            let mut apply = false;
            if is_air && y < top as i32 {
                apply = random.next_double() > 0.01;
            }
            if !apply && is_water && y > bottom as i32 && y < self.sea_level && bottom != 0.0 {
                apply = random.next_double() > 0.15;
            }

            if apply {
                if snow_placed <= snow_limit && y > snow_start {
                    let _ =
                        chunk.set_block_state(local_x, y, local_z, block("minecraft:snow_block"));
                    snow_placed += 1;
                } else {
                    let _ =
                        chunk.set_block_state(local_x, y, local_z, block("minecraft:packed_ice"));
                }
            }

            y -= 1;
        }
    }
}

fn require_noise(
    noises: &HashMap<String, Arc<NormalNoise>>,
    name: &str,
) -> io::Result<Arc<NormalNoise>> {
    noises
        .get(name)
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("unknown noise {name}")))
}

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn is_air(block: &BlockState) -> bool {
    matches!(
        block.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `SurfaceSystem.generateBands`.
fn generate_bands(random: &mut impl RandomSource) -> Vec<BlockState> {
    const LENGTH: usize = 192;
    let mut bands = vec![block("minecraft:terracotta"); LENGTH];

    let mut index = 0i32;
    while index < LENGTH as i32 {
        index += random.next_int_bound(5) + 1;
        if index < LENGTH as i32 {
            bands[index as usize] = block("minecraft:orange_terracotta");
        }
        index += 1;
    }

    make_bands(random, &mut bands, 1, "minecraft:yellow_terracotta");
    make_bands(random, &mut bands, 2, "minecraft:brown_terracotta");
    make_bands(random, &mut bands, 1, "minecraft:red_terracotta");

    let count = random.next_int_between_inclusive(9, 15);
    let mut placed = 0;
    let mut position = 0i32;
    while placed < count && position < LENGTH as i32 {
        bands[position as usize] = block("minecraft:white_terracotta");
        // Java checks `position - 1 > 0`, so index 0 and 1 never receive the
        // left neighbour.
        if position > 1 && random.next_boolean() {
            bands[(position - 1) as usize] = block("minecraft:light_gray_terracotta");
        }
        if position + 1 < LENGTH as i32 && random.next_boolean() {
            bands[(position + 1) as usize] = block("minecraft:light_gray_terracotta");
        }
        placed += 1;
        position += random.next_int_bound(16) + 4;
    }

    bands
}

/// `SurfaceSystem.makeBands`.
fn make_bands(random: &mut impl RandomSource, bands: &mut [BlockState], min_size: i32, name: &str) {
    let count = random.next_int_between_inclusive(6, 15);
    for _ in 0..count {
        let size = min_size + random.next_int_bound(3);
        let start = random.next_int_bound(bands.len() as i32);
        let mut offset = 0;
        while start + offset < bands.len() as i32 && offset < size {
            bands[(start + offset) as usize] = block(name);
            offset += 1;
        }
    }
}
