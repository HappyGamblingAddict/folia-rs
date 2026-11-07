//! Overworld density-function evaluation.
//!
//! The vanilla `noise_settings/overworld.json` router, its referenced density
//! functions, and noise parameter files are embedded in
//! `assets/worldgen.json`. This module builds the same function graph and
//! evaluates it at single block positions, matching Java's `DensityFunction`
//! semantics.

use crate::worldgen::noise::{BlendedNoise, NormalNoise};
use folia_random::{XoroshiroPositionalRandomFactory, XoroshiroRandomSource};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const WORLDGEN_DATA: &str = include_str!("../../assets/worldgen.json");

pub struct WorldgenData {
    settings: Value,
    functions: HashMap<String, Value>,
    noises: HashMap<String, (i32, Vec<f64>)>,
    factory: XoroshiroPositionalRandomFactory,
    noise_instances: HashMap<String, Arc<NormalNoise>>,
}

impl WorldgenData {
    pub fn overworld(seed: i64) -> io::Result<Self> {
        let root: Value = serde_json::from_str(WORLDGEN_DATA)
            .map_err(|error| invalid(format!("invalid embedded worldgen data: {error}")))?;
        let settings = root
            .get("settings")
            .cloned()
            .ok_or_else(|| invalid("worldgen data is missing settings"))?;
        let functions = root
            .get("density_functions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("worldgen data is missing density functions"))?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let noises: HashMap<String, (i32, Vec<f64>)> = root
            .get("noises")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("worldgen data is missing noises"))?
            .iter()
            .map(|(key, value)| {
                let first_octave = value
                    .get("firstOctave")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32;
                let amplitudes = value
                    .get("amplitudes")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_f64().unwrap_or(0.0))
                            .collect()
                    })
                    .unwrap_or_default();
                (key.clone(), (first_octave, amplitudes))
            })
            .collect();

        let mut random = XoroshiroRandomSource::new(seed);
        let factory = random.fork_positional();
        let mut data = Self {
            settings,
            functions,
            noises,
            factory,
            noise_instances: HashMap::new(),
        };
        let names: Vec<String> = data.noises.keys().cloned().collect();
        for name in names {
            let (first_octave, amplitudes) = data.noises.get(&name).unwrap().clone();
            let noise = NormalNoise::from_factory(&factory, &name, first_octave, &amplitudes);
            data.noise_instances.insert(name, Arc::new(noise));
        }
        Ok(data)
    }

    pub fn seed(&self) -> &XoroshiroPositionalRandomFactory {
        &self.factory
    }

    pub fn sea_level(&self) -> i32 {
        self.settings
            .get("sea_level")
            .and_then(Value::as_i64)
            .unwrap_or(63) as i32
    }

    pub fn min_y(&self) -> i32 {
        self.settings
            .get("noise")
            .and_then(|noise| noise.get("min_y"))
            .and_then(Value::as_i64)
            .unwrap_or(-64) as i32
    }

    pub fn height(&self) -> i32 {
        self.settings
            .get("noise")
            .and_then(|noise| noise.get("height"))
            .and_then(Value::as_i64)
            .unwrap_or(384) as i32
    }

    pub fn noise(&self, name: &str) -> io::Result<Arc<NormalNoise>> {
        self.noise_instances
            .get(name)
            .cloned()
            .ok_or_else(|| invalid(format!("unknown noise {name}")))
    }

    /// All instantiated noise samplers, keyed by their full registry name.
    pub fn noises(&self) -> &HashMap<String, Arc<NormalNoise>> {
        &self.noise_instances
    }

    /// The serialized `surface_rule` tree for this noise setting.
    pub fn surface_rule(&self) -> io::Result<Value> {
        self.settings
            .get("surface_rule")
            .cloned()
            .ok_or_else(|| invalid("worldgen data is missing surface_rule"))
    }

    fn function_json(&self, name: &str) -> io::Result<Value> {
        self.functions
            .get(name)
            .cloned()
            .ok_or_else(|| invalid(format!("unknown density function {name}")))
    }

    fn build_named(
        &self,
        name: &str,
        cache: &mut HashMap<String, Arc<DensityFunction>>,
    ) -> io::Result<Arc<DensityFunction>> {
        if let Some(function) = cache.get(name) {
            return Ok(function.clone());
        }
        let value = self.function_json(name)?;
        let function = self.build_value(&value, cache)?;
        cache.insert(name.to_owned(), function.clone());
        Ok(function)
    }

    fn build_value(
        &self,
        value: &Value,
        cache: &mut HashMap<String, Arc<DensityFunction>>,
    ) -> io::Result<Arc<DensityFunction>> {
        if let Some(number) = value.as_f64() {
            return Ok(Arc::new(DensityFunction::Constant(number)));
        }
        if let Some(name) = value.as_str() {
            return self.build_named(name, cache);
        }
        let object = value
            .as_object()
            .ok_or_else(|| invalid("density function must be a number, string or object"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("density function is missing type"))?;

        Ok(match kind {
            "minecraft:constant" => Arc::new(DensityFunction::Constant(
                object.get("value").and_then(Value::as_f64).unwrap_or(0.0),
            )),
            "minecraft:noise" => {
                let noise = self.noise(noise_name(object)?)?;
                Arc::new(DensityFunction::Noise {
                    noise,
                    xz_scale: number(object, "xz_scale").unwrap_or(1.0),
                    y_scale: number(object, "y_scale").unwrap_or(1.0),
                })
            }
            "minecraft:shifted_noise" => Arc::new(DensityFunction::ShiftedNoise {
                shift_x: self.build_field(object, "shift_x", cache)?,
                shift_y: self.build_field(object, "shift_y", cache)?,
                shift_z: self.build_field(object, "shift_z", cache)?,
                xz_scale: number(object, "xz_scale").unwrap_or(1.0),
                y_scale: number(object, "y_scale").unwrap_or(1.0),
                noise: self.noise(noise_name(object)?)?,
            }),
            "minecraft:shift_x" | "minecraft:shift_a" => Arc::new(DensityFunction::ShiftX(
                self.noise(&argument_noise_name(object)?)?,
            )),
            "minecraft:shift_z" | "minecraft:shift_b" => Arc::new(DensityFunction::ShiftZ(
                self.noise(&argument_noise_name(object)?)?,
            )),
            "minecraft:shift" => Arc::new(DensityFunction::Shift(
                self.noise(&argument_noise_name(object)?)?,
            )),
            "minecraft:add" | "minecraft:mul" | "minecraft:min" | "minecraft:max" => {
                let first = self.build_field(object, "argument1", cache)?;
                let second = self.build_field(object, "argument2", cache)?;
                Arc::new(match kind {
                    "minecraft:add" => DensityFunction::Add(first, second),
                    "minecraft:mul" => DensityFunction::Mul(first, second),
                    "minecraft:min" => DensityFunction::Min(first, second),
                    _ => DensityFunction::Max(first, second),
                })
            }
            "minecraft:abs"
            | "minecraft:square"
            | "minecraft:cube"
            | "minecraft:half_negative"
            | "minecraft:quarter_negative"
            | "minecraft:invert"
            | "minecraft:squeeze" => {
                let input = self.build_field(object, "argument", cache)?;
                Arc::new(match kind {
                    "minecraft:abs" => DensityFunction::Abs(input),
                    "minecraft:square" => DensityFunction::Square(input),
                    "minecraft:cube" => DensityFunction::Cube(input),
                    "minecraft:half_negative" => DensityFunction::HalfNegative(input),
                    "minecraft:quarter_negative" => DensityFunction::QuarterNegative(input),
                    "minecraft:invert" => DensityFunction::Invert(input),
                    _ => DensityFunction::Squeeze(input),
                })
            }
            "minecraft:clamp" => Arc::new(DensityFunction::Clamp {
                input: self.build_field(object, "input", cache)?,
                min: number(object, "min").unwrap_or(0.0),
                max: number(object, "max").unwrap_or(0.0),
            }),
            "minecraft:y_clamped_gradient" => Arc::new(DensityFunction::YClampedGradient {
                from_y: object.get("from_y").and_then(Value::as_i64).unwrap_or(0) as i32,
                to_y: object.get("to_y").and_then(Value::as_i64).unwrap_or(0) as i32,
                from_value: number(object, "from_value").unwrap_or(0.0),
                to_value: number(object, "to_value").unwrap_or(0.0),
            }),
            "minecraft:range_choice" => Arc::new(DensityFunction::RangeChoice {
                input: self.build_field(object, "input", cache)?,
                min: number(object, "min_inclusive").unwrap_or(0.0),
                max: number(object, "max_exclusive").unwrap_or(0.0),
                when_in_range: self.build_field(object, "when_in_range", cache)?,
                when_out_of_range: self.build_field(object, "when_out_of_range", cache)?,
            }),
            "minecraft:flat_cache" | "minecraft:cache_2d" => Arc::new(DensityFunction::Cache2D {
                id: next_cache_id(),
                argument: self.build_field(object, "argument", cache)?,
            }),
            "minecraft:cache_once" => Arc::new(DensityFunction::CacheOnce {
                id: next_cache_id(),
                argument: self.build_field(object, "argument", cache)?,
            }),
            // Java's `NoiseChunk` wraps every `interpolated` marker in a
            // `NoiseInterpolator`, so the argument is only evaluated at the
            // 4x8x4 cell corners and trilinearly interpolated per block.
            "minecraft:interpolated" => Arc::new(DensityFunction::Interpolated {
                id: next_cache_id(),
                origin_y: self.min_y(),
                argument: self.build_field(object, "argument", cache)?,
            }),
            // `cache_all_in_cell` is a per-cell marker and `blend_density`
            // only wraps the blended function during blending passes.
            "minecraft:cache_all_in_cell" | "minecraft:blend_density" => Arc::new(
                DensityFunction::Marker(self.build_field(object, "argument", cache)?),
            ),
            "minecraft:blend_alpha" => Arc::new(DensityFunction::Constant(1.0)),
            "minecraft:blend_offset" => Arc::new(DensityFunction::Constant(0.0)),
            "minecraft:old_blended_noise" => {
                let blended = BlendedNoise::from_factory(
                    &self.factory,
                    "minecraft:terrain",
                    number(object, "xz_scale").unwrap_or(1.0),
                    number(object, "y_scale").unwrap_or(1.0),
                    number(object, "xz_factor").unwrap_or(1.0),
                    number(object, "y_factor").unwrap_or(1.0),
                    number(object, "smear_scale_multiplier").unwrap_or(1.0),
                );
                Arc::new(DensityFunction::OldBlendedNoise(Arc::new(blended)))
            }
            "minecraft:find_top_surface" => Arc::new(DensityFunction::FindTopSurface {
                density: self.build_field(object, "density", cache)?,
                upper_bound: self.build_field(object, "upper_bound", cache)?,
                lower_bound: object
                    .get("lower_bound")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32,
                cell_height: object
                    .get("cell_height")
                    .and_then(Value::as_i64)
                    .unwrap_or(1) as i32,
            }),
            "minecraft:spline" => {
                let spec = object
                    .get("spline")
                    .ok_or_else(|| invalid("spline is missing spec"))?;
                Arc::new(DensityFunction::Spline(self.build_spline(spec, cache)?))
            }
            "minecraft:weird_scaled_sampler" => Arc::new(DensityFunction::WeirdScaledSampler {
                input: self.build_field(object, "input", cache)?,
                noise: self.noise(noise_name(object)?)?,
                rarity: match object
                    .get("rarity_value_mapper")
                    .and_then(Value::as_str)
                    .unwrap_or("type_1")
                {
                    "type_2" => Rarity::Type2,
                    _ => Rarity::Type1,
                },
            }),
            other => {
                return Err(invalid(format!(
                    "unsupported density function type {other}"
                )));
            }
        })
    }

    fn build_field(
        &self,
        object: &serde_json::Map<String, Value>,
        field: &str,
        cache: &mut HashMap<String, Arc<DensityFunction>>,
    ) -> io::Result<Arc<DensityFunction>> {
        let value = object
            .get(field)
            .ok_or_else(|| invalid(format!("density function is missing {field}")))?;
        self.build_value(value, cache)
    }

    fn build_spline(
        &self,
        spec: &Value,
        cache: &mut HashMap<String, Arc<DensityFunction>>,
    ) -> io::Result<Spline> {
        let object = spec
            .as_object()
            .ok_or_else(|| invalid("spline spec must be an object"))?;
        let coordinate = self.build_field(object, "coordinate", cache)?;
        let points = object
            .get("points")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("spline is missing points"))?;
        let mut locations = Vec::with_capacity(points.len());
        let mut values = Vec::with_capacity(points.len());
        let mut derivatives = Vec::with_capacity(points.len());
        for point in points {
            let point = point
                .as_object()
                .ok_or_else(|| invalid("spline point must be an object"))?;
            locations.push(
                point
                    .get("location")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| invalid("spline point is missing location"))?
                    as f32,
            );
            let value = point
                .get("value")
                .ok_or_else(|| invalid("spline point is missing value"))?;
            values.push(if let Some(number) = value.as_f64() {
                SplineValue::Constant(number as f32)
            } else {
                SplineValue::Spline(Box::new(self.build_spline(value, cache)?))
            });
            derivatives.push(
                point
                    .get("derivative")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
            );
        }
        Ok(Spline {
            coordinate,
            locations,
            values,
            derivatives,
        })
    }

    /// Builds the overworld noise router for a seed.
    pub fn router(&self) -> io::Result<NoiseRouter> {
        let mut cache: HashMap<String, Arc<DensityFunction>> = HashMap::new();
        let mut functions = HashMap::new();
        let router = self
            .settings
            .get("noise_router")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("settings are missing noise_router"))?;
        for (name, value) in router {
            functions.insert(name.clone(), self.build_value(value, &mut cache)?);
        }
        Ok(NoiseRouter { functions })
    }
}

pub struct NoiseRouter {
    functions: HashMap<String, Arc<DensityFunction>>,
}

impl NoiseRouter {
    pub fn function(&self, name: &str) -> io::Result<&Arc<DensityFunction>> {
        self.functions
            .get(name)
            .ok_or_else(|| invalid(format!("unknown router function {name}")))
    }

    pub fn compute(&self, name: &str, x: i32, y: i32, z: i32) -> io::Result<f64> {
        Ok(self.function(name)?.compute(x, y, z))
    }

    /// Evaluates a router function the way `NoiseChunk` does: every
    /// `interpolated` marker is sampled on the 4x8x4 cell grid and trilinearly
    /// interpolated for the queried block position.
    pub fn compute_interpolated(&self, name: &str, x: i32, y: i32, z: i32) -> io::Result<f64> {
        Ok(self.function(name)?.compute_interpolated(x, y, z))
    }

    pub fn final_density(&self) -> &Arc<DensityFunction> {
        self.functions
            .get("final_density")
            .expect("router always has final_density")
    }
}

pub enum DensityFunction {
    Constant(f64),
    Noise {
        noise: Arc<NormalNoise>,
        xz_scale: f64,
        y_scale: f64,
    },
    ShiftedNoise {
        shift_x: Arc<DensityFunction>,
        shift_y: Arc<DensityFunction>,
        shift_z: Arc<DensityFunction>,
        xz_scale: f64,
        y_scale: f64,
        noise: Arc<NormalNoise>,
    },
    ShiftX(Arc<NormalNoise>),
    ShiftZ(Arc<NormalNoise>),
    Shift(Arc<NormalNoise>),
    Add(Arc<DensityFunction>, Arc<DensityFunction>),
    Mul(Arc<DensityFunction>, Arc<DensityFunction>),
    Min(Arc<DensityFunction>, Arc<DensityFunction>),
    Max(Arc<DensityFunction>, Arc<DensityFunction>),
    Abs(Arc<DensityFunction>),
    Square(Arc<DensityFunction>),
    Cube(Arc<DensityFunction>),
    HalfNegative(Arc<DensityFunction>),
    QuarterNegative(Arc<DensityFunction>),
    Invert(Arc<DensityFunction>),
    Squeeze(Arc<DensityFunction>),
    Clamp {
        input: Arc<DensityFunction>,
        min: f64,
        max: f64,
    },
    YClampedGradient {
        from_y: i32,
        to_y: i32,
        from_value: f64,
        to_value: f64,
    },
    RangeChoice {
        input: Arc<DensityFunction>,
        min: f64,
        max: f64,
        when_in_range: Arc<DensityFunction>,
        when_out_of_range: Arc<DensityFunction>,
    },
    Marker(Arc<DensityFunction>),
    /// `DensityFunctions.Marker.Type.Interpolated`: `NoiseChunk` evaluates the
    /// argument on the 4x8x4 cell grid and trilinearly interpolates between
    /// the corners. Direct `compute` remains the unwrapped evaluation, matching
    /// Java's `DensityFunction.compute` outside `NoiseChunk`.
    Interpolated {
        id: u64,
        origin_y: i32,
        argument: Arc<DensityFunction>,
    },
    /// `DensityFunctions.Cache2D` / `FlatCache`: the wrapped function depends
    /// only on x/z, so its last value is reused while the column does not move.
    Cache2D {
        id: u64,
        argument: Arc<DensityFunction>,
    },
    /// `DensityFunctions.CacheOnce`: caches one exact block position.
    CacheOnce {
        id: u64,
        argument: Arc<DensityFunction>,
    },
    OldBlendedNoise(Arc<BlendedNoise>),
    FindTopSurface {
        density: Arc<DensityFunction>,
        upper_bound: Arc<DensityFunction>,
        lower_bound: i32,
        cell_height: i32,
    },
    Spline(Spline),
    WeirdScaledSampler {
        input: Arc<DensityFunction>,
        noise: Arc<NormalNoise>,
        rarity: Rarity,
    },
}

impl DensityFunction {
    /// Direct block-position evaluation with `interpolated` markers treated as
    /// pass-through wrappers. This matches Java's `DensityFunction.compute`
    /// outside a `NoiseChunk` (the grid oracle samples corners, where direct and
    /// interpolated values are equal).
    pub fn compute(&self, x: i32, y: i32, z: i32) -> f64 {
        self.compute_with(x, y, z, false)
    }

    /// `NoiseChunk` evaluation: every `interpolated` marker is sampled on the
    /// 4x8x4 cell grid and trilinearly interpolated at the block position.
    pub fn compute_interpolated(&self, x: i32, y: i32, z: i32) -> f64 {
        self.compute_with(x, y, z, true)
    }

    fn compute_with(&self, x: i32, y: i32, z: i32, interpolate: bool) -> f64 {
        match self {
            DensityFunction::Constant(value) => *value,
            DensityFunction::Noise {
                noise,
                xz_scale,
                y_scale,
            } => noise.get_value(
                f64::from(x) * xz_scale,
                f64::from(y) * y_scale,
                f64::from(z) * xz_scale,
            ),
            DensityFunction::ShiftedNoise {
                shift_x,
                shift_y,
                shift_z,
                xz_scale,
                y_scale,
                noise,
            } => noise.get_value(
                f64::from(x) * xz_scale + shift_x.compute_with(x, y, z, interpolate),
                f64::from(y) * y_scale + shift_y.compute_with(x, y, z, interpolate),
                f64::from(z) * xz_scale + shift_z.compute_with(x, y, z, interpolate),
            ),
            DensityFunction::ShiftX(noise) => shift_value(noise, f64::from(x), 0.0, f64::from(z)),
            DensityFunction::ShiftZ(noise) => shift_value(noise, f64::from(z), f64::from(x), 0.0),
            DensityFunction::Shift(noise) => {
                shift_value(noise, f64::from(x), f64::from(y), f64::from(z))
            }
            DensityFunction::Add(first, second) => {
                first.compute_with(x, y, z, interpolate) + second.compute_with(x, y, z, interpolate)
            }
            DensityFunction::Mul(first, second) => {
                first.compute_with(x, y, z, interpolate) * second.compute_with(x, y, z, interpolate)
            }
            DensityFunction::Min(first, second) => first
                .compute_with(x, y, z, interpolate)
                .min(second.compute_with(x, y, z, interpolate)),
            DensityFunction::Max(first, second) => first
                .compute_with(x, y, z, interpolate)
                .max(second.compute_with(x, y, z, interpolate)),
            DensityFunction::Abs(input) => input.compute_with(x, y, z, interpolate).abs(),
            DensityFunction::Square(input) => {
                let value = input.compute_with(x, y, z, interpolate);
                value * value
            }
            DensityFunction::Cube(input) => {
                let value = input.compute_with(x, y, z, interpolate);
                value * value * value
            }
            DensityFunction::HalfNegative(input) => {
                let value = input.compute_with(x, y, z, interpolate);
                if value > 0.0 { value } else { value * 0.5 }
            }
            DensityFunction::QuarterNegative(input) => {
                let value = input.compute_with(x, y, z, interpolate);
                if value > 0.0 { value } else { value * 0.25 }
            }
            DensityFunction::Invert(input) => 1.0 / input.compute_with(x, y, z, interpolate),
            DensityFunction::Squeeze(input) => {
                let value = input.compute_with(x, y, z, interpolate).clamp(-1.0, 1.0);
                value / 2.0 - value * value * value / 24.0
            }
            DensityFunction::Clamp { input, min, max } => {
                input.compute_with(x, y, z, interpolate).clamp(*min, *max)
            }
            DensityFunction::YClampedGradient {
                from_y,
                to_y,
                from_value,
                to_value,
            } => clamped_map(
                f64::from(y),
                f64::from(*from_y),
                f64::from(*to_y),
                *from_value,
                *to_value,
            ),
            DensityFunction::RangeChoice {
                input,
                min,
                max,
                when_in_range,
                when_out_of_range,
            } => {
                let value = input.compute_with(x, y, z, interpolate);
                if value >= *min && value < *max {
                    when_in_range.compute_with(x, y, z, interpolate)
                } else {
                    when_out_of_range.compute_with(x, y, z, interpolate)
                }
            }
            DensityFunction::Marker(input) => input.compute_with(x, y, z, interpolate),
            DensityFunction::Interpolated {
                id,
                origin_y,
                argument,
            } => {
                if interpolate {
                    interpolated_value(*id, *origin_y, argument, x, y, z)
                } else {
                    argument.compute_with(x, y, z, false)
                }
            }
            DensityFunction::Cache2D { id, argument } => {
                if let Some(value) = cache_2d_get(*id, x, z) {
                    return value;
                }
                let value = argument.compute_with(x, y, z, interpolate);
                cache_2d_set(*id, x, z, value);
                value
            }
            DensityFunction::CacheOnce { id, argument } => {
                if let Some(value) = cache_once_get(*id, x, y, z) {
                    return value;
                }
                let value = argument.compute_with(x, y, z, interpolate);
                cache_once_set(*id, x, y, z, value);
                value
            }
            DensityFunction::OldBlendedNoise(noise) => {
                noise.compute(f64::from(x), f64::from(y), f64::from(z))
            }
            DensityFunction::FindTopSurface {
                density,
                upper_bound,
                lower_bound,
                cell_height,
            } => {
                let upper = (upper_bound.compute_with(x, y, z, interpolate)
                    / f64::from(*cell_height))
                .floor() as i32
                    * *cell_height;
                if upper <= *lower_bound {
                    return f64::from(*lower_bound);
                }
                let mut level = upper;
                while level >= *lower_bound {
                    if density.compute_with(x, level, z, interpolate) > 0.0 {
                        return f64::from(level);
                    }
                    level -= *cell_height;
                }
                f64::from(*lower_bound)
            }
            DensityFunction::Spline(spline) => f64::from(spline.apply_with(x, y, z, interpolate)),
            DensityFunction::WeirdScaledSampler {
                input,
                noise,
                rarity,
            } => {
                let value = input.compute_with(x, y, z, interpolate);
                let rarity = rarity.map(value);
                rarity
                    * noise
                        .get_value(
                            f64::from(x) / rarity,
                            f64::from(y) / rarity,
                            f64::from(z) / rarity,
                        )
                        .abs()
            }
        }
    }
}

/// `NoiseChunk` cell width/height for the overworld noise settings
/// (`size_horizontal: 1`, `size_vertical: 2`, each multiplied by four).
const INTERPOLATION_CELL_WIDTH: i32 = 4;
const INTERPOLATION_CELL_HEIGHT: i32 = 8;

#[derive(Clone, Copy)]
struct InterpolatedCell {
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    values: [f64; 8],
}

thread_local! {
    static INTERPOLATED_CELLS: RefCell<HashMap<u64, InterpolatedCell>> =
        RefCell::new(HashMap::new());
}

fn interpolated_value(
    id: u64,
    min_y: i32,
    argument: &DensityFunction,
    x: i32,
    y: i32,
    z: i32,
) -> f64 {
    let origin_x = x.div_euclid(INTERPOLATION_CELL_WIDTH) * INTERPOLATION_CELL_WIDTH;
    let origin_y =
        min_y + (y - min_y).div_euclid(INTERPOLATION_CELL_HEIGHT) * INTERPOLATION_CELL_HEIGHT;
    let origin_z = z.div_euclid(INTERPOLATION_CELL_WIDTH) * INTERPOLATION_CELL_WIDTH;

    let cached = INTERPOLATED_CELLS.with(|cache| {
        cache
            .borrow()
            .get(&id)
            .filter(|cell| {
                cell.origin_x == origin_x && cell.origin_y == origin_y && cell.origin_z == origin_z
            })
            .map(|cell| cell.values)
    });
    let values = cached.unwrap_or_else(|| {
        let x1 = origin_x + INTERPOLATION_CELL_WIDTH;
        let y1 = origin_y + INTERPOLATION_CELL_HEIGHT;
        let z1 = origin_z + INTERPOLATION_CELL_WIDTH;
        let values = [
            argument.compute_with(origin_x, origin_y, origin_z, true),
            argument.compute_with(x1, origin_y, origin_z, true),
            argument.compute_with(origin_x, y1, origin_z, true),
            argument.compute_with(x1, y1, origin_z, true),
            argument.compute_with(origin_x, origin_y, z1, true),
            argument.compute_with(x1, origin_y, z1, true),
            argument.compute_with(origin_x, y1, z1, true),
            argument.compute_with(x1, y1, z1, true),
        ];
        INTERPOLATED_CELLS.with(|cache| {
            cache.borrow_mut().insert(
                id,
                InterpolatedCell {
                    origin_x,
                    origin_y,
                    origin_z,
                    values,
                },
            );
        });
        values
    });

    let delta_x = f64::from(x - origin_x) / f64::from(INTERPOLATION_CELL_WIDTH);
    let delta_y = f64::from(y - origin_y) / f64::from(INTERPOLATION_CELL_HEIGHT);
    let delta_z = f64::from(z - origin_z) / f64::from(INTERPOLATION_CELL_WIDTH);
    // `Mth.lerp3` argument order: x0y0z0, x1y0z0, x0y1z0, x1y1z0,
    // x0y0z1, x1y0z1, x0y1z1, x1y1z1.
    lerp(
        delta_z,
        lerp2(delta_x, delta_y, values[0], values[1], values[2], values[3]),
        lerp2(delta_x, delta_y, values[4], values[5], values[6], values[7]),
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

static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);

fn next_cache_id() -> u64 {
    NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
    static CACHE_2D: RefCell<HashMap<u64, (i32, i32, f64)>> = RefCell::new(HashMap::new());
    static CACHE_ONCE: RefCell<HashMap<u64, (i32, i32, i32, f64)>> = RefCell::new(HashMap::new());
}

fn cache_2d_get(id: u64, x: i32, z: i32) -> Option<f64> {
    CACHE_2D.with(|cache| {
        cache
            .borrow()
            .get(&id)
            .filter(|(cached_x, cached_z, _)| *cached_x == x && *cached_z == z)
            .map(|(_, _, value)| *value)
    })
}

fn cache_2d_set(id: u64, x: i32, z: i32, value: f64) {
    CACHE_2D.with(|cache| {
        cache.borrow_mut().insert(id, (x, z, value));
    });
}

fn cache_once_get(id: u64, x: i32, y: i32, z: i32) -> Option<f64> {
    CACHE_ONCE.with(|cache| {
        cache
            .borrow()
            .get(&id)
            .filter(|(cached_x, cached_y, cached_z, _)| {
                *cached_x == x && *cached_y == y && *cached_z == z
            })
            .map(|(_, _, _, value)| *value)
    })
}

fn cache_once_set(id: u64, x: i32, y: i32, z: i32, value: f64) {
    CACHE_ONCE.with(|cache| {
        cache.borrow_mut().insert(id, (x, y, z, value));
    });
}

fn shift_value(noise: &NormalNoise, x: f64, y: f64, z: f64) -> f64 {
    noise.get_value(x * 0.25, y * 0.25, z * 0.25) * 4.0
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

pub enum Rarity {
    Type1,
    Type2,
}

impl Rarity {
    fn map(&self, value: f64) -> f64 {
        match self {
            Rarity::Type1 => {
                if value < -0.5 {
                    0.75
                } else if value < 0.0 {
                    1.0
                } else if value < 0.5 {
                    1.5
                } else {
                    2.0
                }
            }
            Rarity::Type2 => {
                if value < -0.75 {
                    0.5
                } else if value < -0.5 {
                    0.75
                } else if value < 0.5 {
                    1.0
                } else if value < 0.75 {
                    2.0
                } else {
                    3.0
                }
            }
        }
    }
}

pub struct Spline {
    coordinate: Arc<DensityFunction>,
    locations: Vec<f32>,
    values: Vec<SplineValue>,
    derivatives: Vec<f32>,
}

pub enum SplineValue {
    Constant(f32),
    Spline(Box<Spline>),
}

impl SplineValue {
    fn apply_with(&self, x: i32, y: i32, z: i32, interpolate: bool) -> f32 {
        match self {
            SplineValue::Constant(value) => *value,
            SplineValue::Spline(spline) => spline.apply_with(x, y, z, interpolate),
        }
    }
}

impl Spline {
    fn apply_with(&self, x: i32, y: i32, z: i32, interpolate: bool) -> f32 {
        let location = self.coordinate.compute_with(x, y, z, interpolate) as f32;
        let start = find_interval_start(&self.locations, location);
        let last = self.locations.len() - 1;
        if start < 0 {
            return linear_extend(
                location,
                &self.locations,
                self.values[0].apply_with(x, y, z, interpolate),
                &self.derivatives,
                0,
            );
        }
        let start = start as usize;
        if start == last {
            return linear_extend(
                location,
                &self.locations,
                self.values[last].apply_with(x, y, z, interpolate),
                &self.derivatives,
                last,
            );
        }

        let first_location = self.locations[start];
        let second_location = self.locations[start + 1];
        let delta = (location - first_location) / (second_location - first_location);
        let first_value = self.values[start].apply_with(x, y, z, interpolate);
        let second_value = self.values[start + 1].apply_with(x, y, z, interpolate);
        let first_derivative = self.derivatives[start];
        let second_derivative = self.derivatives[start + 1];
        let d8 =
            first_derivative * (second_location - first_location) - (second_value - first_value);
        let d9 =
            -second_derivative * (second_location - first_location) + (second_value - first_value);
        lerp_f32(delta, first_value, second_value) + delta * (1.0 - delta) * lerp_f32(delta, d8, d9)
    }
}

fn find_interval_start(locations: &[f32], start: f32) -> i32 {
    // Mth.binarySearch(0, len, i -> start < locations[i]) - 1
    let mut low = 0usize;
    let mut high = locations.len();
    while low < high {
        let mid = low + (high - low) / 2;
        if start < locations[mid] {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low as i32 - 1
}

fn linear_extend(
    coordinate: f32,
    locations: &[f32],
    value: f32,
    derivatives: &[f32],
    index: usize,
) -> f32 {
    let derivative = derivatives[index];
    if derivative == 0.0 {
        value
    } else {
        value + derivative * (coordinate - locations[index])
    }
}

fn lerp_f32(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

fn noise_name(object: &serde_json::Map<String, Value>) -> io::Result<&str> {
    if let Some(name) = object.get("noise").and_then(Value::as_str) {
        return Ok(name);
    }
    if let Some(holder) = object.get("noise").and_then(Value::as_object)
        && let Some(name) = holder.get("noise").and_then(Value::as_str)
    {
        return Ok(name);
    }
    Err(invalid("density function is missing noise"))
}

fn argument_noise_name(object: &serde_json::Map<String, Value>) -> io::Result<String> {
    if let Some(name) = object.get("argument").and_then(Value::as_str) {
        return Ok(name.to_owned());
    }
    if let Some(holder) = object.get("argument").and_then(Value::as_object)
        && let Some(name) = holder.get("noise").and_then(Value::as_str)
    {
        return Ok(name.to_owned());
    }
    Err(invalid("shift density function is missing noise"))
}

fn number(object: &serde_json::Map<String, Value>, field: &str) -> Option<f64> {
    object.get(field).and_then(Value::as_f64)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> NoiseRouter {
        WorldgenData::overworld(12345).unwrap().router().unwrap()
    }

    #[test]
    fn continents_router_matches_java() {
        let router = router();
        let cases = [
            ((0, 0, 0), -0.399_103_899_103_672_44),
            ((123, 64, -456), 0.262_597_044_484_264_85),
            ((-1000, 80, 2000), 0.192_619_812_023_379_88),
            ((15, -16, 31), -0.399_799_905_048_133_9),
            ((16, 0, -16), -0.370_611_352_562_558_7),
        ];
        for ((x, y, z), expected) in cases {
            let actual = router.compute("continents", x, y, z).unwrap();
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "continents ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }

    #[test]
    fn temperature_vegetation_erosion_ridges_match_java() {
        let router = router();
        let cases: [(&str, (i32, i32, i32), f64); 6] = [
            ("temperature", (0, 0, 0), -0.021_591_955_520_305_697),
            ("vegetation", (0, 0, 0), 0.672_137_932_891_602_8),
            ("erosion", (0, 0, 0), -0.729_403_288_756_411_6),
            ("ridges", (0, 0, 0), 0.156_911_749_394_763_77),
            ("erosion", (123, 64, -456), -0.473_755_345_958_190_85),
            ("ridges", (16, 0, -16), 0.331_609_791_480_752_04),
        ];
        for (name, (x, y, z), expected) in cases {
            let actual = router.compute(name, x, y, z).unwrap();
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "{name} ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }

    #[test]
    fn final_density_matches_java() {
        let router = router();
        let cases = [
            ((0, 0, 0), -0.023_074_310_796_174_443),
            ((0, -64, 0), 0.037_482_421_875),
            ((0, -60, 0), 0.036_986_244_779_772_25),
            ((0, -30, 0), 0.051_113_816_735_200_907),
            ((0, 63, 0), -0.178_946_153_443_645_28),
            ((0, 120, 0), -0.458_333_333_333_333_3),
            ((123, 64, -456), -0.012_288_201_688_792_234),
            ((-1000, 80, 2000), -0.297_057_924_827_922_8),
            ((15, -16, 31), 0.129_759_656_434_129_84),
            ((16, 0, -16), 0.001_536_797_943_627_938_5),
        ];
        for ((x, y, z), expected) in cases {
            let actual = router.compute("final_density", x, y, z).unwrap();
            assert!(
                (actual - expected).abs() < 1.0e-9,
                "final_density ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }
}
