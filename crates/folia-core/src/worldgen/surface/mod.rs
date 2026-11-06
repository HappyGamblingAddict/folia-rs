//! Port of vanilla `SurfaceRules` and the `SurfaceSystem.buildSurface` driver.
//!
//! The rule tree is read from the embedded `surface_rule` in
//! `assets/worldgen.json`, which is the real tree the Java server used
//! (including Paper's `optionally_flat_bedrock_condition_source`). Rules are
//! evaluated per stone block, exactly like `SurfaceSystem.buildSurface`:
//! a downward column walk tracks the stone depth above/below and the water
//! height, and the first matching rule replaces the default block.

pub mod system;

use crate::chunk::{BlockState, Chunk};
use crate::worldgen::biome::BiomeSampler;
use crate::worldgen::density::DensityFunction;
use crate::worldgen::surface::system::SurfaceSystem;
use folia_random::{RandomSource, XoroshiroPositionalRandomFactory};
use serde_json::Value;
use std::io;

/// `net.minecraft.world.level.levelgen.VerticalAnchor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl VerticalAnchor {
    /// Java: `VerticalAnchor.resolveY(WorldGenerationContext)`.
    pub fn resolve(self, min_y: i32, height: i32) -> i32 {
        match self {
            Self::Absolute(y) => y,
            Self::AboveBottom(offset) => min_y + offset,
            Self::BelowTop(offset) => height - 1 + min_y - offset,
        }
    }

    fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("vertical anchor must be an object"))?;
        if let Some(y) = object.get("absolute").and_then(Value::as_i64) {
            return Ok(Self::Absolute(y as i32));
        }
        if let Some(offset) = object.get("above_bottom").and_then(Value::as_i64) {
            return Ok(Self::AboveBottom(offset as i32));
        }
        if let Some(offset) = object.get("below_top").and_then(Value::as_i64) {
            return Ok(Self::BelowTop(offset as i32));
        }
        Err(invalid("unknown vertical anchor"))
    }
}

/// `SurfaceRules.ConditionSource` with vertical gradient factories resolved at
/// parse time.
#[derive(Debug, Clone)]
pub enum ConditionSource {
    Biome(Vec<String>),
    NoiseThreshold {
        noise: String,
        min_threshold: f64,
        max_threshold: f64,
    },
    VerticalGradient {
        factory: XoroshiroPositionalRandomFactory,
        true_at_and_below: i32,
        false_at_and_above: i32,
    },
    Y {
        anchor: VerticalAnchor,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Water {
        offset: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    StoneDepth {
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    },
    Not(Box<ConditionSource>),
    Temperature,
    Steep,
    Hole,
    AbovePreliminarySurface,
}

impl ConditionSource {
    fn test(&self, context: &Context<'_>) -> bool {
        match self {
            Self::Biome(names) => names.iter().any(|name| name == context.biome()),
            Self::NoiseThreshold {
                noise,
                min_threshold,
                max_threshold,
            } => {
                let Some(noise) = context.system.noise(noise) else {
                    return false;
                };
                let value =
                    noise.get_value(f64::from(context.block_x), 0.0, f64::from(context.block_z));
                value >= *min_threshold && value <= *max_threshold
            }
            Self::VerticalGradient {
                factory,
                true_at_and_below,
                false_at_and_above,
            } => {
                let y = context.block_y;
                if y <= *true_at_and_below {
                    return true;
                }
                if y >= *false_at_and_above {
                    return false;
                }
                let delta = (f64::from(y) - f64::from(*true_at_and_below))
                    / (f64::from(*false_at_and_above) - f64::from(*true_at_and_below));
                // Mth.map(y, true, false, 1.0, 0.0)
                let probability = 1.0 - delta;
                let mut random = factory.at(context.block_x, y, context.block_z);
                f64::from(random.next_float()) < probability
            }
            Self::Y {
                anchor,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                let stone_depth = if *add_stone_depth {
                    context.stone_depth_above
                } else {
                    0
                };
                context.block_y + stone_depth
                    >= anchor.resolve(context.min_y, context.height)
                        + context.surface_depth * surface_depth_multiplier
            }
            Self::Water {
                offset,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                if context.water_height == i32::MIN {
                    return true;
                }
                let stone_depth = if *add_stone_depth {
                    context.stone_depth_above
                } else {
                    0
                };
                context.block_y + stone_depth
                    >= context.water_height
                        + offset
                        + context.surface_depth * surface_depth_multiplier
            }
            Self::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                ceiling,
            } => {
                let depth = if *ceiling {
                    context.stone_depth_below
                } else {
                    context.stone_depth_above
                };
                let surface_depth = if *add_surface_depth {
                    context.surface_depth
                } else {
                    0
                };
                let secondary = if *secondary_depth_range == 0 {
                    0
                } else {
                    map(
                        context.surface_secondary,
                        -1.0,
                        1.0,
                        0.0,
                        f64::from(*secondary_depth_range),
                    ) as i32
                };
                depth <= 1 + offset + surface_depth + secondary
            }
            Self::Not(target) => !target.test(context),
            Self::Temperature => {
                cold_enough_to_snow(context.biome(), context.block_y, context.system.sea_level())
            }
            Self::Steep => steep(context),
            Self::Hole => context.surface_depth <= 0,
            Self::AbovePreliminarySurface => context.block_y >= context.min_surface_level,
        }
    }
}

/// `SurfaceRules.RuleSource` with blocks parsed from the data pack.
#[derive(Debug, Clone)]
pub enum RuleSource {
    Block(BlockState),
    Sequence(Vec<RuleSource>),
    Test {
        condition: ConditionSource,
        then_run: Box<RuleSource>,
    },
    Bandlands,
}

impl RuleSource {
    /// Returns the first matching replacement state, if any.
    pub fn try_apply(&self, context: &Context<'_>) -> Option<BlockState> {
        match self {
            Self::Block(state) => Some(state.clone()),
            Self::Sequence(rules) => rules.iter().find_map(|rule| rule.try_apply(context)),
            Self::Test {
                condition,
                then_run,
            } => {
                if condition.test(context) {
                    then_run.try_apply(context)
                } else {
                    None
                }
            }
            Self::Bandlands => Some(context.system.get_band(
                context.block_x,
                context.block_y,
                context.block_z,
            )),
        }
    }
}

/// Parses a `surface_rule` JSON tree. Vertical-gradient random factories are
/// derived from the level's positional factory, matching
/// `RandomState.getOrCreateRandomFactory`.
pub fn parse_rules(
    value: &Value,
    seed: &XoroshiroPositionalRandomFactory,
    min_y: i32,
    height: i32,
) -> io::Result<RuleSource> {
    parse_rule(value, seed, min_y, height)
}

fn parse_rule(
    value: &Value,
    seed: &XoroshiroPositionalRandomFactory,
    min_y: i32,
    height: i32,
) -> io::Result<RuleSource> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("surface rule is missing type"))?;
    match kind {
        "minecraft:sequence" => {
            let sequence = value
                .get("sequence")
                .or_else(|| value.get("rules"))
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("sequence rule is missing sequence"))?;
            let rules = sequence
                .iter()
                .map(|rule| parse_rule(rule, seed, min_y, height))
                .collect::<io::Result<Vec<_>>>()?;
            Ok(RuleSource::Sequence(rules))
        }
        "minecraft:block" => {
            let state = value
                .get("result_state")
                .ok_or_else(|| invalid("block rule is missing result_state"))?;
            Ok(RuleSource::Block(parse_block_state(state)?))
        }
        "minecraft:condition" => {
            let if_true = value
                .get("if_true")
                .ok_or_else(|| invalid("condition rule is missing if_true"))?;
            let then_run = value
                .get("then_run")
                .ok_or_else(|| invalid("condition rule is missing then_run"))?;
            Ok(RuleSource::Test {
                condition: parse_condition(if_true, seed, min_y, height)?,
                then_run: Box::new(parse_rule(then_run, seed, min_y, height)?),
            })
        }
        "minecraft:bandlands" => Ok(RuleSource::Bandlands),
        other => Err(invalid(format!("unknown surface rule {other}"))),
    }
}

fn parse_condition(
    value: &Value,
    seed: &XoroshiroPositionalRandomFactory,
    min_y: i32,
    height: i32,
) -> io::Result<ConditionSource> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("surface condition is missing type"))?;
    Ok(match kind {
        "minecraft:biome" => {
            let names = value
                .get("biome_is")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("biome condition is missing biome_is"))?
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("biome name must be a string"))
                })
                .collect::<io::Result<Vec<_>>>()?;
            ConditionSource::Biome(names)
        }
        "minecraft:noise_threshold" => ConditionSource::NoiseThreshold {
            noise: string(value, "noise")?.to_owned(),
            min_threshold: number(value, "min_threshold")?,
            max_threshold: number(value, "max_threshold")?,
        },
        "minecraft:vertical_gradient" => {
            let name = string(value, "random_name")?;
            let factory = seed.from_hash_of(name).fork_positional();
            ConditionSource::VerticalGradient {
                factory,
                true_at_and_below: anchor(value, "true_at_and_below")?.resolve(min_y, height),
                false_at_and_above: anchor(value, "false_at_and_above")?.resolve(min_y, height),
            }
        }
        // Paper's replacement for the vanilla bedrock gradient. With
        // `generate-flat-bedrock` disabled (the setting the captured world
        // used, confirmed by the oracle's random bedrock depths) it behaves
        // exactly like a vertical gradient.
        "paper:optionally_flat_bedrock_condition_source" => {
            let name = string(value, "random_name")?;
            let factory = seed.from_hash_of(name).fork_positional();
            ConditionSource::VerticalGradient {
                factory,
                true_at_and_below: anchor(value, "true_at_and_below")?.resolve(min_y, height),
                false_at_and_above: anchor(value, "false_at_and_above")?.resolve(min_y, height),
            }
        }
        "minecraft:y_above" => ConditionSource::Y {
            anchor: anchor(value, "anchor")?,
            surface_depth_multiplier: integer(value, "surface_depth_multiplier")?,
            add_stone_depth: boolean(value, "add_stone_depth")?,
        },
        "minecraft:water" => ConditionSource::Water {
            offset: integer(value, "offset")?,
            surface_depth_multiplier: integer(value, "surface_depth_multiplier")?,
            add_stone_depth: boolean(value, "add_stone_depth")?,
        },
        "minecraft:stone_depth" => ConditionSource::StoneDepth {
            offset: integer(value, "offset")?,
            add_surface_depth: boolean(value, "add_surface_depth")?,
            secondary_depth_range: integer(value, "secondary_depth_range")?,
            ceiling: string(value, "surface_type")? == "ceiling",
        },
        "minecraft:not" => {
            let target = value
                .get("invert")
                .ok_or_else(|| invalid("not condition is missing invert"))?;
            ConditionSource::Not(Box::new(parse_condition(target, seed, min_y, height)?))
        }
        "minecraft:temperature" => ConditionSource::Temperature,
        "minecraft:steep" => ConditionSource::Steep,
        "minecraft:hole" => ConditionSource::Hole,
        "minecraft:above_preliminary_surface" => ConditionSource::AbovePreliminarySurface,
        other => return Err(invalid(format!("unknown surface condition {other}"))),
    })
}

fn parse_block_state(value: &Value) -> io::Result<BlockState> {
    let name = value
        .get("Name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("block state is missing Name"))?;
    let mut properties = Vec::new();
    if let Some(map) = value.get("Properties").and_then(Value::as_object) {
        for (key, value) in map {
            let value = value
                .as_str()
                .ok_or_else(|| invalid("block state property must be a string"))?;
            properties.push((key.clone(), value.to_owned()));
        }
    }
    properties.sort();
    Ok(BlockState {
        name: name.to_owned(),
        properties,
    })
}

fn string<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("missing string field {key}")))
}

fn number(value: &Value, key: &str) -> io::Result<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| invalid(format!("missing number field {key}")))
}

fn integer(value: &Value, key: &str) -> io::Result<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .map(|number| number as i32)
        .ok_or_else(|| invalid(format!("missing integer field {key}")))
}

fn boolean(value: &Value, key: &str) -> io::Result<bool> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid(format!("missing boolean field {key}")))
}

fn anchor(value: &Value, key: &str) -> io::Result<VerticalAnchor> {
    let anchor = value
        .get(key)
        .ok_or_else(|| invalid(format!("missing anchor field {key}")))?;
    VerticalAnchor::parse(anchor)
}

/// Runtime state for `SurfaceRules.Context`.
pub struct Context<'a> {
    system: &'a SurfaceSystem,
    biomes: &'a BiomeSampler,
    biome_indices: &'a [usize],
    preliminary_surface: &'a DensityFunction,
    /// First-air height (`Heightmap.WORLD_SURFACE_WG` value) per column,
    /// indexed by `local_x * 16 + local_z`.
    heights: [i32; 256],
    min_y: i32,
    height: i32,
    preliminary_origin: Option<(i32, i32)>,
    preliminary: [i32; 4],
    block_x: i32,
    block_z: i32,
    surface_depth: i32,
    surface_secondary: f64,
    min_surface_level: i32,
    block_y: i32,
    water_height: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
}

impl<'a> Context<'a> {
    fn new(
        system: &'a SurfaceSystem,
        biomes: &'a BiomeSampler,
        biome_indices: &'a [usize],
        preliminary_surface: &'a DensityFunction,
        chunk: &Chunk,
    ) -> Self {
        let min_y = system.min_y();
        let height = system.height();
        let max_y = min_y + height - 1;
        let mut heights = [min_y; 256];
        for local_x in 0..16usize {
            for local_z in 0..16usize {
                heights[local_x * 16 + local_z] =
                    column_first_air(chunk, local_x, local_z, min_y, max_y);
            }
        }
        Self {
            system,
            biomes,
            biome_indices,
            preliminary_surface,
            heights,
            min_y,
            height,
            preliminary_origin: None,
            preliminary: [0; 4],
            block_x: 0,
            block_z: 0,
            surface_depth: 0,
            surface_secondary: 0.0,
            min_surface_level: 0,
            block_y: 0,
            water_height: i32::MIN,
            stone_depth_above: 0,
            stone_depth_below: i32::MAX,
        }
    }

    fn update_xz(&mut self, block_x: i32, block_z: i32) {
        self.block_x = block_x;
        self.block_z = block_z;
        self.surface_depth = self.system.get_surface_depth(block_x, block_z);
        self.surface_secondary = self.system.get_surface_secondary(block_x, block_z);
        self.min_surface_level = self.min_surface_level();
    }

    fn update_y(
        &mut self,
        stone_depth_above: i32,
        stone_depth_below: i32,
        water_height: i32,
        block_y: i32,
    ) {
        self.block_y = block_y;
        self.stone_depth_above = stone_depth_above;
        self.stone_depth_below = stone_depth_below;
        self.water_height = water_height;
    }

    /// Java: `SurfaceRules.Context.getMinSurfaceLevel`.
    fn min_surface_level(&mut self) -> i32 {
        let cell_x = self.block_x >> 4;
        let cell_z = self.block_z >> 4;
        if self.preliminary_origin != Some((cell_x, cell_z)) {
            self.preliminary_origin = Some((cell_x, cell_z));
            self.preliminary = [
                self.preliminary_surface_level(cell_x * 16, cell_z * 16),
                self.preliminary_surface_level((cell_x + 1) * 16, cell_z * 16),
                self.preliminary_surface_level(cell_x * 16, (cell_z + 1) * 16),
                self.preliminary_surface_level((cell_x + 1) * 16, (cell_z + 1) * 16),
            ];
        }
        let fraction_x = f64::from(self.block_x & 15) / 16.0;
        let fraction_z = f64::from(self.block_z & 15) / 16.0;
        let p = self.preliminary;
        let height = lerp(
            fraction_z,
            lerp(fraction_x, f64::from(p[0]), f64::from(p[1])),
            lerp(fraction_x, f64::from(p[2]), f64::from(p[3])),
        );
        height.floor() as i32 + self.surface_depth - 8
    }

    /// `NoiseChunk.preliminarySurfaceLevel`: quartz-quantized coordinates and a
    /// floor of the `preliminary_surface_level` density function.
    fn preliminary_surface_level(&self, x: i32, z: i32) -> i32 {
        let quantized_x = (x >> 2) << 2;
        let quantized_z = (z >> 2) << 2;
        self.preliminary_surface
            .compute(quantized_x, 0, quantized_z)
            .floor() as i32
    }

    fn biome(&self) -> &str {
        let local_x = (self.block_x & 15) >> 2;
        let local_z = (self.block_z & 15) >> 2;
        let quart_y = (self.block_y >> 2) - (self.min_y >> 2);
        let max_quart_y = (self.height >> 2).max(1);
        let quart_y = quart_y.clamp(0, max_quart_y - 1) as usize;
        let index = (quart_y * 4 + local_z as usize) * 4 + local_x as usize;
        let entry = self.biome_indices.get(index).copied().unwrap_or_default();
        self.biomes.biome_name(entry)
    }
}

/// `SurfaceSystem.buildSurface`.
#[allow(clippy::too_many_arguments)]
pub fn build_surface(
    system: &SurfaceSystem,
    rules: &RuleSource,
    preliminary_surface: &DensityFunction,
    biomes: &BiomeSampler,
    biome_indices: &[usize],
    chunk: &mut Chunk,
) {
    let min_y = system.min_y();
    let height = system.height();
    let max_y = min_y + height - 1;
    let mut context = Context::new(system, biomes, biome_indices, preliminary_surface, chunk);
    let default_block = system.default_block().clone();

    for local_x in 0..16usize {
        for local_z in 0..16usize {
            let world_x = chunk.x * 16 + local_x as i32;
            let world_z = chunk.z * 16 + local_z as i32;
            let column = local_x * 16 + local_z;

            let mut first_air = context.heights[column];
            // `i4` in Java is the pre-extension height used by both the
            // extended-badlands test and the frozen-ocean extension.
            let surface_y = first_air + 1;
            let surface_biome = biome_name_at(
                biomes,
                biome_indices,
                min_y,
                height,
                world_x,
                surface_y,
                world_z,
            );
            let melts_slightly = frozen_ocean_melt(surface_biome);

            if surface_biome == "minecraft:eroded_badlands" {
                system.eroded_badlands_extension(chunk, world_x, world_z, first_air + 1);
                first_air = column_first_air(chunk, local_x, local_z, min_y, max_y);
                context.heights[column] = first_air;
            }

            context.update_xz(world_x, world_z);
            let mut stone_depth_above = 0;
            let mut water_height = i32::MIN;
            let mut stone_depth_below = i32::MAX;

            let mut y = first_air + 1;
            while y >= min_y {
                let (is_air, is_fluid, is_default) =
                    block_flags(chunk, local_x, y, local_z, &default_block);

                if is_air {
                    stone_depth_above = 0;
                    water_height = i32::MIN;
                } else if is_fluid {
                    if water_height == i32::MIN {
                        water_height = y + 1;
                    }
                } else {
                    if stone_depth_below >= y {
                        stone_depth_below = -32512;
                        let mut below = y - 1;
                        while below >= min_y - 1 {
                            let (below_air, below_fluid, _) =
                                block_flags(chunk, local_x, below, local_z, &default_block);
                            if below_air || below_fluid {
                                stone_depth_below = below + 1;
                                break;
                            }
                            below -= 1;
                        }
                    }

                    stone_depth_above += 1;
                    let depth_below = y - stone_depth_below + 1;
                    context.update_y(stone_depth_above, depth_below, water_height, y);

                    if is_default && let Some(state) = rules.try_apply(&context) {
                        let _ = chunk.set_block_state(local_x, y, local_z, state);
                    }
                }

                y -= 1;
            }

            if let Some(melts_slightly) = melts_slightly {
                system.frozen_ocean_extension(
                    chunk,
                    context.min_surface_level,
                    melts_slightly,
                    world_x,
                    world_z,
                    surface_y,
                );
            }
        }
    }
}

/// Java: `SurfaceSystem.topMaterial`.
///
/// Carvers call this when they cut through grass or mycelium and expose dirt:
/// the surface rule is evaluated again at the dirt position with a stone
/// depth of one, so the exposed cave floor gets the same material it would
/// have received during the normal surface pass.
#[allow(clippy::too_many_arguments)]
pub fn top_material(
    system: &SurfaceSystem,
    rules: &RuleSource,
    preliminary_surface: &DensityFunction,
    biomes: &BiomeSampler,
    biome_indices: &[usize],
    chunk: &Chunk,
    block_x: i32,
    block_y: i32,
    block_z: i32,
    has_fluid: bool,
) -> Option<BlockState> {
    let mut context = Context::new(system, biomes, biome_indices, preliminary_surface, chunk);
    context.update_xz(block_x, block_z);
    context.update_y(
        1,
        1,
        if has_fluid { block_y + 1 } else { i32::MIN },
        block_y,
    );
    rules.try_apply(&context)
}

fn biome_name_at<'a>(
    biomes: &'a BiomeSampler,
    biome_indices: &[usize],
    min_y: i32,
    height: i32,
    x: i32,
    y: i32,
    z: i32,
) -> &'a str {
    let local_x = (x & 15) >> 2;
    let local_z = (z & 15) >> 2;
    let quart_y = (y >> 2) - (min_y >> 2);
    let max_quart_y = (height >> 2).max(1);
    let quart_y = quart_y.clamp(0, max_quart_y - 1) as usize;
    let index = (quart_y * 4 + local_z as usize) * 4 + local_x as usize;
    biomes.biome_name(biome_indices.get(index).copied().unwrap_or_default())
}

fn block_flags(
    chunk: &Chunk,
    local_x: usize,
    y: i32,
    local_z: usize,
    default_block: &BlockState,
) -> (bool, bool, bool) {
    match chunk.block_state(local_x, y, local_z) {
        Some(state) => (is_air(state), is_fluid(state), state == default_block),
        None => (true, false, false),
    }
}

fn column_first_air(chunk: &Chunk, local_x: usize, local_z: usize, min_y: i32, max_y: i32) -> i32 {
    for y in (min_y..=max_y).rev() {
        let Some(state) = chunk.block_state(local_x, y, local_z) else {
            continue;
        };
        if !is_air(state) {
            return y + 1;
        }
    }
    min_y
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

fn is_fluid(state: &BlockState) -> bool {
    matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
}

fn steep(context: &Context<'_>) -> bool {
    let local_x = (context.block_x & 15) as usize;
    let local_z = (context.block_z & 15) as usize;
    let min_z = local_z.saturating_sub(1);
    let max_z = (local_z + 1).min(15);
    let height = context.heights[local_x * 16 + min_z];
    let height_z = context.heights[local_x * 16 + max_z];
    if height_z >= height + 4 {
        return true;
    }
    let min_x = local_x.saturating_sub(1);
    let max_x = (local_x + 1).min(15);
    let height_x = context.heights[min_x * 16 + local_z];
    let height_x2 = context.heights[max_x * 16 + local_z];
    height_x >= height_x2 + 4
}

/// Java `Mth.map`.
fn map(value: f64, min: f64, max: f64, new_min: f64, new_max: f64) -> f64 {
    (value - min) / (max - min) * (new_max - new_min) + new_min
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// Approximation of `Biome.coldEnoughToSnow` for the frozen-ocean branch.
///
/// The overworld rule tree only asks for temperature inside the frozen-ocean
/// hole branch. Base temperatures come from the biome data pack; the frozen
/// temperature modifier is not modelled yet, so `frozen_ocean` is treated as
/// cold while `deep_frozen_ocean` (base 0.5) is treated as warm.
fn cold_enough_to_snow(biome: &str, y: i32, sea_level: i32) -> bool {
    let base_temperature = match biome {
        "minecraft:frozen_ocean" => 0.0,
        "minecraft:deep_frozen_ocean" => 0.5,
        _ => return false,
    };
    // `Biome.getHeightAdjustedTemperature` only cools above sea level + 17.
    // `Biome.TEMPERATURE_NOISE` is a `PerlinSimplexNoise` that is not ported,
    // so its term is omitted; frozen oceans sit near sea level where the
    // height adjustment does not apply anyway.
    let temperature = if y > sea_level + 17 {
        base_temperature - f64::from(y - (sea_level + 17)) * 0.05 / 40.0
    } else {
        base_temperature
    };
    (temperature as f32) < 0.15
}

/// Approximation of `Biome.shouldMeltFrozenOceanIcebergSlightly`: only the
/// deep frozen ocean is unconditionally above the 0.1 threshold.
fn frozen_ocean_melt(biome: &str) -> Option<bool> {
    match biome {
        "minecraft:frozen_ocean" => Some(false),
        "minecraft:deep_frozen_ocean" => Some(true),
        _ => None,
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
