//! Nether dimension generation.
//!
//! The router and surface values come from the shipped
//! `worldgen/noise_settings/nether.json` and `dimension_type/the_nether.json`
//! documents. They are encoded here instead of `assets/worldgen.json` because
//! that asset only carries the Overworld settings; the dimension entry point
//! keeps this module out of the Overworld path.
//!
//! Parity notes:
//! - The Nether uses `legacy_random_source: true` in 1.21.11. Exact parity
//!   would build `BlendedNoise` and the temperature/vegetation `NormalNoise`
//!   from a sequential `LegacyRandomSource`; the public noise API only exposes
//!   the positional path, so this module seeds both from the world seed with
//!   `BlendedNoise::create` / `NormalNoise::create_legacy_seed`. Terrain is
//!   deterministic and structurally correct, but not block-identical to Java.
//! - Only the surface rules needed by the shipped Nether biomes are executed.
//!   Structures, nether fortresses, bastions, and portal logic are out of
//!   scope for this slice.

use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use crate::worldgen::density::{DensityFunction, WorldgenData};
use crate::worldgen::feature::{
    ConfiguredFeature, DECORATION_STEPS, Decoration, FeatureData, Heightmap, PlacedFeature,
    PlacementWorld, WorldGenRegion,
};
use crate::worldgen::generator::sample_grid;
use crate::worldgen::noise::{BlendedNoise, NormalNoise};
use folia_math::BlockPos;
use folia_random::{
    RandomSource, WorldgenRandom, XoroshiroPositionalRandomFactory, XoroshiroRandomSource,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io;
use std::sync::Arc;

pub const MIN_Y: i32 = 0;
pub const HEIGHT: i32 = 128;
/// The Nether dimension type is 256 blocks tall; terrain and the lava sea
/// occupy the lower 128 blocks and the remaining sections are empty.
pub const DIMENSION_HEIGHT: i32 = 256;
pub const SEA_LEVEL: i32 = 32;

const CELL_WIDTH: i32 = 4;
const CELL_HEIGHT: i32 = 8;

const NETHERRACK: &str = "minecraft:netherrack";
const BEDROCK: &str = "minecraft:bedrock";
const LAVA: &str = "minecraft:lava";

/// Nether `MultiNoiseBiomeSource` parameter list
/// (`MultiNoiseBiomeSourceParameterList.Preset.NETHER`).
const NETHER_BIOMES: [NetherBiomeEntry; 5] = [
    NetherBiomeEntry::new("minecraft:nether_wastes", 0, 0, 0),
    NetherBiomeEntry::new("minecraft:soul_sand_valley", 0, -5000, 0),
    NetherBiomeEntry::new("minecraft:crimson_forest", 4000, 0, 0),
    NetherBiomeEntry::new("minecraft:warped_forest", 0, 5000, 3750),
    NetherBiomeEntry::new("minecraft:basalt_deltas", -5000, 0, 1750),
];

#[derive(Debug, Clone, Copy)]
struct NetherBiomeEntry {
    name: &'static str,
    temperature: i64,
    humidity: i64,
    offset: i64,
}

impl NetherBiomeEntry {
    const fn new(name: &'static str, temperature: i64, humidity: i64, offset: i64) -> Self {
        Self {
            name,
            temperature,
            humidity,
            offset,
        }
    }

    fn centre_abs(self) -> i64 {
        self.temperature.abs() + self.humidity.abs() + self.offset.abs()
    }

    fn fitness(self, temperature: i64, humidity: i64) -> i64 {
        square(distance(self.temperature, temperature))
            + square(distance(self.humidity, humidity))
            + square(self.offset)
    }
}

pub struct NetherGenerator {
    seed: i64,
    final_density: Arc<DensityFunction>,
    temperature: Arc<DensityFunction>,
    humidity: Arc<DensityFunction>,
    biomes: [NetherBiomeEntry; 5],
    biome_order: [usize; 5],
    surface_noise: Arc<NormalNoise>,
    patch_noise: Arc<NormalNoise>,
    netherrack_noise: Arc<NormalNoise>,
    nether_wart_noise: Arc<NormalNoise>,
    soul_sand_layer_noise: Arc<NormalNoise>,
    gravel_layer_noise: Arc<NormalNoise>,
    state_selector_noise: Arc<NormalNoise>,
    seed_factory: XoroshiroPositionalRandomFactory,
    bedrock_floor: XoroshiroPositionalRandomFactory,
    bedrock_roof: XoroshiroPositionalRandomFactory,
    feature_data: &'static FeatureData,
    step_order: Vec<Vec<String>>,
}

impl NetherGenerator {
    pub fn new(seed: i64) -> io::Result<Self> {
        let data = WorldgenData::overworld(seed)?;
        let factory = *data.seed();

        // `NoiseRouterData.nether` = noNewCaves(slideNetherLike(base noise)).
        // The shipped JSON expands the slide into this y-clamped expression.
        let mut noise_random = XoroshiroRandomSource::new(seed);
        let base = Arc::new(DensityFunction::OldBlendedNoise(Arc::new(
            BlendedNoise::create(&mut noise_random, 0.25, 0.375, 80.0, 60.0, 8.0),
        )));
        let base = add(base, constant(-0.9375));
        let top_gradient = Arc::new(DensityFunction::YClampedGradient {
            from_y: 104,
            to_y: 128,
            from_value: 1.0,
            to_value: 0.0,
        });
        let top = add(constant(0.9375), mul(top_gradient, base));
        let middle = add(constant(-2.5), top);
        let bottom_gradient = Arc::new(DensityFunction::YClampedGradient {
            from_y: -8,
            to_y: 24,
            from_value: 0.0,
            to_value: 1.0,
        });
        let outer = add(constant(2.5), mul(bottom_gradient, middle));
        let final_density = Arc::new(DensityFunction::Squeeze(mul(constant(0.64), outer)));

        // `RandomState.NoiseWiringHelper` replaces the biome noises with
        // `NormalNoise.createLegacyNetherBiome` instances for legacy settings.
        let temperature_noise = NormalNoise::create_legacy_seed(seed, -7, &[1.0, 1.0]);
        let humidity_noise = NormalNoise::create_legacy_seed(seed.wrapping_add(1), -7, &[1.0, 1.0]);
        let temperature = Arc::new(DensityFunction::Noise {
            noise: Arc::new(temperature_noise),
            xz_scale: 0.25,
            y_scale: 0.0,
        });
        let humidity = Arc::new(DensityFunction::Noise {
            noise: Arc::new(humidity_noise),
            xz_scale: 0.25,
            y_scale: 0.0,
        });

        let mut biome_order = [0usize, 1, 2, 3, 4];
        biome_order.sort_by_key(|index| NETHER_BIOMES[*index].centre_abs());
        let feature_data = FeatureData::vanilla();
        let step_order =
            nether_step_feature_order(feature_data, &NETHER_BIOMES.map(|entry| entry.name));

        Ok(Self {
            seed,
            final_density,
            temperature,
            humidity,
            biomes: NETHER_BIOMES,
            biome_order,
            surface_noise: require_noise(&data, "minecraft:surface")?,
            patch_noise: require_noise(&data, "minecraft:patch")?,
            netherrack_noise: require_noise(&data, "minecraft:netherrack")?,
            nether_wart_noise: require_noise(&data, "minecraft:nether_wart")?,
            soul_sand_layer_noise: require_noise(&data, "minecraft:soul_sand_layer")?,
            gravel_layer_noise: require_noise(&data, "minecraft:gravel_layer")?,
            state_selector_noise: require_noise(&data, "minecraft:nether_state_selector")?,
            seed_factory: factory,
            bedrock_floor: random_factory(&factory, "minecraft:bedrock_floor"),
            bedrock_roof: random_factory(&factory, "minecraft:bedrock_roof"),
            feature_data,
            step_order,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn min_y(&self) -> i32 {
        MIN_Y
    }

    pub fn height(&self) -> i32 {
        HEIGHT
    }

    pub fn sea_level(&self) -> i32 {
        SEA_LEVEL
    }

    /// Final density at one block position, before the 4x8x4 interpolation
    /// grid used by chunk generation.
    pub fn density(&self, x: i32, y: i32, z: i32) -> f64 {
        self.final_density.compute(x, y, z)
    }

    /// Nether biome for a quart position.
    pub fn biome_at_quart(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> &'static str {
        let x = quart_x << 2;
        let y = quart_y << 2;
        let z = quart_z << 2;
        self.biome_at_block(x, y, z)
    }

    /// Nether biome at a block position. The three climate dimensions that are
    /// non-zero in the Nether router are temperature and humidity; the rest
    /// are constant zero, matching `NoiseRouterData.nether`.
    pub fn biome_at_block(&self, x: i32, y: i32, z: i32) -> &'static str {
        let temperature = quantize(self.temperature.compute(x, y, z) as f32);
        let humidity = quantize(self.humidity.compute(x, y, z) as f32);
        let mut best = self.biome_order[0];
        let mut best_fitness = self.biomes[best].fitness(temperature, humidity);
        for index in self.biome_order.iter().skip(1) {
            let fitness = self.biomes[*index].fitness(temperature, humidity);
            if fitness < best_fitness {
                best = *index;
                best_fitness = fitness;
            }
        }
        self.biomes[best].name
    }

    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32) -> io::Result<Chunk> {
        let cells_x = (16 / CELL_WIDTH + 1) as usize;
        let cells_z = cells_x;
        let cells_y = (HEIGHT / CELL_HEIGHT + 1) as usize;
        let mut grid = vec![0.0f64; cells_x * cells_z * cells_y];
        for xi in 0..cells_x {
            for zi in 0..cells_z {
                for yi in 0..cells_y {
                    let x = chunk_x * 16 + xi as i32 * CELL_WIDTH;
                    let y = MIN_Y + yi as i32 * CELL_HEIGHT;
                    let z = chunk_z * 16 + zi as i32 * CELL_WIDTH;
                    grid[(xi * cells_z + zi) * cells_y + yi] = self.final_density.compute(x, y, z);
                }
            }
        }

        let section_count = DIMENSION_HEIGHT / 16;
        let mut sections = Vec::with_capacity(section_count as usize);
        for index in 0..section_count {
            sections.push(ChunkSection {
                y: (MIN_Y.div_euclid(16) + index) as i8,
                block_states: PalettedContainer {
                    palette: vec![air()],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:nether_wastes".into()],
                    indices: vec![0; 64],
                }),
                light: crate::light::SectionLight::default(),
            });
        }
        let mut chunk = Chunk {
            x: chunk_x,
            z: chunk_z,
            sections,
        };

        let netherrack = BlockState {
            name: NETHERRACK.into(),
            properties: vec![],
        };
        let lava = BlockState {
            name: LAVA.into(),
            properties: vec![("level".into(), "0".into())],
        };

        let mut solid_columns = vec![false; HEIGHT as usize];
        for local_x in 0..16 {
            for local_z in 0..16 {
                let world_x = chunk_x * 16 + local_x;
                let world_z = chunk_z * 16 + local_z;
                solid_columns.fill(false);
                for (index, solid) in solid_columns.iter_mut().enumerate() {
                    let y = MIN_Y + index as i32;
                    let value = sample_grid(
                        &grid,
                        cells_x,
                        cells_z,
                        cells_y,
                        CELL_WIDTH,
                        CELL_HEIGHT,
                        local_x,
                        local_z,
                        y - MIN_Y,
                    );
                    if value > 0.0 {
                        *solid = true;
                        chunk.set_block_state_ref(
                            local_x as usize,
                            y,
                            local_z as usize,
                            &netherrack,
                        );
                    } else if y < SEA_LEVEL {
                        chunk.set_block_state_ref(local_x as usize, y, local_z as usize, &lava);
                    }
                }
                self.apply_surface(
                    &mut chunk,
                    local_x as usize,
                    local_z as usize,
                    world_x,
                    world_z,
                    &solid_columns,
                );
            }
        }

        let biome_indices = self.fill_biomes(&mut chunk, chunk_x, chunk_z);
        self.apply_features(&mut chunk, &biome_indices)?;
        Ok(chunk)
    }

    fn fill_biomes(&self, chunk: &mut Chunk, chunk_x: i32, chunk_z: i32) -> Vec<usize> {
        let quart_layers = (DIMENSION_HEIGHT >> 2) as usize;
        let mut indices = vec![0usize; quart_layers * 16];
        for qz in 0..4usize {
            for qx in 0..4usize {
                let quart_x = chunk_x * 4 + qx as i32;
                let quart_z = chunk_z * 4 + qz as i32;
                let biome = self.biome_at_quart(quart_x, 64 >> 2, quart_z);
                let index = self
                    .biomes
                    .iter()
                    .position(|entry| entry.name == biome)
                    .unwrap_or(0);
                for layer in 0..quart_layers {
                    indices[(layer * 4 + qz) * 4 + qx] = index;
                }
            }
        }

        let mut cursor = 0usize;
        for section in &mut chunk.sections {
            let mut palette: Vec<String> = Vec::new();
            let mut section_indices = Vec::with_capacity(64);
            for _ in 0..64 {
                let entry = indices[cursor];
                cursor += 1;
                let name = self.biomes[entry].name.to_owned();
                let index = match palette.iter().position(|value| value == &name) {
                    Some(index) => index,
                    None => {
                        palette.push(name);
                        palette.len() - 1
                    }
                };
                section_indices.push(index as u16);
            }
            section.biomes = Some(PalettedContainer {
                palette,
                indices: section_indices,
            });
        }
        indices
    }

    /// Applies the shipped Nether surface sequence to one column.
    ///
    /// This is a focused port of `worldgen/noise_settings/nether.json`, using
    /// the same `surface` noise and the same `stoneDepthAbove`/`hole`
    /// definitions as `SurfaceSystem.buildSurface`.
    #[allow(clippy::too_many_arguments)]
    fn apply_surface(
        &self,
        chunk: &mut Chunk,
        local_x: usize,
        local_z: usize,
        world_x: i32,
        world_z: i32,
        solid: &[bool],
    ) {
        // `stoneDepthAbove` counts down from the top of each contiguous stone
        // stack; `stoneDepthBelow` counts up from its bottom, matching the
        // counters `SurfaceSystem.buildSurface` maintains.
        let mut depth_above = vec![0i32; HEIGHT as usize];
        let mut depth_below = vec![0i32; HEIGHT as usize];
        let mut above = 0;
        for y in (0..HEIGHT as usize).rev() {
            if solid[y] {
                above += 1;
                depth_above[y] = above;
            } else {
                above = 0;
            }
        }
        let mut run_bottom = 0i32;
        for y in 0..HEIGHT as usize {
            if solid[y] {
                if y == 0 || !solid[y - 1] {
                    run_bottom = y as i32;
                }
                depth_below[y] = y as i32 - run_bottom + 1;
            }
        }

        let surface_depth = self.surface_depth(world_x, world_z);
        let biome = self.biome_at_block(world_x, 64, world_z);
        let patch = self
            .patch_noise
            .get_value(f64::from(world_x), 0.0, f64::from(world_z));
        let netherrack =
            self.netherrack_noise
                .get_value(f64::from(world_x), 0.0, f64::from(world_z));
        let wart = self
            .nether_wart_noise
            .get_value(f64::from(world_x), 0.0, f64::from(world_z));
        let soul_sand =
            self.soul_sand_layer_noise
                .get_value(f64::from(world_x), 0.0, f64::from(world_z));
        let gravel = self
            .gravel_layer_noise
            .get_value(f64::from(world_x), 0.0, f64::from(world_z));
        let selector =
            self.state_selector_noise
                .get_value(f64::from(world_x), 0.0, f64::from(world_z));

        for y in 0..HEIGHT {
            if !solid[y as usize] {
                continue;
            }
            if let Some(state) = self.surface_block(
                y,
                biome,
                surface_depth,
                depth_above[y as usize],
                depth_below[y as usize],
                patch,
                netherrack,
                wart,
                soul_sand,
                gravel,
                selector,
                world_x,
                world_z,
            ) {
                chunk.set_block_state_ref(local_x, y, local_z, &state);
            }
        }
    }

    /// The first matching rule of the Nether surface sequence, or `None` when
    /// the block stays netherrack.
    #[allow(clippy::too_many_arguments)]
    fn surface_block(
        &self,
        y: i32,
        biome: &str,
        surface_depth: i32,
        depth_above: i32,
        depth_below: i32,
        patch: f64,
        netherrack: f64,
        wart: f64,
        soul_sand: f64,
        gravel: f64,
        selector: f64,
        world_x: i32,
        world_z: i32,
    ) -> Option<BlockState> {
        let hole = surface_depth <= 0;

        // Floor bedrock: always at the bottom, random through the next four.
        if y == 0 {
            return Some(state(BEDROCK));
        }
        if y < 5 {
            let probability = (5 - y) as f32 / 5.0;
            let mut random = self.bedrock_floor.at(world_x, y, world_z);
            if random.next_float() < probability {
                return Some(state(BEDROCK));
            }
        }
        // Roof bedrock: always at the top, random through the five below it.
        if y >= 127 {
            return Some(state(BEDROCK));
        }
        if y >= 123 {
            let probability = (y - 122) as f32 / 5.0;
            let mut random = self.bedrock_roof.at(world_x, y, world_z);
            if random.next_float() < probability {
                return Some(state(BEDROCK));
            }
        }

        let floor = depth_above <= 1 + surface_depth;
        let shallow_floor = depth_above <= 1;
        let ceiling = depth_below <= 1 + surface_depth;
        let above_31 = y >= 31;
        let above_30_with_depth = y + depth_above >= 30 && y + depth_above < 35;
        let above_35_with_depth = y + depth_above >= 35;

        match biome {
            "minecraft:basalt_deltas" => {
                if ceiling {
                    return Some(basalt());
                }
                if floor {
                    if patch >= -0.012 && above_30_with_depth {
                        return Some(state("minecraft:gravel"));
                    }
                    if selector >= 0.0 {
                        return Some(basalt());
                    }
                    return Some(state("minecraft:blackstone"));
                }
            }
            "minecraft:soul_sand_valley" => {
                if ceiling {
                    return Some(if selector >= 0.0 {
                        state("minecraft:soul_sand")
                    } else {
                        state("minecraft:soul_soil")
                    });
                }
                if floor {
                    if patch >= -0.012 && above_30_with_depth {
                        return Some(state("minecraft:gravel"));
                    }
                    return Some(if selector >= 0.0 {
                        state("minecraft:soul_sand")
                    } else {
                        state("minecraft:soul_soil")
                    });
                }
            }
            _ => {}
        }

        // Generic floor rule: lava in holes below y=32 and the two nylium
        // forests near the lava sea.
        if shallow_floor {
            if y < 32 && hole {
                return Some(state(LAVA));
            }
            if (biome == "minecraft:warped_forest" || biome == "minecraft:crimson_forest")
                && above_31
                && !above_35_with_depth
                && netherrack < 0.54
            {
                let top = match (biome, wart >= 1.17) {
                    ("minecraft:warped_forest", true) => "minecraft:warped_wart_block",
                    ("minecraft:warped_forest", false) => "minecraft:warped_nylium",
                    ("minecraft:crimson_forest", true) => "minecraft:nether_wart_block",
                    _ => "minecraft:crimson_nylium",
                };
                return Some(state(top));
            }
        }

        if biome == "minecraft:nether_wastes" {
            if floor && soul_sand >= -0.012 {
                if !hole && above_30_with_depth {
                    return Some(state("minecraft:soul_sand"));
                }
                return None;
            }
            if shallow_floor
                && above_31
                && !above_35_with_depth
                && gravel >= -0.012
                && (y >= 32 || !hole)
            {
                return Some(state("minecraft:gravel"));
            }
        }

        None
    }

    fn surface_depth(&self, x: i32, z: i32) -> i32 {
        let value = self
            .surface_noise
            .get_value(f64::from(x), 0.0, f64::from(z));
        let mut random = self.seed_factory.at(x, 0, z);
        (value * 2.75 + 3.0 + random.next_double() * 0.25) as i32
    }

    fn apply_features(&self, chunk: &mut Chunk, biome_indices: &[usize]) -> io::Result<()> {
        let mut chunk_biomes = HashSet::new();
        for entry in biome_indices {
            chunk_biomes.insert(self.biomes[*entry].name);
        }
        let mut random = WorldgenRandom::new(0);
        let decoration_seed = random.set_decoration_seed(
            self.seed,
            chunk.x.wrapping_mul(16),
            chunk.z.wrapping_mul(16),
        );
        let origin = BlockPos::new(chunk.x.wrapping_mul(16), MIN_Y, chunk.z.wrapping_mul(16));
        for step in [
            Decoration::UndergroundOres,
            Decoration::UndergroundDecoration,
            Decoration::VegetalDecoration,
            Decoration::TopLayerModification,
        ] {
            let mut active = HashSet::new();
            for biome in &chunk_biomes {
                let Some(steps) = self.feature_data.biome_features(biome) else {
                    continue;
                };
                for feature in &steps[step.index()] {
                    active.insert(feature.as_str());
                }
            }
            if active.is_empty() {
                continue;
            }
            for (index, id) in self.step_order[step.index()].iter().enumerate() {
                if !active.contains(id.as_str()) {
                    continue;
                }
                let Some(placed) = self.feature_data.placed(id) else {
                    continue;
                };
                if !placed.placement_is_supported() {
                    continue;
                }
                let Some(configured) = self.feature_data.configured(placed.feature()) else {
                    continue;
                };
                if !configured.is_supported() {
                    continue;
                }
                random.set_feature_seed(decoration_seed, index as i32, step.index() as i32);
                self.place_placed_feature(placed, configured, chunk, &mut random, origin)?;
            }
        }
        Ok(())
    }

    fn place_placed_feature(
        &self,
        placed: &PlacedFeature,
        configured: &ConfiguredFeature,
        chunk: &mut Chunk,
        random: &mut WorldgenRandom,
        origin: BlockPos,
    ) -> io::Result<()> {
        self.place_modifier_step(placed, configured, chunk, random, origin, 0)
    }

    fn place_modifier_step(
        &self,
        placed: &PlacedFeature,
        configured: &ConfiguredFeature,
        chunk: &mut Chunk,
        random: &mut WorldgenRandom,
        pos: BlockPos,
        index: usize,
    ) -> io::Result<()> {
        let Some(modifier) = placed.placement_modifiers().get(index) else {
            let mut region = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
            return configured
                .place(&mut region, random, pos)
                .map(|_| ())
                .map_err(|feature| {
                    io::Error::new(io::ErrorKind::InvalidData, feature.to_string())
                });
        };
        let positions = {
            let world = NetherPlacementWorld {
                chunk: &*chunk,
                generator: self,
                data: self.feature_data,
            };
            modifier
                .get_positions(pos, random, &world, placed.id(), MIN_Y, HEIGHT)
                .map_err(|unsupported| {
                    io::Error::new(io::ErrorKind::InvalidData, unsupported.to_string())
                })?
        };
        for next in positions {
            self.place_modifier_step(placed, configured, chunk, random, next, index + 1)?;
        }
        Ok(())
    }
}

struct NetherPlacementWorld<'a> {
    chunk: &'a Chunk,
    generator: &'a NetherGenerator,
    data: &'a FeatureData,
}

impl NetherPlacementWorld<'_> {
    fn local_column(&self, x: i32, z: i32) -> Option<(usize, usize)> {
        let local_x = i64::from(x) - i64::from(self.chunk.x) * 16;
        let local_z = i64::from(z) - i64::from(self.chunk.z) * 16;
        if (0..16).contains(&local_x) && (0..16).contains(&local_z) {
            Some((local_x as usize, local_z as usize))
        } else {
            None
        }
    }
}

impl PlacementWorld for NetherPlacementWorld<'_> {
    fn biome_at(&self, pos: BlockPos) -> &str {
        if let Some((x, z)) = self.local_column(pos.x(), pos.z()) {
            let y = pos.y().clamp(MIN_Y, MIN_Y + HEIGHT - 1);
            if let Some(biome) = self.chunk.biome(x, y, z) {
                return biome;
            }
        }
        self.generator.biome_at_block(pos.x(), pos.y(), pos.z())
    }

    fn biome_has_placed_feature(&self, biome: &str, placed_feature: &str) -> bool {
        self.data.biome_features(biome).is_some_and(|steps| {
            steps
                .iter()
                .any(|step| step.iter().any(|feature| feature == placed_feature))
        })
    }

    fn heightmap(&self, kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        let (x, z) = self.local_column(x, z)?;
        for y in (MIN_Y..MIN_Y + HEIGHT).rev() {
            let Some(state) = self.chunk.block_state(x, y, z) else {
                continue;
            };
            let fluid = is_fluid(state);
            let blocks_motion = match kind {
                Heightmap::WorldSurface | Heightmap::WorldSurfaceWg => !is_air(state),
                Heightmap::OceanFloor | Heightmap::OceanFloorWg => !is_air(state) && !fluid,
                Heightmap::MotionBlocking => !is_air(state),
                Heightmap::MotionBlockingNoLeaves => {
                    !is_air(state) && !crate::tags::block_has("minecraft:leaves", &state.name)
                }
            };
            if blocks_motion {
                return Some(y + 1);
            }
        }
        Some(MIN_Y)
    }

    fn block_at(&self, pos: BlockPos) -> Option<&BlockState> {
        let (x, z) = self.local_column(pos.x(), pos.z())?;
        self.chunk.block_state(x, pos.y(), z)
    }
}

/// `FeatureSorter.buildFeaturesPerStep` for the Nether preset order.
///
/// `FeatureData::step_feature_order` derives its biome order from the
/// Overworld parameter list, which excludes every Nether biome. The Nether
/// must use `MultiNoiseBiomeSourceParameterList.Preset.NETHER` order instead.
fn nether_step_feature_order(data: &FeatureData, biome_order: &[&str; 5]) -> Vec<Vec<String>> {
    let mut feature_indices: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut features: Vec<String> = Vec::new();
    let mut graph: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();
    for biome in biome_order {
        let Some(steps) = data.biome_features(biome) else {
            continue;
        };
        for (step, list) in steps.iter().enumerate() {
            let mut nodes = Vec::with_capacity(list.len());
            for id in list {
                let index = match feature_indices.get(id) {
                    Some(index) => *index,
                    None => {
                        let index = features.len();
                        features.push(id.clone());
                        feature_indices.insert(id.clone(), index);
                        index
                    }
                };
                nodes.push((step, index));
            }
            for pair in nodes.windows(2) {
                graph.entry(pair[0]).or_default().insert(pair[1]);
            }
        }
    }

    let mut visited = BTreeSet::new();
    let mut in_progress = BTreeSet::new();
    let mut ordered = Vec::new();
    for root in graph.keys().copied().collect::<Vec<_>>() {
        if !visited.contains(&root) {
            feature_order_dfs(&graph, root, &mut visited, &mut in_progress, &mut ordered);
        }
    }
    ordered.reverse();

    let mut by_step = vec![Vec::new(); DECORATION_STEPS];
    for (step, index) in ordered {
        if step < DECORATION_STEPS {
            by_step[step].push(features[index].clone());
        }
    }
    by_step
}

fn feature_order_dfs(
    graph: &BTreeMap<(usize, usize), BTreeSet<(usize, usize)>>,
    node: (usize, usize),
    visited: &mut BTreeSet<(usize, usize)>,
    in_progress: &mut BTreeSet<(usize, usize)>,
    ordered: &mut Vec<(usize, usize)>,
) {
    if visited.contains(&node) {
        return;
    }
    if !in_progress.insert(node) {
        panic!("feature order cycle found");
    }
    if let Some(neighbours) = graph.get(&node) {
        for next in neighbours {
            feature_order_dfs(graph, *next, visited, in_progress, ordered);
        }
    }
    in_progress.remove(&node);
    visited.insert(node);
    ordered.push(node);
}

fn require_noise(data: &WorldgenData, name: &str) -> io::Result<Arc<NormalNoise>> {
    data.noise(name)
}

fn random_factory(
    seed: &XoroshiroPositionalRandomFactory,
    name: &str,
) -> XoroshiroPositionalRandomFactory {
    let mut random = seed.from_hash_of(name);
    random.fork_positional()
}

fn quantize(value: f32) -> i64 {
    (value * 10000.0) as i64
}

fn distance(centre: i64, point: i64) -> i64 {
    (centre - point).abs()
}

fn square(value: i64) -> i64 {
    value.saturating_mul(value)
}

fn constant(value: f64) -> Arc<DensityFunction> {
    Arc::new(DensityFunction::Constant(value))
}

fn add(first: Arc<DensityFunction>, second: Arc<DensityFunction>) -> Arc<DensityFunction> {
    Arc::new(DensityFunction::Add(first, second))
}

fn mul(first: Arc<DensityFunction>, second: Arc<DensityFunction>) -> Arc<DensityFunction> {
    Arc::new(DensityFunction::Mul(first, second))
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn basalt() -> BlockState {
    BlockState {
        name: "minecraft:basalt".into(),
        properties: vec![("axis".into(), "y".into())],
    }
}

fn air() -> BlockState {
    state("minecraft:air")
}

fn is_air(state: &BlockState) -> bool {
    crate::worldgen::feature::is_air_name(&state.name)
}

fn is_fluid(state: &BlockState) -> bool {
    matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nether_biome_order_matches_the_vanilla_rtree_order() {
        let generator = NetherGenerator::new(12345).unwrap();
        let names: Vec<&str> = generator
            .biome_order
            .iter()
            .map(|index| generator.biomes[*index].name)
            .collect();
        assert_eq!(
            names,
            vec![
                "minecraft:nether_wastes",
                "minecraft:crimson_forest",
                "minecraft:soul_sand_valley",
                "minecraft:basalt_deltas",
                "minecraft:warped_forest",
            ]
        );
    }

    #[test]
    fn nether_biomes_are_all_valid_registry_names() {
        let generator = NetherGenerator::new(12345).unwrap();
        let expected: Vec<&str> = generator.biomes.iter().map(|entry| entry.name).collect();
        for x in (-64..64).step_by(16) {
            for z in (-64..64).step_by(16) {
                let biome = generator.biome_at_block(x, 64, z);
                assert!(expected.contains(&biome), "unknown sampled biome {biome}");
            }
        }
    }

    #[test]
    fn nether_density_is_deterministic() {
        let first = NetherGenerator::new(12345).unwrap();
        let second = NetherGenerator::new(12345).unwrap();
        for (x, y, z) in [(0, 0, 0), (0, 64, 0), (0, 127, 0), (15, 32, -16)] {
            assert_eq!(first.density(x, y, z), second.density(x, y, z));
        }
    }
}
