//! Terrain, fluid, ice, and geode features.
//!
//! Ports the procedural Overworld/Nether feature types that do not need
//! structures or block entities: disks, springs, lakes, icebergs, ice spikes,
//! blue ice, netherrack blobs, basalt columns/pillars, deltas, and amethyst
//! geodes.
//!
//! The implementations follow the decompiled 1.21.11 classes in RNG draw
//! order. `IcebergFeature` and `BasaltColumnsFeature` read the generator sea
//! level; [`FeatureLevel::sea_level`] defaults to the Overworld value because
//! the region has no generator handle and every shipped config using it is
//! Overworld or Nether lava-level compatible.

use super::placement::IntProvider;
use super::support::{
    StateProvider, int_field, is_air, is_solid, optional_bool, parse_block_predicate,
    parse_int_provider_supported, parse_state, state, test_predicate,
};
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::tags;
use crate::worldgen::noise::NormalNoise;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

fn object<'a>(config: &'a Value, kind: &str) -> io::Result<&'a Map<String, Value>> {
    config
        .as_object()
        .ok_or_else(|| super::invalid(format!("{kind} config must be an object")))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str, kind: &str) -> io::Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| super::invalid(format!("{kind} is missing `{key}`")))
}

fn block_name(state: &BlockState) -> &str {
    &state.name
}

#[derive(Debug, Clone)]
pub struct DiskFeature {
    half_height: i32,
    radius: IntProvider,
    state_provider: StateProvider,
    target: super::placement::BlockPredicate,
}

impl DiskFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let y = origin.y();
        let max_y = y + self.half_height;
        let min_y = y - self.half_height - 1;
        let radius = self.radius.sample(random).expect("int provider is ported");
        let mut placed = false;
        for x in origin.x() - radius..=origin.x() + radius {
            for z in origin.z() - radius..=origin.z() + radius {
                let dx = x - origin.x();
                let dz = z - origin.z();
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                for current_y in (min_y + 1..=max_y).rev() {
                    let pos = BlockPos::new(x, current_y, z);
                    if test_predicate(&self.target, level, pos)
                        && let Some(block) = self.state_provider.sample(level, random, pos)
                    {
                        level.set_block(pos, block);
                        placed = true;
                    }
                }
            }
        }
        placed
    }
}

pub(crate) fn parse_disk_config(config: &Value) -> io::Result<Option<DiskFeature>> {
    let object = object(config, "disk")?;
    let half_height = int_field(object, "half_height")?;
    if half_height < 0 {
        return Err(super::invalid("disk half_height must not be negative"));
    }
    let radius = parse_int_provider_supported(required(object, "radius", "disk")?)?;
    let state_provider = StateProvider::parse(required(object, "state_provider", "disk")?)?;
    if !state_provider.is_supported() {
        return Ok(None);
    }
    let Some(target) = parse_block_predicate(required(object, "target", "disk")?)? else {
        return Ok(None);
    };
    Ok(Some(DiskFeature {
        half_height,
        radius,
        state_provider,
        target,
    }))
}

#[derive(Debug, Clone)]
pub struct SpringFeature {
    hole_count: i32,
    requires_block_below: bool,
    rock_count: i32,
    state: BlockState,
    valid_blocks: Vec<String>,
}

impl SpringFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let _ = random;
        if !self.is_valid(level, origin.above()) {
            return false;
        }
        if self.requires_block_below && !self.is_valid(level, origin.below()) {
            return false;
        }
        let current = level
            .get_block(origin)
            .cloned()
            .unwrap_or_else(|| state("minecraft:air"));
        if !is_air_name(&current.name) && !self.is_valid(level, origin) {
            return false;
        }
        let mut rocks = 0;
        if self.is_valid(level, origin.west()) {
            rocks += 1;
        }
        if self.is_valid(level, origin.east()) {
            rocks += 1;
        }
        if self.is_valid(level, origin.north()) {
            rocks += 1;
        }
        if self.is_valid(level, origin.south()) {
            rocks += 1;
        }
        if self.is_valid(level, origin.below()) {
            rocks += 1;
        }
        let mut holes = 0;
        if is_air(level, origin.west()) {
            holes += 1;
        }
        if is_air(level, origin.east()) {
            holes += 1;
        }
        if is_air(level, origin.north()) {
            holes += 1;
        }
        if is_air(level, origin.south()) {
            holes += 1;
        }
        if is_air(level, origin.below()) {
            holes += 1;
        }
        if rocks == self.rock_count && holes == self.hole_count {
            level.set_block(origin, self.state.clone());
            level.schedule_tick(origin, &self.state.name, 0);
            return true;
        }
        false
    }

    fn is_valid(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        level
            .get_block(pos)
            .is_some_and(|state| self.valid_blocks.iter().any(|name| name == &state.name))
    }
}

pub(crate) fn parse_spring_config(config: &Value) -> io::Result<Option<SpringFeature>> {
    let object = object(config, "spring")?;
    let state = parse_state(required(object, "state", "spring")?)?;
    let valid_blocks = match required(object, "valid_blocks", "spring")? {
        Value::String(name) => vec![super::resource_location(name)],
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(super::resource_location)
                    .ok_or_else(|| super::invalid("spring valid block must be a string"))
            })
            .collect::<io::Result<Vec<_>>>()?,
        _ => {
            return Err(super::invalid(
                "spring `valid_blocks` must be a block id or a list",
            ));
        }
    };
    Ok(Some(SpringFeature {
        hole_count: int_field(object, "hole_count")?,
        requires_block_below: optional_bool(object, "requires_block_below", false),
        rock_count: int_field(object, "rock_count")?,
        state,
        valid_blocks,
    }))
}

#[derive(Debug, Clone)]
pub struct BlueIceFeature;

impl BlueIceFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if origin.y() > level.sea_level() - 1 {
            return false;
        }
        let at_origin = level.get_block(origin).map(block_name);
        let below = level.get_block(origin.below()).map(block_name);
        if at_origin != Some("minecraft:water") && below != Some("minecraft:water") {
            return false;
        }
        let mut adjacent = false;
        for direction in Direction::ALL {
            if direction != Direction::Down
                && level.get_block(origin.relative(direction)).map(block_name)
                    == Some("minecraft:packed_ice")
            {
                adjacent = true;
                break;
            }
        }
        if !adjacent {
            return false;
        }

        level.set_block(origin, state("minecraft:blue_ice"));
        for _ in 0..200 {
            let dy = random.next_int_bound(5) - random.next_int_bound(6);
            let mut radius = 3;
            if dy < 2 {
                radius += dy / 2;
            }
            if radius < 1 {
                continue;
            }
            let dx = random.next_int_bound(radius) - random.next_int_bound(radius);
            let dz = random.next_int_bound(radius) - random.next_int_bound(radius);
            let pos = origin.offset(dx, dy, dz);
            let current = level.get_block(pos).map(block_name);
            if matches!(
                current,
                Some("minecraft:air")
                    | Some("minecraft:water")
                    | Some("minecraft:packed_ice")
                    | Some("minecraft:ice")
            ) {
                for direction in Direction::ALL {
                    if level.get_block(pos.relative(direction)).map(block_name)
                        == Some("minecraft:blue_ice")
                    {
                        level.set_block(pos, state("minecraft:blue_ice"));
                        break;
                    }
                }
            }
        }
        true
    }
}

pub(crate) fn parse_blue_ice_config() -> Option<BlueIceFeature> {
    Some(BlueIceFeature)
}

#[derive(Debug, Clone)]
pub struct IcebergFeature {
    state: BlockState,
}

impl IcebergFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let origin = BlockPos::new(origin.x(), level.sea_level(), origin.z());
        let place_snow = random.next_double() > 0.7;
        let angle = random.next_double() * 2.0 * std::f64::consts::PI;
        let ellipse_radius = 11 - random.next_int_bound(5);
        let snow_radius = 3 + random.next_int_bound(3);
        let elliptical = random.next_double() > 0.7;
        let mut major = if elliptical {
            random.next_int_bound(6) + 6
        } else {
            random.next_int_bound(15) + 3
        };
        if !elliptical && random.next_double() > 0.9 {
            major += random.next_int_bound(19) + 7;
        }
        let cut_height = (major + random.next_int_bound(11)).min(18);
        let minor = (major + random.next_int_bound(7) - random.next_int_bound(5)).min(11);
        let horizontal = if elliptical { ellipse_radius } else { 11 };

        for x in -horizontal..horizontal {
            for z in -horizontal..horizontal {
                for y in 0..major {
                    let radius = if elliptical {
                        height_dependent_radius_ellipse(y, major, minor)
                    } else {
                        height_dependent_radius_round(random, y, major, minor)
                    };
                    if elliptical || x < radius {
                        self.generate_iceberg_block(
                            level,
                            random,
                            origin,
                            major,
                            x,
                            y,
                            z,
                            radius,
                            horizontal,
                            elliptical,
                            snow_radius,
                            angle,
                            place_snow,
                        );
                    }
                }
            }
        }

        self.smooth(level, origin, minor, major, elliptical, ellipse_radius);

        for x in -horizontal..horizontal {
            for z in -horizontal..horizontal {
                let mut y = -1;
                while y > -cut_height {
                    let radius = if elliptical {
                        ((horizontal as f64 * (1.0 - (y * y) as f64 / (cut_height as f64 * 8.0)))
                            .ceil()) as i32
                    } else {
                        horizontal
                    };
                    let steep = height_dependent_radius_steep(random, -y, cut_height, minor);
                    if x < steep {
                        self.generate_iceberg_block(
                            level,
                            random,
                            origin,
                            cut_height,
                            x,
                            y,
                            z,
                            steep,
                            radius,
                            elliptical,
                            snow_radius,
                            angle,
                            place_snow,
                        );
                    }
                    y -= 1;
                }
            }
        }

        let cut_out = if elliptical {
            random.next_double() > 0.1
        } else {
            random.next_double() > 0.7
        };
        if cut_out {
            self.generate_cut_out(
                random,
                level,
                minor,
                major,
                origin,
                elliptical,
                ellipse_radius,
                angle,
                snow_radius,
            );
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_iceberg_block(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        height: i32,
        x: i32,
        y: i32,
        z: i32,
        radius: i32,
        major_radius: i32,
        elliptical: bool,
        minor_radius: i32,
        angle: f64,
        place_snow: bool,
    ) {
        let distance = if elliptical {
            signed_distance_ellipse(
                x,
                z,
                0,
                0,
                major_radius,
                ellipse_c(y, height, minor_radius),
                angle,
            )
        } else {
            signed_distance_circle(random, x, z, radius)
        };
        if distance >= 0.0 {
            return;
        }
        let edge = if elliptical {
            -0.5
        } else {
            -6.0 - f64::from(random.next_int_bound(3))
        };
        if distance > edge && random.next_double() > 0.9 {
            return;
        }
        self.set_iceberg_block(
            level,
            random,
            origin.offset(x, y, z),
            height - y,
            height,
            elliptical,
            place_snow,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn set_iceberg_block(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        height_remaining: i32,
        height: i32,
        elliptical: bool,
        place_snow: bool,
    ) {
        let Some(current) = level.get_block(pos).cloned() else {
            return;
        };
        if !matches!(
            current.name.as_str(),
            "minecraft:air" | "minecraft:snow_block" | "minecraft:ice" | "minecraft:water"
        ) {
            return;
        }
        let place = !elliptical || random.next_double() > 0.05;
        let divisor = if elliptical { 3 } else { 2 };
        let snow = place_snow && current.name != "minecraft:water" && {
            let threshold = f64::from(random.next_int_bound((height / divisor).max(1)))
                + f64::from(height) * 0.6;
            f64::from(height_remaining) <= threshold && place
        };
        if snow {
            level.set_block(pos, state("minecraft:snow_block"));
        } else {
            level.set_block(pos, self.state.clone());
        }
    }

    fn smooth(
        &self,
        level: &mut dyn FeatureLevel,
        origin: BlockPos,
        major_radius: i32,
        height: i32,
        elliptical: bool,
        minor_radius: i32,
    ) {
        let radius = if elliptical {
            minor_radius
        } else {
            major_radius / 2
        };
        for x in -radius..=radius {
            for z in -radius..=radius {
                for y in 0..=height {
                    let pos = origin.offset(x, y, z);
                    let Some(current) = level.get_block(pos).cloned() else {
                        continue;
                    };
                    if !is_iceberg_state(&current.name) && current.name != "minecraft:snow" {
                        continue;
                    }
                    let below = level
                        .get_block(pos.below())
                        .is_none_or(|state| is_air_name(&state.name));
                    if below {
                        level.set_block(pos, state("minecraft:air"));
                        level.set_block(pos.above(), state("minecraft:air"));
                    } else if is_iceberg_state(&current.name) {
                        let mut exposed = 0;
                        for neighbour in [pos.west(), pos.east(), pos.north(), pos.south()] {
                            if !level
                                .get_block(neighbour)
                                .is_some_and(|state| is_iceberg_state(&state.name))
                            {
                                exposed += 1;
                            }
                        }
                        if exposed >= 3 {
                            level.set_block(pos, state("minecraft:air"));
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_cut_out(
        &self,
        random: &mut dyn RandomSource,
        level: &mut dyn FeatureLevel,
        major_axis: i32,
        height: i32,
        origin: BlockPos,
        elliptical: bool,
        ellipse_radius: i32,
        angle: f64,
        minor_axis: i32,
    ) {
        let x_sign = if random.next_boolean() { -1 } else { 1 };
        let z_sign = if random.next_boolean() { -1 } else { 1 };
        let mut x = random.next_int_bound((major_axis / 2 - 2).max(1));
        if random.next_boolean() {
            x = major_axis / 2 + 1
                - random.next_int_bound((major_axis - major_axis / 2 - 1).max(1));
        }
        let mut z = random.next_int_bound((major_axis / 2 - 2).max(1));
        if random.next_boolean() {
            z = major_axis / 2 + 1
                - random.next_int_bound((major_axis - major_axis / 2 - 1).max(1));
        }
        if elliptical {
            x = random.next_int_bound((ellipse_radius - 5).max(1));
            z = x;
        }
        let ellipse_origin = (x_sign * x, z_sign * z);
        let perpendicular_angle = if elliptical {
            angle + std::f64::consts::FRAC_PI_2
        } else {
            random.next_double() * 2.0 * std::f64::consts::PI
        };

        for y in 0..height - 3 {
            let radius = height_dependent_radius_round(random, y, height, major_axis);
            self.carve(
                radius,
                y,
                origin,
                level,
                false,
                perpendicular_angle,
                ellipse_origin,
                ellipse_radius,
                minor_axis,
            );
        }
        let mut y = -1;
        while y > -height + random.next_int_bound(5) {
            let radius = height_dependent_radius_steep(random, -y, height, major_axis);
            self.carve(
                radius,
                y,
                origin,
                level,
                true,
                perpendicular_angle,
                ellipse_origin,
                ellipse_radius,
                minor_axis,
            );
            y -= 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn carve(
        &self,
        radius: i32,
        local_y: i32,
        origin: BlockPos,
        level: &mut dyn FeatureLevel,
        place_water: bool,
        perpendicular_angle: f64,
        ellipse_origin: (i32, i32),
        major_radius: i32,
        minor_radius: i32,
    ) {
        let horizontal = radius + 1 + major_radius / 3;
        let other = (radius - 3).min(3) + minor_radius / 2 - 1;
        for x in -horizontal..horizontal {
            for z in -horizontal..horizontal {
                let distance = signed_distance_ellipse(
                    x,
                    z,
                    ellipse_origin.0,
                    ellipse_origin.1,
                    horizontal,
                    other,
                    perpendicular_angle,
                );
                if distance >= 0.0 {
                    continue;
                }
                let pos = origin.offset(x, local_y, z);
                let Some(current) = level.get_block(pos).cloned() else {
                    continue;
                };
                if !is_iceberg_state(&current.name) && current.name != "minecraft:snow_block" {
                    continue;
                }
                if place_water {
                    level.set_block(pos, state("minecraft:water"));
                } else {
                    level.set_block(pos, state("minecraft:air"));
                    if level.get_block(pos.above()).map(block_name) == Some("minecraft:snow") {
                        level.set_block(pos.above(), state("minecraft:air"));
                    }
                }
            }
        }
    }
}

fn signed_distance_circle(random: &mut dyn RandomSource, x: i32, z: i32, radius: i32) -> f64 {
    let noise = 10.0 * f64::from(random.next_float().clamp(0.2, 0.8)) / f64::from(radius);
    noise + f64::from(x * x) + f64::from(z * z) - f64::from(radius * radius)
}

fn signed_distance_ellipse(
    x: i32,
    z: i32,
    center_x: i32,
    center_z: i32,
    major: i32,
    minor: i32,
    angle: f64,
) -> f64 {
    let dx = f64::from(x - center_x);
    let dz = f64::from(z - center_z);
    ((dx * angle.cos() - dz * angle.sin()) / f64::from(major)).powi(2)
        + ((dx * angle.sin() + dz * angle.cos()) / f64::from(minor)).powi(2)
        - 1.0
}

fn ellipse_c(y: i32, height: i32, minor_axis: i32) -> i32 {
    let mut result = minor_axis;
    if y > 0 && height - y <= 3 {
        result = minor_axis - (4 - (height - y));
    }
    result
}

fn height_dependent_radius_round(
    random: &mut dyn RandomSource,
    y: i32,
    height: i32,
    major_axis: i32,
) -> i32 {
    let f = 3.5 - random.next_float();
    let mut radius = (1.0 - (y * y) as f32 / (height as f32 * f)) * major_axis as f32;
    if height > 15 + random.next_int_bound(5) {
        let adjusted = if y < 3 + random.next_int_bound(6) {
            y / 2
        } else {
            y
        };
        radius = (1.0 - adjusted as f32 / (height as f32 * f * 0.4)) * major_axis as f32;
    }
    (radius / 2.0).ceil() as i32
}

fn height_dependent_radius_ellipse(y: i32, height: i32, max_radius: i32) -> i32 {
    let radius = (1.0 - (y * y) as f32 / height as f32) * max_radius as f32;
    (radius / 2.0).ceil() as i32
}

fn height_dependent_radius_steep(
    random: &mut dyn RandomSource,
    y: i32,
    height: i32,
    max_radius: i32,
) -> i32 {
    let f = 1.0 + random.next_float() / 2.0;
    let radius = (1.0 - y as f32 / (height as f32 * f)) * max_radius as f32;
    (radius / 2.0).ceil() as i32
}

fn is_iceberg_state(name: &str) -> bool {
    matches!(
        name,
        "minecraft:packed_ice" | "minecraft:snow_block" | "minecraft:blue_ice"
    )
}

pub(crate) fn parse_iceberg_config(config: &Value) -> io::Result<Option<IcebergFeature>> {
    let object = object(config, "iceberg")?;
    let state = parse_state(required(object, "state", "iceberg")?)?;
    Ok(Some(IcebergFeature { state }))
}

#[derive(Debug, Clone)]
pub struct IceSpikeFeature;

impl IceSpikeFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut pos = origin;
        while is_air(level, pos) && pos.y() > level.min_y() + 2 {
            pos = pos.below();
        }
        if level.get_block(pos).map(block_name) != Some("minecraft:snow_block") {
            return false;
        }
        pos = pos.above_n(random.next_int_bound(4));
        let height = random.next_int_bound(4) + 7;
        let radius = height / 4 + random.next_int_bound(2);
        if radius > 1 && random.next_int_bound(60) == 0 {
            pos = pos.above_n(10 + random.next_int_bound(30));
        }

        for y in 0..height {
            let f = (1.0 - y as f32 / height as f32) * radius as f32;
            let ceil = f.ceil() as i32;
            for x in -ceil..=ceil {
                let fx = x.abs() as f32 - 0.25;
                for z in -ceil..=ceil {
                    let fz = z.abs() as f32 - 0.25;
                    let inside = (x == 0 && z == 0) || fx * fx + fz * fz <= f * f;
                    let edge = x == -ceil || x == ceil || z == -ceil || z == ceil;
                    let corner_ok = !edge || random.next_float() <= 0.75;
                    if !(inside && corner_ok) {
                        continue;
                    }
                    let target = pos.offset(x, y, z);
                    let Some(current) = level.get_block(target).cloned() else {
                        continue;
                    };
                    if can_ice_spike_replace(&current.name) {
                        level.set_block(target, state("minecraft:packed_ice"));
                    }
                    if y != 0 && ceil > 1 {
                        let bottom = pos.offset(x, -y, z);
                        let Some(current) = level.get_block(bottom).cloned() else {
                            continue;
                        };
                        if can_ice_spike_replace(&current.name) {
                            level.set_block(bottom, state("minecraft:packed_ice"));
                        }
                    }
                }
            }
        }

        let base_radius = (radius - 1).clamp(0, 1);
        for x in -base_radius..=base_radius {
            for z in -base_radius..=base_radius {
                let mut column = pos.offset(x, -1, z);
                let mut budget = 50;
                if x.abs() == 1 && z.abs() == 1 {
                    budget = random.next_int_bound(5);
                }
                while column.y() > 50 {
                    let Some(current) = level.get_block(column).cloned() else {
                        break;
                    };
                    if !can_ice_spike_base_replace(&current.name) {
                        break;
                    }
                    level.set_block(column, state("minecraft:packed_ice"));
                    column = column.below();
                    budget -= 1;
                    if budget <= 0 {
                        column = column.below_n(random.next_int_bound(5) + 1);
                        budget = random.next_int_bound(5);
                    }
                }
            }
        }
        true
    }
}

fn can_ice_spike_replace(name: &str) -> bool {
    is_air_name(name)
        || tags::block_has("minecraft:dirt", name)
        || matches!(name, "minecraft:snow_block" | "minecraft:ice")
}

fn can_ice_spike_base_replace(name: &str) -> bool {
    can_ice_spike_replace(name) || name == "minecraft:packed_ice"
}

pub(crate) fn parse_ice_spike_config(_config: &Value) -> Option<IceSpikeFeature> {
    Some(IceSpikeFeature)
}

#[derive(Debug, Clone)]
pub struct LakeFeature {
    fluid: StateProvider,
    barrier: StateProvider,
}

impl LakeFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if origin.y() <= level.min_y() + 4 {
            return false;
        }
        let origin = origin.below_n(4);
        let mut flags = [false; 2048];
        let spheres = random.next_int_bound(4) + 4;

        for _ in 0..spheres {
            let d = random.next_double() * 6.0 + 3.0;
            let d1 = random.next_double() * 4.0 + 2.0;
            let d2 = random.next_double() * 6.0 + 3.0;
            let d3 = random.next_double() * (16.0 - d - 2.0) + 1.0 + d / 2.0;
            let d4 = random.next_double() * (8.0 - d1 - 4.0) + 2.0 + d1 / 2.0;
            let d5 = random.next_double() * (16.0 - d2 - 2.0) + 1.0 + d2 / 2.0;
            for x in 1i32..15 {
                for z in 1i32..15 {
                    for y in 1i32..7 {
                        let dx = (f64::from(x) - d3) / (d / 2.0);
                        let dy = (f64::from(y) - d4) / (d1 / 2.0);
                        let dz = (f64::from(z) - d5) / (d2 / 2.0);
                        if dx * dx + dy * dy + dz * dz < 1.0 {
                            flags[((x * 16 + z) * 8 + y) as usize] = true;
                        }
                    }
                }
            }
        }

        let fluid = self
            .fluid
            .sample(level, random, origin)
            .expect("lake fluid provider is supported");
        if fluid.name == "minecraft:water" {
            // Water lakes need `Biome.shouldFreeze`, which the region cannot
            // observe. The shipped data only contains `lake_lava`; reject a
            // water config instead of silently skipping the freeze pass.
            return false;
        }

        for x in 0i32..16 {
            for z in 0i32..16 {
                for y in 0i32..8 {
                    let neighbour = !flags[((x * 16 + z) * 8 + y) as usize]
                        && ((x < 15 && flags[(((x + 1) * 16 + z) * 8 + y) as usize])
                            || (x > 0 && flags[(((x - 1) * 16 + z) * 8 + y) as usize])
                            || (z < 15 && flags[((x * 16 + z + 1) * 8 + y) as usize])
                            || (z > 0 && flags[((x * 16 + (z - 1)) * 8 + y) as usize])
                            || (y < 7 && flags[((x * 16 + z) * 8 + y + 1) as usize])
                            || (y > 0 && flags[((x * 16 + z) * 8 + (y - 1)) as usize]));
                    if !neighbour {
                        continue;
                    }
                    let pos = origin.offset(x, y, z);
                    let current = level
                        .get_block(pos)
                        .cloned()
                        .unwrap_or_else(|| state("minecraft:air"));
                    if y >= 4 && crate::block_behaviour::fluid(&current).is_some() {
                        return false;
                    }
                    if y < 4 && !is_solid(level, pos) && current != fluid {
                        return false;
                    }
                }
            }
        }

        for x in 0i32..16 {
            for z in 0i32..16 {
                for y in 0i32..8 {
                    if !flags[((x * 16 + z) * 8 + y) as usize] {
                        continue;
                    }
                    let pos = origin.offset(x, y, z);
                    let blocked = level.get_block(pos).is_some_and(|state| {
                        tags::block_has("minecraft:features_cannot_replace", &state.name)
                    });
                    if blocked {
                        continue;
                    }
                    if y >= 4 {
                        level.set_block(pos, state("minecraft:cave_air"));
                        level.schedule_tick(pos, "minecraft:cave_air", 0);
                    } else {
                        level.set_block(pos, fluid.clone());
                    }
                }
            }
        }

        let barrier = self
            .barrier
            .sample(level, random, origin)
            .expect("lake barrier provider is supported");
        if !is_air_name(&barrier.name) {
            for x in 0i32..16 {
                for z in 0i32..16 {
                    for y in 0i32..8 {
                        let neighbour = !flags[((x * 16 + z) * 8 + y) as usize]
                            && ((x < 15 && flags[(((x + 1) * 16 + z) * 8 + y) as usize])
                                || (x > 0 && flags[(((x - 1) * 16 + z) * 8 + y) as usize])
                                || (z < 15 && flags[((x * 16 + z + 1) * 8 + y) as usize])
                                || (z > 0 && flags[((x * 16 + (z - 1)) * 8 + y) as usize])
                                || (y < 7 && flags[((x * 16 + z) * 8 + y + 1) as usize])
                                || (y > 0 && flags[((x * 16 + z) * 8 + (y - 1)) as usize]));
                        if !neighbour || (y >= 4 && random.next_int_bound(2) == 0) {
                            continue;
                        }
                        let pos = origin.offset(x, y, z);
                        let Some(current) = level.get_block(pos) else {
                            continue;
                        };
                        if crate::block_behaviour::is_solid(current)
                            && !tags::block_has(
                                "minecraft:lava_pool_stone_cannot_replace",
                                &current.name,
                            )
                        {
                            level.set_block(pos, barrier.clone());
                        }
                    }
                }
            }
        }
        true
    }
}

pub(crate) fn parse_lake_config(config: &Value) -> io::Result<Option<LakeFeature>> {
    let object = object(config, "lake")?;
    let fluid = StateProvider::parse(required(object, "fluid", "lake")?)?;
    let barrier = StateProvider::parse(required(object, "barrier", "lake")?)?;
    if !fluid.is_supported() || !barrier.is_supported() {
        return Ok(None);
    }
    Ok(Some(LakeFeature { fluid, barrier }))
}

#[derive(Debug, Clone)]
pub struct ReplaceBlobsFeature {
    radius: IntProvider,
    state: BlockState,
    target: BlockState,
}

impl ReplaceBlobsFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let max_y = level.min_y() + level.height() - 1;
        let mut cursor = BlockPos::new(
            origin.x(),
            origin.y().clamp(level.min_y() + 1, max_y),
            origin.z(),
        );
        let mut target = None;
        while cursor.y() > level.min_y() + 1 {
            if level.get_block(cursor).map(block_name) == Some(&self.target.name) {
                target = Some(cursor);
                break;
            }
            cursor = cursor.below();
        }
        let Some(target) = target else {
            return false;
        };
        let rx = self.radius.sample(random).expect("int provider is ported");
        let ry = self.radius.sample(random).expect("int provider is ported");
        let rz = self.radius.sample(random).expect("int provider is ported");
        let max = rx.max(ry).max(rz);
        let mut placed = false;
        for distance in 0..=max {
            for x in -rx..=rx {
                for y in -ry..=ry {
                    for z in -rz..=rz {
                        if x.abs() + y.abs() + z.abs() != distance {
                            continue;
                        }
                        let pos = target.offset(x, y, z);
                        if level.get_block(pos).map(block_name) == Some(&self.target.name) {
                            level.set_block(pos, self.state.clone());
                            placed = true;
                        }
                    }
                }
            }
        }
        placed
    }
}

pub(crate) fn parse_replace_blobs_config(
    config: &Value,
) -> io::Result<Option<ReplaceBlobsFeature>> {
    let object = object(config, "netherrack_replace_blobs")?;
    let radius = parse_int_provider_supported(required(object, "radius", "replace blobs")?)?;
    Ok(Some(ReplaceBlobsFeature {
        radius,
        state: parse_state(required(object, "state", "replace blobs")?)?,
        target: parse_state(required(object, "target", "replace blobs")?)?,
    }))
}

#[derive(Debug, Clone)]
pub struct BasaltColumnsFeature {
    height: IntProvider,
    reach: IntProvider,
}

impl BasaltColumnsFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let sea_level = level.sea_level();
        if !can_place_at(level, sea_level, origin) {
            return false;
        }
        let height = self.height.sample(random).expect("int provider is ported");
        let clustered = random.next_float() < 0.9;
        let min = height.min(if clustered { 5 } else { 8 });
        let size = if clustered { 50 } else { 15 };
        let mut placed = false;

        let min_x = origin.x() - min;
        let min_z = origin.z() - min;
        let width = min * 2 + 1;
        for _ in 0..size {
            let x = min_x + random.next_int_bound(width);
            let _y = origin.y() + random.next_int_bound(1);
            let z = min_z + random.next_int_bound(width);
            let pos = BlockPos::new(x, origin.y(), z);
            let distance = pos.x().abs_diff(origin.x()) as i32
                + pos.y().abs_diff(origin.y()) as i32
                + pos.z().abs_diff(origin.z()) as i32;
            let column_height = height - distance;
            if column_height < 0 {
                continue;
            }
            let reach = self.reach.sample(random).expect("int provider is ported");
            placed |= place_column(level, sea_level, pos, column_height, reach);
        }
        placed
    }
}

fn cannot_place_on(name: &str) -> bool {
    matches!(
        name,
        "minecraft:lava"
            | "minecraft:bedrock"
            | "minecraft:magma_block"
            | "minecraft:soul_sand"
            | "minecraft:nether_bricks"
            | "minecraft:nether_brick_fence"
            | "minecraft:nether_brick_stairs"
            | "minecraft:nether_wart"
            | "minecraft:chest"
            | "minecraft:spawner"
    )
}

fn is_air_or_lava_ocean(level: &dyn FeatureLevel, sea_level: i32, pos: BlockPos) -> bool {
    level.get_block(pos).is_none_or(|state| {
        is_air_name(&state.name) || (state.name == "minecraft:lava" && pos.y() <= sea_level)
    })
}

fn can_place_at(level: &dyn FeatureLevel, sea_level: i32, pos: BlockPos) -> bool {
    if !is_air_or_lava_ocean(level, sea_level, pos) {
        return false;
    }
    level
        .get_block(pos.below())
        .is_some_and(|state| !is_air_name(&state.name) && !cannot_place_on(&state.name))
}

fn find_surface(
    level: &dyn FeatureLevel,
    sea_level: i32,
    mut pos: BlockPos,
    mut distance: i32,
) -> Option<BlockPos> {
    while pos.y() > level.min_y() + 1 && distance > 0 {
        distance -= 1;
        if can_place_at(level, sea_level, pos) {
            return Some(pos);
        }
        pos = pos.below();
    }
    None
}

fn find_air(level: &dyn FeatureLevel, mut pos: BlockPos, mut distance: i32) -> Option<BlockPos> {
    let max_y = level.min_y() + level.height() - 1;
    while pos.y() <= max_y && distance > 0 {
        distance -= 1;
        let state = level.get_block(pos)?;
        if cannot_place_on(&state.name) {
            return None;
        }
        if is_air_name(&state.name) {
            return Some(pos);
        }
        pos = pos.above();
    }
    None
}

fn place_column(
    level: &mut dyn FeatureLevel,
    sea_level: i32,
    pos: BlockPos,
    distance: i32,
    reach: i32,
) -> bool {
    let mut placed = false;
    for x in pos.x() - reach..=pos.x() + reach {
        for z in pos.z() - reach..=pos.z() + reach {
            let candidate = BlockPos::new(x, pos.y(), z);
            let manhattan = candidate.x().abs_diff(pos.x()) as i32
                + candidate.y().abs_diff(pos.y()) as i32
                + candidate.z().abs_diff(pos.z()) as i32;
            let found = if is_air_or_lava_ocean(level, sea_level, candidate) {
                find_surface(level, sea_level, candidate, manhattan)
            } else {
                find_air(level, candidate, manhattan)
            };
            let Some(found) = found else {
                continue;
            };
            let mut remaining = distance - manhattan / 2;
            let mut cursor = found;
            while remaining >= 0 {
                if is_air_or_lava_ocean(level, sea_level, cursor) {
                    level.set_block(cursor, state("minecraft:basalt"));
                    cursor = cursor.above();
                    placed = true;
                } else if level.get_block(cursor).map(block_name) == Some("minecraft:basalt") {
                    cursor = cursor.above();
                } else {
                    break;
                }
                remaining -= 1;
            }
        }
    }
    placed
}

pub(crate) fn parse_basalt_columns_config(
    config: &Value,
) -> io::Result<Option<BasaltColumnsFeature>> {
    let object = object(config, "basalt columns")?;
    Ok(Some(BasaltColumnsFeature {
        height: parse_int_provider_supported(required(object, "height", "basalt columns")?)?,
        reach: parse_int_provider_supported(required(object, "reach", "basalt columns")?)?,
    }))
}

#[derive(Debug, Clone)]
pub struct BasaltPillarFeature;

impl BasaltPillarFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if !is_air(level, origin) || is_air(level, origin.above()) {
            return false;
        }
        let mut cursor = origin;
        let mut north = true;
        let mut south = true;
        let mut west = true;
        let mut east = true;
        while is_air(level, cursor) {
            if cursor.y() < level.min_y() || cursor.y() >= level.min_y() + level.height() {
                return true;
            }
            level.set_block(cursor, state("minecraft:basalt"));
            north &= place_hang_off(level, random, cursor.north());
            south &= place_hang_off(level, random, cursor.south());
            west &= place_hang_off(level, random, cursor.west());
            east &= place_hang_off(level, random, cursor.east());
            cursor = cursor.below();
        }
        // The four flags only gate later draws; vanilla never reads them after
        // the loop, but they keep the RNG stream aligned.
        let _ = (north, south, west, east);
        cursor = cursor.above();
        place_base_hang_off(level, random, cursor.north());
        place_base_hang_off(level, random, cursor.south());
        place_base_hang_off(level, random, cursor.west());
        place_base_hang_off(level, random, cursor.east());
        cursor = cursor.below();

        for x in -3i32..4 {
            for z in -3i32..4 {
                let edge = x.abs() * z.abs();
                if random.next_int_bound(10) < 10 - edge {
                    let mut pos = cursor.offset(x, 0, z);
                    let mut budget = 3;
                    while is_air(level, pos.below()) {
                        pos = pos.below();
                        budget -= 1;
                        if budget <= 0 {
                            break;
                        }
                    }
                    if !is_air(level, pos.below()) {
                        level.set_block(pos, state("minecraft:basalt"));
                    }
                }
            }
        }
        true
    }
}

fn place_hang_off(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    pos: BlockPos,
) -> bool {
    if random.next_int_bound(10) != 0 {
        level.set_block(pos, state("minecraft:basalt"));
        true
    } else {
        false
    }
}

fn place_base_hang_off(level: &mut dyn FeatureLevel, random: &mut dyn RandomSource, pos: BlockPos) {
    if random.next_boolean() {
        level.set_block(pos, state("minecraft:basalt"));
    }
}

pub(crate) fn parse_basalt_pillar_config(_config: &Value) -> Option<BasaltPillarFeature> {
    Some(BasaltPillarFeature)
}

#[derive(Debug, Clone)]
pub struct DeltaFeature {
    contents: BlockState,
    rim: BlockState,
    rim_size: IntProvider,
    size: IntProvider,
}

impl DeltaFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut placed = false;
        let rimmed = random.next_double() < 0.9;
        let rim_x = if rimmed {
            self.rim_size
                .sample(random)
                .expect("int provider is ported")
        } else {
            0
        };
        let rim_z = if rimmed {
            self.rim_size
                .sample(random)
                .expect("int provider is ported")
        } else {
            0
        };
        let has_rim = rimmed && rim_x != 0 && rim_z != 0;
        let size_x = self.size.sample(random).expect("int provider is ported");
        let size_z = self.size.sample(random).expect("int provider is ported");
        let max = size_x.max(size_z);

        for distance in 0..=max {
            for x in -size_x..=size_x {
                for z in -size_z..=size_z {
                    if x.abs() + z.abs() != distance {
                        continue;
                    }
                    let pos = origin.offset(x, 0, z);
                    if !delta_is_clear(level, pos, &self.contents) {
                        continue;
                    }
                    if has_rim {
                        placed = true;
                        level.set_block(pos, self.rim.clone());
                    }
                    let shifted = pos.offset(rim_x, 0, rim_z);
                    if delta_is_clear(level, shifted, &self.contents) {
                        placed = true;
                        level.set_block(shifted, self.contents.clone());
                    }
                }
            }
        }
        placed
    }
}

fn delta_is_clear(level: &dyn FeatureLevel, pos: BlockPos, contents: &BlockState) -> bool {
    let Some(state) = level.get_block(pos) else {
        return false;
    };
    if state.name == contents.name {
        return false;
    }
    if matches!(
        state.name.as_str(),
        "minecraft:bedrock"
            | "minecraft:nether_bricks"
            | "minecraft:nether_brick_fence"
            | "minecraft:nether_brick_stairs"
            | "minecraft:nether_wart"
            | "minecraft:chest"
            | "minecraft:spawner"
    ) {
        return false;
    }
    for direction in Direction::ALL {
        let neighbour_air = is_air(level, pos.relative(direction));
        if (neighbour_air && direction != Direction::Up)
            || (!neighbour_air && direction == Direction::Up)
        {
            return false;
        }
    }
    true
}

pub(crate) fn parse_delta_config(config: &Value) -> io::Result<Option<DeltaFeature>> {
    let object = object(config, "delta")?;
    Ok(Some(DeltaFeature {
        contents: parse_state(required(object, "contents", "delta")?)?,
        rim: parse_state(required(object, "rim", "delta")?)?,
        rim_size: parse_int_provider_supported(required(object, "rim_size", "delta")?)?,
        size: parse_int_provider_supported(required(object, "size", "delta")?)?,
    }))
}

/// `minecraft:geode` block settings (`GeodeBlockSettings`).
#[derive(Debug, Clone)]
struct GeodeBlocks {
    filling: StateProvider,
    inner_layer: StateProvider,
    alternate_inner_layer: StateProvider,
    middle_layer: StateProvider,
    outer_layer: StateProvider,
    inner_placements: Vec<BlockState>,
    cannot_replace: String,
    invalid_blocks: String,
}

/// `minecraft:geode` layer radii (`GeodeLayerSettings`).
#[derive(Debug, Clone, Copy)]
struct GeodeLayers {
    filling: f64,
    inner_layer: f64,
    middle_layer: f64,
    outer_layer: f64,
}

/// `minecraft:geode` crack settings (`GeodeCrackSettings`).
#[derive(Debug, Clone, Copy)]
struct GeodeCrack {
    generate_crack_chance: f64,
    base_crack_size: f64,
    crack_point_offset: i32,
}

/// `GeodeFeature`.
#[derive(Debug, Clone)]
pub struct GeodeFeature {
    blocks: GeodeBlocks,
    layers: GeodeLayers,
    crack: GeodeCrack,
    use_potential_placements_chance: f64,
    use_alternate_layer0_chance: f64,
    placements_require_layer0_alternate: bool,
    outer_wall_distance: IntProvider,
    outer_wall_max: i32,
    distribution_points: IntProvider,
    point_offset: IntProvider,
    min_gen_offset: i32,
    max_gen_offset: i32,
    noise_multiplier: f64,
    invalid_blocks_threshold: i32,
}

impl GeodeFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let point_count = self
            .distribution_points
            .sample(random)
            .expect("geode distribution_points is a ported int provider");
        let noise = NormalNoise::create_legacy_seed(level.seed(), -4, &[1.0]);

        let outer_wall_max = f64::from(self.outer_wall_max);
        let d = f64::from(point_count) / outer_wall_max;
        let d1 = 1.0 / self.layers.filling.sqrt();
        let d2 = 1.0 / (self.layers.inner_layer + d).sqrt();
        let d3 = 1.0 / (self.layers.middle_layer + d).sqrt();
        let d4 = 1.0 / (self.layers.outer_layer + d).sqrt();
        let crack_bonus = if point_count > 3 { d } else { 0.0 };
        let d5 =
            1.0 / (self.crack.base_crack_size + random.next_double() / 2.0 + crack_bonus).sqrt();
        let has_crack = f64::from(random.next_float()) < self.crack.generate_crack_chance;

        let mut points = Vec::with_capacity(point_count.max(0) as usize);
        let mut invalid = 0i32;
        for _ in 0..point_count {
            let x = self
                .outer_wall_distance
                .sample(random)
                .expect("geode outer_wall_distance is a ported int provider");
            let y = self
                .outer_wall_distance
                .sample(random)
                .expect("geode outer_wall_distance is a ported int provider");
            let z = self
                .outer_wall_distance
                .sample(random)
                .expect("geode outer_wall_distance is a ported int provider");
            let pos = origin.offset(x, y, z);
            let invalid_state = level.get_block(pos).is_none_or(|block| {
                is_air_name(&block.name)
                    || tags::block_has(&self.blocks.invalid_blocks, &block.name)
            });
            if invalid_state {
                invalid += 1;
                if invalid > self.invalid_blocks_threshold {
                    return false;
                }
            }
            let offset = self
                .point_offset
                .sample(random)
                .expect("geode point_offset is a ported int provider");
            points.push((pos, offset));
        }

        let mut crack_points = Vec::new();
        if has_crack {
            let axis = random.next_int_bound(4);
            let span = point_count * 2 + 1;
            if axis == 0 {
                crack_points.push(origin.offset(span, 7, 0));
                crack_points.push(origin.offset(span, 5, 0));
                crack_points.push(origin.offset(span, 1, 0));
            } else if axis == 1 {
                crack_points.push(origin.offset(0, 7, span));
                crack_points.push(origin.offset(0, 5, span));
                crack_points.push(origin.offset(0, 1, span));
            } else if axis == 2 {
                crack_points.push(origin.offset(span, 7, span));
                crack_points.push(origin.offset(span, 5, span));
                crack_points.push(origin.offset(span, 1, span));
            } else {
                crack_points.push(origin.offset(0, 7, 0));
                crack_points.push(origin.offset(0, 5, 0));
                crack_points.push(origin.offset(0, 1, 0));
            }
        }

        let air = state("minecraft:air");
        let mut placement_positions = Vec::new();
        let min = origin.offset(
            self.min_gen_offset,
            self.min_gen_offset,
            self.min_gen_offset,
        );
        let max = origin.offset(
            self.max_gen_offset,
            self.max_gen_offset,
            self.max_gen_offset,
        );
        for z in min.z()..=max.z() {
            for y in min.y()..=max.y() {
                for x in min.x()..=max.x() {
                    let pos = BlockPos::new(x, y, z);
                    let d6 = noise.get_value(f64::from(x), f64::from(y), f64::from(z))
                        * self.noise_multiplier;
                    let mut d7 = 0.0;
                    let mut d8 = 0.0;
                    for (point, offset) in &points {
                        d7 += inverse_sqrt(distance_squared(pos, *point) + f64::from(*offset)) + d6;
                    }
                    for point in &crack_points {
                        d8 += inverse_sqrt(
                            distance_squared(pos, *point)
                                + f64::from(self.crack.crack_point_offset),
                        ) + d6;
                    }

                    if d7 < d4 {
                        continue;
                    }

                    if has_crack && d8 >= d5 && d7 < d1 {
                        safe_set_geode(level, pos, air.clone(), &self.blocks.cannot_replace);
                        for direction in Direction::ALL {
                            let neighbour = pos.relative(direction);
                            if let Some(fluid) = fluid_block_at(level, neighbour) {
                                level.schedule_tick(neighbour, fluid, 0);
                            }
                        }
                    } else if d7 >= d1 {
                        if let Some(block) = self.blocks.filling.sample(level, random, pos) {
                            safe_set_geode(level, pos, block, &self.blocks.cannot_replace);
                        }
                    } else if d7 >= d2 {
                        let alternate =
                            f64::from(random.next_float()) < self.use_alternate_layer0_chance;
                        let provider = if alternate {
                            &self.blocks.alternate_inner_layer
                        } else {
                            &self.blocks.inner_layer
                        };
                        if let Some(block) = provider.sample(level, random, pos) {
                            safe_set_geode(level, pos, block, &self.blocks.cannot_replace);
                        }
                        if (!self.placements_require_layer0_alternate || alternate)
                            && f64::from(random.next_float()) < self.use_potential_placements_chance
                        {
                            placement_positions.push(pos);
                        }
                    } else if d7 >= d3 {
                        if let Some(block) = self.blocks.middle_layer.sample(level, random, pos) {
                            safe_set_geode(level, pos, block, &self.blocks.cannot_replace);
                        }
                    } else {
                        if let Some(block) = self.blocks.outer_layer.sample(level, random, pos) {
                            safe_set_geode(level, pos, block, &self.blocks.cannot_replace);
                        }
                    }
                }
            }
        }

        for pos in placement_positions {
            let index = random.next_int_bound(self.blocks.inner_placements.len() as i32) as usize;
            let mut placement = self.blocks.inner_placements[index].clone();
            for direction in Direction::ALL {
                if placement.properties.iter().any(|(key, _)| key == "facing") {
                    let _ = super::support::set_property(
                        &mut placement,
                        "facing",
                        direction_name(direction),
                    );
                }
                let neighbour = pos.relative(direction);
                if placement
                    .properties
                    .iter()
                    .any(|(key, _)| key == "waterlogged")
                {
                    let waterlogged = level.get_block(neighbour).is_some_and(is_water_source);
                    let _ = super::support::set_property(
                        &mut placement,
                        "waterlogged",
                        if waterlogged { "true" } else { "false" },
                    );
                }
                if level
                    .get_block(neighbour)
                    .is_some_and(can_cluster_grow_at_state)
                {
                    safe_set_geode(level, neighbour, placement, &self.blocks.cannot_replace);
                    break;
                }
            }
        }

        true
    }
}

fn inverse_sqrt(value: f64) -> f64 {
    1.0 / value.sqrt()
}

fn distance_squared(left: BlockPos, right: BlockPos) -> f64 {
    let dx = f64::from(left.x() - right.x());
    let dy = f64::from(left.y() - right.y());
    let dz = f64::from(left.z() - right.z());
    dx * dx + dy * dy + dz * dz
}

/// `Feature.safeSetBlock`: only replace blocks not in `cannot_replace`.
fn safe_set_geode(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    block: BlockState,
    cannot_replace: &str,
) {
    if level
        .get_block(pos)
        .is_none_or(|current| !tags::block_has(cannot_replace, &current.name))
    {
        level.set_block(pos, block);
    }
}

/// `BuddingAmethystBlock.canClusterGrowAtState`.
fn can_cluster_grow_at_state(state: &BlockState) -> bool {
    is_air_name(&state.name) || is_water_source(state)
}

fn is_water_source(state: &BlockState) -> bool {
    match state.name.as_str() {
        "minecraft:water" => state
            .properties
            .iter()
            .find(|(key, _)| key == "level")
            .is_none_or(|(_, level)| level == "0"),
        "minecraft:flowing_water" => false,
        _ => state
            .properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true"),
    }
}

fn fluid_block_at(level: &dyn FeatureLevel, pos: BlockPos) -> Option<&'static str> {
    let state = level.get_block(pos)?;
    match state.name.as_str() {
        "minecraft:water" | "minecraft:flowing_water" => Some("minecraft:water"),
        "minecraft:lava" | "minecraft:flowing_lava" => Some("minecraft:lava"),
        _ if state
            .properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true") =>
        {
            Some("minecraft:water")
        }
        _ => None,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

fn double_field(object: &Map<String, Value>, key: &str, kind: &str) -> io::Result<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| super::invalid(format!("{kind} is missing `{key}`")))
}

fn optional_double(object: &Map<String, Value>, key: &str, default: f64) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn tag_field(object: &Map<String, Value>, key: &str, kind: &str) -> io::Result<String> {
    let value = required(object, key, kind)?
        .as_str()
        .ok_or_else(|| super::invalid(format!("{kind} `{key}` must be a tag id")))?;
    Ok(value.trim_start_matches('#').to_owned())
}

/// The largest value a ported int provider can sample.
fn int_provider_max(provider: &IntProvider) -> Option<i32> {
    match provider {
        IntProvider::Constant(value) => Some(*value),
        IntProvider::Uniform { max_inclusive, .. }
        | IntProvider::BiasedToBottom { max_inclusive, .. }
        | IntProvider::Clamped { max_inclusive, .. } => Some(*max_inclusive),
        IntProvider::WeightedList(entries) => entries
            .iter()
            .filter_map(|entry| int_provider_max(&entry.data))
            .max(),
        IntProvider::ClampedNormal { max_inclusive, .. } => Some(*max_inclusive),
        IntProvider::Unsupported { .. } => None,
    }
}

pub(crate) fn parse_geode_config(config: &Value) -> io::Result<Option<GeodeFeature>> {
    let object = object(config, "geode")?;
    let block_object = required(object, "blocks", "geode")?
        .as_object()
        .ok_or_else(|| super::invalid("geode `blocks` must be an object"))?;
    let parse_provider = |key: &str| -> io::Result<StateProvider> {
        StateProvider::parse(required(block_object, key, "geode blocks")?)
    };
    let filling = parse_provider("filling_provider")?;
    let inner_layer = parse_provider("inner_layer_provider")?;
    let alternate_inner_layer = parse_provider("alternate_inner_layer_provider")?;
    let middle_layer = parse_provider("middle_layer_provider")?;
    let outer_layer = parse_provider("outer_layer_provider")?;
    if !filling.is_supported()
        || !inner_layer.is_supported()
        || !alternate_inner_layer.is_supported()
        || !middle_layer.is_supported()
        || !outer_layer.is_supported()
    {
        return Ok(None);
    }

    let inner_placements = required(block_object, "inner_placements", "geode blocks")?
        .as_array()
        .ok_or_else(|| super::invalid("geode `inner_placements` must be a list"))?;
    if inner_placements.is_empty() {
        return Err(super::invalid("geode `inner_placements` must not be empty"));
    }
    let inner_placements = inner_placements
        .iter()
        .map(parse_state)
        .collect::<io::Result<Vec<_>>>()?;

    let layer_object = required(object, "layers", "geode")?
        .as_object()
        .ok_or_else(|| super::invalid("geode `layers` must be an object"))?;
    let layers = GeodeLayers {
        filling: double_field(layer_object, "filling", "geode layers")?,
        inner_layer: double_field(layer_object, "inner_layer", "geode layers")?,
        middle_layer: double_field(layer_object, "middle_layer", "geode layers")?,
        outer_layer: double_field(layer_object, "outer_layer", "geode layers")?,
    };
    if layers.filling <= 0.0
        || layers.inner_layer <= 0.0
        || layers.middle_layer <= 0.0
        || layers.outer_layer <= 0.0
    {
        return Err(super::invalid("geode layer radii must be positive"));
    }

    let crack_object = required(object, "crack", "geode")?
        .as_object()
        .ok_or_else(|| super::invalid("geode `crack` must be an object"))?;
    let crack = GeodeCrack {
        generate_crack_chance: double_field(crack_object, "generate_crack_chance", "geode crack")?,
        base_crack_size: double_field(crack_object, "base_crack_size", "geode crack")?,
        crack_point_offset: int_field(crack_object, "crack_point_offset")?,
    };

    let outer_wall_distance =
        parse_int_provider_supported(required(object, "outer_wall_distance", "geode")?)?;
    let Some(outer_wall_max) = int_provider_max(&outer_wall_distance) else {
        return Ok(None);
    };
    if outer_wall_max <= 0 {
        return Err(super::invalid(
            "geode outer_wall_distance max must be positive",
        ));
    }

    Ok(Some(GeodeFeature {
        blocks: GeodeBlocks {
            filling,
            inner_layer,
            alternate_inner_layer,
            middle_layer,
            outer_layer,
            inner_placements,
            cannot_replace: tag_field(block_object, "cannot_replace", "geode blocks")?,
            invalid_blocks: tag_field(block_object, "invalid_blocks", "geode blocks")?,
        },
        layers,
        crack,
        use_potential_placements_chance: optional_double(
            object,
            "use_potential_placements_chance",
            0.35,
        ),
        use_alternate_layer0_chance: optional_double(object, "use_alternate_layer0_chance", 0.0),
        placements_require_layer0_alternate: optional_bool(
            object,
            "placements_require_layer0_alternate",
            true,
        ),
        outer_wall_distance,
        outer_wall_max,
        distribution_points: parse_int_provider_supported(required(
            object,
            "distribution_points",
            "geode",
        )?)?,
        point_offset: parse_int_provider_supported(required(object, "point_offset", "geode")?)?,
        min_gen_offset: int_field(object, "min_gen_offset")?,
        max_gen_offset: int_field(object, "max_gen_offset")?,
        noise_multiplier: double_field(object, "noise_multiplier", "geode")?,
        invalid_blocks_threshold: int_field(object, "invalid_blocks_threshold")?,
    }))
}
