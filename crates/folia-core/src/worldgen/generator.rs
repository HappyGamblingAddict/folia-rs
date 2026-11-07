//! Noise-based overworld chunk generation.
//!
//! This evaluates the real `minecraft:overworld` final-density router on the
//! vanilla 4x8x4 interpolation grid and fills a Rust chunk. The density pass
//! produces stone/air/water/lava with the `OreVeinifier` filler, then the full
//! vanilla surface-rule tree (bedrock floor, deepslate gradient, badlands,
//! snow/ice, swamp water, ...) replaces the near-surface blocks, the cave and
//! canyon carvers replay their starts from the 17x17 neighbourhood, and the
//! supported underground-ore, underground, vegetation, and top-layer features
//! finish the chunk. Other features and structures are not applied yet.

use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer, is_air_state};
use crate::worldgen::aquifer::{Aquifer, AquiferSubstance};
use crate::worldgen::biome::BiomeSampler;
use crate::worldgen::carver::{CarverContext, CarverSet};
use crate::worldgen::density::{NoiseRouter, WorldgenData};
use crate::worldgen::feature::{
    ConfiguredFeature, Decoration, FeatureData, FeatureLevel, Heightmap, PlacedFeature,
    PlacementWorld, WorldGenRegion,
};
use crate::worldgen::ore_vein::OreVeinifier;
use crate::worldgen::structure::StructureDimension;
use crate::worldgen::structure::set::StructureBiomeLookup;
use crate::worldgen::structure::start::StructureGenerator;
use crate::worldgen::structure::template::StructureTemplate;
use crate::worldgen::surface::{self, RuleSource, system::SurfaceSystem};
use folia_math::BlockPos;
use folia_random::{WorldgenRandom, XoroshiroPositionalRandomFactory};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, OnceLock};

const CELL_WIDTH: i32 = 4;
const CELL_HEIGHT: i32 = 8;

static GENERATOR: OnceLock<Arc<WorldGenerator>> = OnceLock::new();

/// A server dimension with the type fields needed by generation.
///
/// This mirrors the shipped `dimension_type` documents for 1.21.11. Generation
/// itself still goes through a per-dimension generator; the Overworld path is
/// the existing [`WorldGenerator`] and is unchanged by this abstraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    pub const fn min_y(self) -> i32 {
        match self {
            Dimension::Overworld => -64,
            Dimension::Nether | Dimension::End => 0,
        }
    }

    pub const fn height(self) -> i32 {
        match self {
            Dimension::Overworld => 384,
            Dimension::Nether | Dimension::End => 256,
        }
    }

    /// Height of the generated noise column, which is shorter than the
    /// dimension's build height in the Nether and the End.
    pub const fn generated_height(self) -> i32 {
        match self {
            Dimension::Overworld => 384,
            Dimension::Nether => 128,
            Dimension::End => 128,
        }
    }

    pub const fn logical_height(self) -> i32 {
        match self {
            Dimension::Overworld => 384,
            Dimension::Nether => 128,
            Dimension::End => 256,
        }
    }

    pub const fn sea_level(self) -> i32 {
        match self {
            Dimension::Overworld => 63,
            Dimension::Nether => 32,
            Dimension::End => 0,
        }
    }

    pub const fn has_skylight(self) -> bool {
        !matches!(self, Dimension::Nether)
    }

    pub const fn has_ceiling(self) -> bool {
        matches!(self, Dimension::Nether)
    }

    pub const fn coordinate_scale(self) -> f32 {
        match self {
            Dimension::Overworld => 1.0,
            Dimension::Nether => 8.0,
            Dimension::End => 1.0,
        }
    }

    pub const fn ambient_light(self) -> f32 {
        match self {
            Dimension::Overworld => 0.0,
            Dimension::Nether => 0.1,
            Dimension::End => 0.25,
        }
    }
}

/// Generates one chunk in the requested dimension.
///
/// This is an explicit entry point for tests and future world storage. It is
/// deliberately not wired into `join.rs` or `world.rs`; the Overworld branch
/// runs the exact same [`WorldGenerator`] path as before, so existing parity
/// fixtures are unaffected.
pub fn generate_dimension(
    dimension: Dimension,
    chunk_x: i32,
    chunk_z: i32,
    seed: i64,
) -> io::Result<Chunk> {
    match dimension {
        Dimension::Overworld => {
            if let Some(generator) = configured()
                && generator.seed() == seed
            {
                return generator.generate_chunk(chunk_x, chunk_z);
            }
            WorldGenerator::overworld(seed)?.generate_chunk(chunk_x, chunk_z)
        }
        Dimension::Nether => {
            crate::worldgen::nether::NetherGenerator::new(seed)?.generate_chunk(chunk_x, chunk_z)
        }
        Dimension::End => {
            crate::worldgen::end::EndGenerator::new(seed)?.generate_chunk(chunk_x, chunk_z)
        }
    }
}

/// Builds and installs the process-wide overworld generator. The server calls
/// this once at startup with the level seed.
pub fn configure(seed: i64) -> io::Result<()> {
    if GENERATOR.get().is_some() {
        return Ok(());
    }
    let generator = WorldGenerator::overworld(seed)?;
    let _ = GENERATOR.set(Arc::new(generator));
    Ok(())
}

/// Returns the configured generator, if startup configured one.
pub fn configured() -> Option<&'static Arc<WorldGenerator>> {
    GENERATOR.get()
}

pub struct WorldGenerator {
    router: NoiseRouter,
    biomes: BiomeSampler,
    surface: SurfaceSystem,
    surface_rules: RuleSource,
    carvers: CarverSet,
    aquifer_factory: XoroshiroPositionalRandomFactory,
    ore_veins: OreVeinifier,
    structures: Option<StructureGenerator>,
    seed: i64,
    min_y: i32,
    height: i32,
    sea_level: i32,
}

impl WorldGenerator {
    pub fn overworld(seed: i64) -> io::Result<Self> {
        let data = WorldgenData::overworld(seed)?;
        let min_y = data.min_y();
        let height = data.height();
        let sea_level = data.sea_level();
        let aquifer_factory = data
            .seed()
            .from_hash_of("minecraft:aquifer")
            .fork_positional();
        let biomes = BiomeSampler::from_data(&data)?;
        let surface = SurfaceSystem::new(
            &data,
            min_y,
            height,
            sea_level,
            BlockState {
                name: "minecraft:stone".into(),
                properties: vec![],
            },
        )?;
        let surface_rules =
            surface::parse_rules(&data.surface_rule()?, data.seed(), min_y, height)?;
        let router = data.router()?;
        let ore_veins = OreVeinifier::new(&router, data.seed())?;
        Ok(Self {
            router,
            biomes,
            surface,
            surface_rules,
            carvers: CarverSet::overworld(min_y),
            aquifer_factory,
            ore_veins,
            structures: None,
            seed,
            min_y,
            height,
            sea_level,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// Enables the shipped structure manager with the caller's template
    /// provider. Without this the generator is the original terrain/feature
    /// path and the parity fixtures are byte-for-byte unchanged.
    pub fn with_shipped_structures(
        mut self,
        templates: HashMap<String, StructureTemplate>,
    ) -> io::Result<Self> {
        self.structures = Some(
            StructureGenerator::shipped(templates)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?,
        );
        Ok(self)
    }

    /// Enables a prebuilt structure manager (used by tests and future
    /// datapack/runtime loaders).
    pub fn with_structure_generator(mut self, structures: StructureGenerator) -> Self {
        self.structures = Some(structures);
        self
    }

    pub fn structures(&self) -> Option<&StructureGenerator> {
        self.structures.as_ref()
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// Evaluates the final density at a block position (no interpolation).
    pub fn density(&self, x: i32, y: i32, z: i32) -> f64 {
        self.router.final_density().compute(x, y, z)
    }

    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.biomes.sample_block(x, y, z)
    }

    /// Approximation of `NoiseBasedChunkGenerator.getBaseHeight` for
    /// `WORLD_SURFACE_WG`: scan the final-density column from the top and
    /// return the first non-air level above sea level.
    ///
    /// Vanilla evaluates an interpolated `NoiseChunk`; this uses the exact
    /// final-density router per block, so heights can differ by a block or two
    /// at cell edges. It is only used for structure start height sampling in
    /// neighbouring chunks, where the alternative is no terrain source at all.
    pub fn base_height(&self, x: i32, z: i32) -> i32 {
        let density = self.router.final_density();
        let max_y = self.min_y + self.height - 1;
        for y in (self.min_y..=max_y).rev() {
            if density.compute(x, y, z) > 0.0 {
                return y + 1;
            }
            if y <= self.sea_level {
                // The heightmap predicate is NOT_AIR, so sea-level fluid
                // counts even when the density column is empty.
                return self.sea_level + 1;
            }
        }
        self.min_y
    }

    /// Decoration steps run by `generate_chunk`. Keeping this explicit lets the
    /// ore tests generate the same terrain with and without a step so the exact
    /// fixture delta can be attributed.
    pub const DEFAULT_DECORATION_STEPS: [Decoration; 4] = [
        Decoration::UndergroundOres,
        Decoration::UndergroundDecoration,
        Decoration::VegetalDecoration,
        Decoration::TopLayerModification,
    ];

    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32) -> io::Result<Chunk> {
        self.generate_chunk_with_steps(chunk_x, chunk_z, &Self::DEFAULT_DECORATION_STEPS)
    }

    /// Generates one chunk with an explicit decoration-step list.
    pub fn generate_chunk_with_steps(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        steps: &[Decoration],
    ) -> io::Result<Chunk> {
        let cells_x = (16 / CELL_WIDTH + 1) as usize;
        let cells_z = cells_x;
        let cells_y = (self.height / CELL_HEIGHT + 1) as usize;
        let mut grid = vec![0.0f64; cells_x * cells_z * cells_y];

        let density = self.router.final_density();
        for xi in 0..cells_x {
            for zi in 0..cells_z {
                for yi in 0..cells_y {
                    let x = chunk_x * 16 + xi as i32 * CELL_WIDTH;
                    let y = self.min_y + yi as i32 * CELL_HEIGHT;
                    let z = chunk_z * 16 + zi as i32 * CELL_WIDTH;
                    grid[(xi * cells_z + zi) * cells_y + yi] = density.compute(x, y, z);
                }
            }
        }

        let min_section = self.min_y.div_euclid(16);
        let section_count = self.height / 16;
        let mut sections = Vec::with_capacity(section_count as usize);
        for index in 0..section_count {
            sections.push(ChunkSection {
                y: (min_section + index) as i8,
                block_states: PalettedContainer {
                    palette: vec![air()],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
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

        let mut aquifer = Aquifer::new(
            chunk_x,
            chunk_z,
            &self.router,
            &self.aquifer_factory,
            self.min_y,
            self.height,
            self.sea_level,
        );

        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        let mut solid = vec![false; self.height as usize];
        let mut fluids: Vec<Option<BlockState>> = vec![None; self.height as usize];
        for local_x in 0..16 {
            for local_z in 0..16 {
                let world_x = chunk_x * 16 + local_x;
                let world_z = chunk_z * 16 + local_z;
                // The column buffers are reused, so clear them: leaving a
                // previous column's solid/fluid flags in place leaks terrain
                // sideways into neighbouring columns.
                solid.fill(false);
                fluids.fill(None);

                for index in 0..self.height as usize {
                    let y = self.min_y + index as i32;
                    let value = sample_grid(
                        &grid,
                        cells_x,
                        cells_z,
                        cells_y,
                        CELL_WIDTH,
                        CELL_HEIGHT,
                        local_x,
                        local_z,
                        y - self.min_y,
                    );
                    if value > 0.0 {
                        solid[index] = true;
                    } else {
                        match aquifer.compute_substance(world_x, y, world_z, value) {
                            AquiferSubstance::Solid => solid[index] = true,
                            AquiferSubstance::Air => {}
                            AquiferSubstance::Fluid(kind) => {
                                fluids[index] = Some(kind.block_state());
                            }
                        }
                    }
                }

                for index in 0..self.height as usize {
                    let y = self.min_y + index as i32;
                    if solid[index] {
                        // Vanilla's block-state filler chain runs the ore-vein
                        // pass for solid positions before surface rules and
                        // carvers. It replaces granite/tuff/copper/iron vein
                        // blocks; `None` keeps the base stone.
                        if let Some(state) = self.ore_veins.block_at(world_x, y, world_z) {
                            chunk.set_block_state(local_x as usize, y, local_z as usize, state);
                        } else {
                            chunk.set_block_state_ref(
                                local_x as usize,
                                y,
                                local_z as usize,
                                &stone,
                            );
                        }
                    } else if let Some(state) = fluids[index].take() {
                        chunk.set_block_state(local_x as usize, y, local_z as usize, state);
                    }
                }
            }
        }

        // Sample every quart biome cell once: the surface rules and the chunk
        // biome palettes both use this array.
        let biome_indices = self.sample_biome_indices(chunk_x, chunk_z);
        let preliminary_surface = self.router.function("preliminary_surface_level")?;
        surface::build_surface(
            &self.surface,
            &self.surface_rules,
            preliminary_surface.as_ref(),
            &self.biomes,
            &biome_indices,
            &mut chunk,
        );

        // Vanilla runs carvers after the surface pass, so they can cut through
        // grass and dirt and then ask the surface rules for a replacement floor.
        let carver_context = CarverContext::new(
            &self.surface,
            &self.surface_rules,
            preliminary_surface.as_ref(),
            &self.biomes,
            &biome_indices,
            self.min_y,
            self.height,
        );
        self.carvers
            .apply(&carver_context, &mut chunk, &mut aquifer, self.seed);

        let mut biome_cursor = 0usize;
        for section in &mut chunk.sections {
            let mut palette: Vec<String> = Vec::new();
            let mut indices = Vec::with_capacity(64);
            for _ in 0..64 {
                let entry = biome_indices[biome_cursor];
                biome_cursor += 1;
                let biome = self.biomes.biome_name(entry).to_owned();
                let index = match palette.iter().position(|value| value == &biome) {
                    Some(index) => index,
                    None => {
                        palette.push(biome);
                        palette.len() - 1
                    }
                };
                indices.push(index as u16);
            }
            section.biomes = Some(PalettedContainer { palette, indices });
        }

        // Biomes are now stored in the chunk, which the biome placement filter
        // reads through `FeaturePlacementWorld`.
        self.apply_decorations(&mut chunk, &biome_indices, steps)?;

        Ok(chunk)
    }

    /// Runs the decoration steps this port implements for every biome that
    /// appears in this chunk.
    ///
    /// Vanilla seeds each placed feature independently:
    /// `WorldgenRandom.setFeatureSeed(decorationSeed, featureIndex, step)`.
    /// The feature index is the position in `FeatureSorter`'s global list for
    /// the step, not the order a biome happens to list it in.
    fn apply_decorations(
        &self,
        chunk: &mut Chunk,
        biome_indices: &[usize],
        steps: &[Decoration],
    ) -> io::Result<()> {
        let data = FeatureData::vanilla();
        let mut chunk_biomes = std::collections::HashSet::new();
        for entry in biome_indices {
            chunk_biomes.insert(self.biomes.biome_name(*entry));
        }

        let step_order = data.step_feature_order();
        let mut worldgen_random = WorldgenRandom::new(0);
        let decoration_seed = worldgen_random.set_decoration_seed(
            self.seed,
            chunk.x.wrapping_mul(16),
            chunk.z.wrapping_mul(16),
        );
        let origin = BlockPos::new(
            chunk.x.wrapping_mul(16),
            self.min_y,
            chunk.z.wrapping_mul(16),
        );
        let mut structures_placed = false;
        for step in steps {
            let step = *step;
            // Vanilla places structure pieces for a step before the features
            // of that step. Surface structures are the only step this slice
            // generates; run them before the first feature step at or after
            // `surface_structures`.
            if !structures_placed
                && self.structures.is_some()
                && step.index() >= Decoration::SurfaceStructures.index()
            {
                self.apply_structures(chunk, Decoration::SurfaceStructures)?;
                structures_placed = true;
            }
            let mut active = std::collections::HashSet::new();
            for biome in &chunk_biomes {
                let Some(steps) = data.biome_features(biome) else {
                    continue;
                };
                for feature in &steps[step.index()] {
                    active.insert(feature.as_str());
                }
            }
            if active.is_empty() {
                continue;
            }

            for (index, id) in step_order[step.index()].iter().enumerate() {
                if !active.contains(id.as_str()) {
                    continue;
                }
                let Some(placed) = data.placed(id) else {
                    continue;
                };
                if !placed.placement_is_supported() {
                    continue;
                }
                let Some(configured) = data.configured(placed.feature()) else {
                    continue;
                };
                if !configured.is_supported() {
                    continue;
                }
                // The `underground_ores` step also contains disks and
                // `underwater_magma`; this slice only wires the ore features.
                if step == Decoration::UndergroundOres
                    && !matches!(
                        configured.vanilla_type(),
                        "minecraft:ore" | "minecraft:scattered_ore"
                    )
                {
                    continue;
                }
                worldgen_random.set_feature_seed(
                    decoration_seed,
                    index as i32,
                    step.index() as i32,
                );
                self.place_placed_feature(placed, configured, chunk, &mut worldgen_random, origin)?;
            }
        }
        Ok(())
    }

    /// Places structure starts whose pieces intersect the chunk currently
    /// being generated. Selection is biome-filtered through the generator's
    /// own sampler at sea level and restricted to the Overworld.
    fn apply_structures(&self, chunk: &mut Chunk, step: Decoration) -> io::Result<()> {
        let Some(structures) = &self.structures else {
            return Ok(());
        };
        let chunk_pos = crate::region::ChunkPos::new(chunk.x, chunk.z);
        let biomes = GeneratorBiomeLookup {
            sampler: &self.biomes,
        };
        let height_source = GeneratorHeightLevel {
            generator: self,
            cache: RefCell::new(HashMap::new()),
        };
        let mut region = WorldGenRegion::new(chunk, self.min_y, self.height);
        structures
            .place_for_chunk_with_height_source(
                &mut region,
                &height_source,
                chunk_pos,
                self.seed,
                StructureDimension::Overworld,
                self.sea_level,
                &biomes,
                8,
                step,
            )
            .map(|_| ())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
    }

    /// Runs one placed feature with vanilla's lazy modifier stream.
    ///
    /// `PlacedFeature.placeWithContext` is a chained `Stream.flatMap`: after a
    /// modifier yields a position, the configured feature runs and consumes
    /// RNG *before* the next position is sampled. Collecting all positions
    /// first (as `PlacedFeature::placement_positions` does for inspection)
    /// desynchronises every attempt after the first for multi-placement
    /// features such as ores.
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
            let mut region = WorldGenRegion::new(chunk, self.min_y, self.height);
            return configured
                .place(&mut region, random, pos)
                .map(|_| ())
                .map_err(|feature| {
                    io::Error::new(io::ErrorKind::InvalidData, feature.to_string())
                });
        };
        let positions = {
            let world = FeaturePlacementWorld {
                chunk: &*chunk,
                data: FeatureData::vanilla(),
                min_y: self.min_y,
                height: self.height,
            };
            modifier
                .get_positions(pos, random, &world, placed.id(), self.min_y, self.height)
                .map_err(|unsupported| {
                    io::Error::new(io::ErrorKind::InvalidData, unsupported.to_string())
                })?
        };
        for next in positions {
            self.place_modifier_step(placed, configured, chunk, random, next, index + 1)?;
        }
        Ok(())
    }

    /// Samples the chunk's 1536 quart biome cells in
    /// `(quart_y, quart_z, quart_x)` order, matching the biome palette layout.
    fn sample_biome_indices(&self, chunk_x: i32, chunk_z: i32) -> Vec<usize> {
        let quart_layers = (self.height >> 2) as usize;
        let quart_y_start = self.min_y >> 2;
        let mut indices = vec![0usize; quart_layers * 16];
        for qz in 0..4usize {
            for qx in 0..4usize {
                let column = self.biomes.sample_quart_column(
                    chunk_x * 4 + qx as i32,
                    chunk_z * 4 + qz as i32,
                    quart_y_start,
                    quart_layers,
                );
                for (layer, entry) in column.into_iter().enumerate() {
                    indices[(layer * 4 + qz) * 4 + qx] = entry;
                }
            }
        }
        indices
    }
}

/// Height-only [`FeatureLevel`] for structure generation in neighbouring
/// chunks. Writes are not supported; placement still goes through the
/// writable [`WorldGenRegion`].
struct GeneratorHeightLevel<'a> {
    generator: &'a WorldGenerator,
    cache: RefCell<HashMap<(i32, i32), i32>>,
}

impl FeatureLevel for GeneratorHeightLevel<'_> {
    fn min_y(&self) -> i32 {
        self.generator.min_y
    }

    fn height(&self) -> i32 {
        self.generator.height
    }

    fn get_block(&self, _pos: BlockPos) -> Option<&BlockState> {
        None
    }

    fn set_block(&mut self, _pos: BlockPos, _state: BlockState) -> bool {
        false
    }

    fn heightmap(&self, _kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        if let Some(height) = self.cache.borrow().get(&(x, z)) {
            return Some(*height);
        }
        let height = self.generator.base_height(x, z);
        self.cache.borrow_mut().insert((x, z), height);
        Some(height)
    }
}

/// [`StructureBiomeLookup`] backed by the generator's multi-noise sampler.
struct GeneratorBiomeLookup<'a> {
    sampler: &'a BiomeSampler,
}

impl StructureBiomeLookup for GeneratorBiomeLookup<'_> {
    fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.sampler.sample_block(x, y, z)
    }
}

/// [`PlacementWorld`] backed by the chunk currently being generated.
///
/// Heightmaps are recomputed from the block states, so later trees in the same
/// chunk see the leaves and logs placed by earlier ones, matching the updated
/// heightmaps of a live `WorldGenLevel`.
struct FeaturePlacementWorld<'a> {
    chunk: &'a Chunk,
    data: &'a FeatureData,
    min_y: i32,
    height: i32,
}

impl FeaturePlacementWorld<'_> {
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

impl PlacementWorld for FeaturePlacementWorld<'_> {
    fn biome_at(&self, pos: BlockPos) -> &str {
        if let Some((x, z)) = self.local_column(pos.x(), pos.z()) {
            let max_y = self.min_y + self.height - 1;
            let y = pos.y().clamp(self.min_y, max_y);
            if let Some(biome) = self.chunk.biome(x, y, z) {
                return biome;
            }
        }
        self.chunk
            .biome(0, self.min_y, 0)
            .map_or("minecraft:plains", |biome| biome.as_str())
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
        for y in (self.min_y..self.min_y + self.height).rev() {
            let Some(state) = self.chunk.block_state(x, y, z) else {
                continue;
            };
            let fluid = is_fluid_state(state);
            let blocks_motion = match kind {
                Heightmap::WorldSurface | Heightmap::WorldSurfaceWg => !is_air_state(state),
                Heightmap::OceanFloor | Heightmap::OceanFloorWg => heightmap_blocks_motion(state),
                Heightmap::MotionBlocking => heightmap_blocks_motion(state) || fluid,
                Heightmap::MotionBlockingNoLeaves => {
                    (heightmap_blocks_motion(state) || fluid)
                        && !crate::tags::block_has("minecraft:leaves", &state.name)
                }
            };
            if blocks_motion {
                return Some(y + 1);
            }
        }
        Some(self.min_y)
    }

    fn block_at(&self, pos: BlockPos) -> Option<&BlockState> {
        let (x, z) = self.local_column(pos.x(), pos.z())?;
        self.chunk.block_state(x, pos.y(), z)
    }
}

fn is_fluid_state(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:water" | "minecraft:lava" | "minecraft:flowing_water" | "minecraft:flowing_lava"
    )
}

/// Approximation of `BlockState.blocksMotion` until P1.5 block metadata lands:
/// air, fluids, and tagged passable vegetation do not block motion.
fn heightmap_blocks_motion(state: &BlockState) -> bool {
    if is_air_state(state) || is_fluid_state(state) {
        return false;
    }
    for tag in [
        "minecraft:replaceable",
        "minecraft:flowers",
        "minecraft:saplings",
        "minecraft:crops",
        "minecraft:rails",
        "minecraft:buttons",
        "minecraft:pressure_plates",
        "minecraft:all_signs",
        "minecraft:fire",
    ] {
        if crate::tags::block_has(tag, &state.name) {
            return false;
        }
    }
    true
}

/// Trilinear interpolation of a vanilla density grid.
///
/// `cell_width` / `cell_height` are `NoiseSettings.getCellWidth` and
/// `getCellHeight` (size horizontal/vertical times four).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sample_grid(
    grid: &[f64],
    cells_x: usize,
    cells_z: usize,
    cells_y: usize,
    cell_width: i32,
    cell_height: i32,
    local_x: i32,
    local_z: i32,
    local_y: i32,
) -> f64 {
    let cell_x = (local_x / cell_width).clamp(0, cells_x as i32 - 2) as usize;
    let cell_z = (local_z / cell_width).clamp(0, cells_z as i32 - 2) as usize;
    let cell_y = (local_y / cell_height).clamp(0, cells_y as i32 - 2) as usize;
    let frac_x = f64::from(local_x.rem_euclid(cell_width)) / f64::from(cell_width);
    let frac_z = f64::from(local_z.rem_euclid(cell_width)) / f64::from(cell_width);
    let frac_y = f64::from(local_y.rem_euclid(cell_height)) / f64::from(cell_height);

    let value = |x: usize, y: usize, z: usize| grid[(x * cells_z + z) * cells_y + y];
    let x0 = cell_x;
    let x1 = cell_x + 1;
    let z0 = cell_z;
    let z1 = cell_z + 1;
    let y0 = cell_y;
    let y1 = cell_y + 1;

    let v000 = value(x0, y0, z0);
    let v100 = value(x1, y0, z0);
    let v010 = value(x0, y1, z0);
    let v110 = value(x1, y1, z0);
    let v001 = value(x0, y0, z1);
    let v101 = value(x1, y0, z1);
    let v011 = value(x0, y1, z1);
    let v111 = value(x1, y1, z1);

    lerp(
        frac_z,
        lerp2(frac_x, frac_y, v000, v100, v010, v110),
        lerp2(frac_x, frac_y, v001, v101, v011, v111),
    )
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

fn lerp2(delta1: f64, delta2: f64, start1: f64, end1: f64, start2: f64, end2: f64) -> f64 {
    lerp(
        delta2,
        lerp(delta1, start1, end1),
        lerp(delta1, start2, end2),
    )
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const JAVA_GRID: &str = include_str!("../../assets/java-density-grid.txt");
    const JAVA_CHUNK: &str = include_str!("../../assets/java-chunk-0--1.txt");
    const JAVA_BIOMES: &str = include_str!("../../assets/java-biome-samples.txt");

    #[test]
    fn generated_chunk_biomes_match_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();
        let mut checked = 0;
        for line in JAVA_BIOMES.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("biome") {
                continue;
            }
            let qx: i32 = parts.next().unwrap().parse().unwrap();
            let qy: i32 = parts.next().unwrap().parse().unwrap();
            let qz: i32 = parts.next().unwrap().parse().unwrap();
            let expected = parts.next().unwrap();
            let local_x = (qx * 4).rem_euclid(16) as usize;
            let local_z = (qz * 4).rem_euclid(16) as usize;
            let actual = chunk.biome(local_x, qy * 4, local_z).unwrap();
            assert_eq!(
                actual, expected,
                "chunk biome mismatch at quart ({qx},{qy},{qz})"
            );
            checked += 1;
        }
        assert_eq!(checked, 1536);
    }

    #[test]
    fn generator_grid_matches_java_density_grid() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let mut checked = 0;
        for line in JAVA_GRID.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let first = parts.next().unwrap();
            if first == "seed" {
                continue;
            }
            let x: i32 = first.parse().unwrap();
            let y: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let expected: f64 = parts.next().unwrap().parse().unwrap();
            let actual = generator.density(x, y, z);
            assert!(
                (actual - expected).abs() < 1.0e-9,
                "density ({x},{y},{z}): {actual} != {expected}"
            );
            checked += 1;
        }
        assert_eq!(checked, 5 * 5 * 49);
    }

    #[test]
    fn forest_chunk_contains_trees_and_is_deterministic() {
        let generator = WorldGenerator::overworld(0).unwrap();
        let chunk = generator.generate_chunk(0, 0).unwrap();
        let again = generator.generate_chunk(0, 0).unwrap();
        assert_eq!(chunk, again, "tree placement must be deterministic");

        let mut logs = 0;
        let mut leaves = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    let state = chunk.block_state(x, y, z).unwrap();
                    if state.name.ends_with("_log") {
                        logs += 1;
                    } else if state.name.ends_with("_leaves") {
                        leaves += 1;
                    }
                }
            }
        }
        println!("forest chunk logs={logs} leaves={leaves}");
        assert!(logs > 0, "a forest chunk should contain tree trunks");
        assert!(leaves > 0, "a forest chunk should contain tree foliage");
    }

    #[test]
    fn generated_chunk_is_not_flat_and_has_water() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();
        let mut heights = Vec::new();
        let mut water = 0;
        for x in 0..16 {
            for z in 0..16 {
                let mut top = generator.min_y();
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    let state = chunk.block_state(x, y, z).unwrap();
                    if state.name == "minecraft:water" {
                        water += 1;
                    }
                    if state.name != "minecraft:air" && state.name != "minecraft:water" {
                        top = y;
                    }
                }
                heights.push(top);
            }
        }
        let min = *heights.iter().min().unwrap();
        let max = *heights.iter().max().unwrap();
        println!("generated chunk top-solid range: {min}..{max}, water blocks: {water}");
        assert!(max - min >= 4, "terrain is too flat: min={min} max={max}");
        assert!(water > 0, "expected sea-level water");
    }

    #[test]
    fn bedrock_layers_match_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut java = Vec::new();
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                if block == "minecraft:bedrock" {
                    java.push(y.parse::<i32>().unwrap());
                }
            }
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            let mut rust = Vec::new();
            for y in -64..-58 {
                if chunk.block_state(local_x, y, local_z).unwrap().name == "minecraft:bedrock" {
                    rust.push(y);
                }
            }
            assert_eq!(rust, java, "bedrock layers differ at column ({x},{z})");
        }
    }

    #[test]
    fn generated_chunk_block_classes_are_close_to_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        let mut matched = 0u64;
        let mut exact_matched = 0u64;
        let mut total = 0u64;
        let mut water_matched = 0u64;
        let mut java_water = 0u64;
        let mut rust_water = 0u64;

        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut java: std::collections::HashMap<i32, &str> = std::collections::HashMap::new();
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                java.insert(y.parse::<i32>().unwrap(), block);
            }
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            for y in -64..320 {
                let state = chunk.block_state(local_x, y, local_z).unwrap();
                let rust = rust_class(&state.name);
                let expected_name = java.get(&y).copied().unwrap_or("minecraft:air");
                let expected = java_class(expected_name);
                if state.name == expected_name {
                    exact_matched += 1;
                }
                if rust == expected {
                    matched += 1;
                }
                if rust == 2 {
                    rust_water += 1;
                }
                if expected == 2 {
                    java_water += 1;
                }
                if rust == 2 && expected == 2 {
                    water_matched += 1;
                }
                total += 1;
            }
        }
        let agreement = matched as f64 / total as f64;
        println!(
            "block-class agreement: {:.1}% ({matched}/{total}), exact names: {:.1}%, water Rust={rust_water} Java={java_water} matched={water_matched}",
            agreement * 100.0,
            exact_matched as f64 / total as f64 * 100.0
        );
        assert!(
            agreement > 0.80,
            "block-class agreement too low: {agreement}"
        );
    }

    fn java_class(block: &str) -> u8 {
        match block {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
            "minecraft:water" => 2,
            "minecraft:lava" => 3,
            _ => 1,
        }
    }

    fn rust_class(name: &str) -> u8 {
        match name {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
            "minecraft:water" => 2,
            "minecraft:lava" => 3,
            _ => 1,
        }
    }

    #[test]
    fn generated_surface_height_is_close_to_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        let mut java_heights: HashMap<(i32, i32), i32> = HashMap::new();
        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut top = -64;
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                let y: i32 = y.parse().unwrap();
                if !matches!(
                    block,
                    "minecraft:air"
                        | "minecraft:cave_air"
                        | "minecraft:water"
                        | "minecraft:lava"
                        | "minecraft:short_grass"
                        | "minecraft:seagrass"
                        | "minecraft:moss_carpet"
                ) {
                    top = top.max(y);
                }
            }
            java_heights.insert((x, z), top);
        }

        let mut total_difference = 0;
        let mut count = 0;
        for ((x, z), java_top) in java_heights {
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            let mut rust_top = -64;
            for y in -64..320 {
                let state = chunk.block_state(local_x, y, local_z).unwrap();
                if !matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:water" | "minecraft:cave_air"
                ) {
                    rust_top = y;
                }
            }
            total_difference += (rust_top - java_top).abs();
            count += 1;
        }
        let average = f64::from(total_difference) / f64::from(count);
        println!("Java/Rust surface-height columns: {count}, average difference: {average:.2}");
        assert!(
            average < 12.0,
            "average surface height difference too large: {average}"
        );
    }

    #[test]
    fn feature_order_uses_the_biome_parameter_order() {
        let order = FeatureData::vanilla().step_feature_order();
        let ores = &order[Decoration::UndergroundOres.index()];
        assert_eq!(ores.len(), 34);
        assert_eq!(ores[0], "minecraft:ore_dirt");
        assert_eq!(ores[10], "minecraft:ore_coal_lower");
        // The registry-order list puts `disk_grass` here instead, which gives
        // `ore_clay` the wrong feature seed; the parameter order is vanilla's
        // `MultiNoiseBiomeSource.possibleBiomes()` order.
        assert_eq!(ores[27], "minecraft:ore_clay");
        assert_eq!(ores[28], "minecraft:ore_gold_extra");
        assert_eq!(ores[29], "minecraft:disk_grass");
        assert_eq!(ores[33], "minecraft:ore_emerald");
    }

    #[test]
    fn ore_veins_fill_the_density_pass() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();
        let mut granite = 0;
        let mut tuff = 0;
        let mut copper = 0;
        let mut iron = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    match chunk.block_state(x, y, z).unwrap().name.as_str() {
                        "minecraft:granite" => granite += 1,
                        "minecraft:tuff" => tuff += 1,
                        "minecraft:copper_ore" | "minecraft:raw_copper_block" => copper += 1,
                        "minecraft:deepslate_iron_ore" | "minecraft:raw_iron_block" => iron += 1,
                        _ => {}
                    }
                }
            }
        }
        println!("vein blocks: granite={granite} tuff={tuff} copper={copper} iron={iron}");
        assert!(granite > 0, "copper veins should write granite filler");
        assert!(tuff > 0, "iron veins should write tuff filler");
        assert!(copper > 0, "copper veins should write copper ore");
        assert!(iron > 0, "iron veins should write deepslate iron ore");
    }
}
