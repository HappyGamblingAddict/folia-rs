//! Status effects, armor calculation, typed damage, and effect packets.
//!
//! This is the P5 damage/effect slice: it models the subset of
//! `net/minecraft/world/effect/MobEffectInstance`, `LivingEntity` armor
//! reduction, and vanilla `DamageType` flags that the server already tracks.
//! Effects are stored on a [`LivingEntity`]; this module owns the tick math,
//! the merge rules, and the `ClientboundUpdateMobEffect` /
//! `ClientboundRemoveMobEffect` wire format.

use crate::entity_metadata::{Equipment, EquipmentSlot, HurtResult, LivingEntity};
use crate::item_components::{ItemStack, MobEffectInstance, PotionContents, normalize_key};
use crate::protocol::{PacketReader, PacketWriter};
use std::io;

/// `GameProtocols.CLIENTBOUND_TEMPLATE` registration order in 1.21.11.
/// `CLIENTBOUND_BUNDLE` occupies id 0; `REMOVE_MOB_EFFECT` is the 76th packet
/// after the bundle delimiter and `UPDATE_MOB_EFFECT` the 130th.
pub const CLIENTBOUND_REMOVE_MOB_EFFECT: i32 = 76;
pub const CLIENTBOUND_UPDATE_MOB_EFFECT: i32 = 130;

/// `ClientboundUpdateMobEffectPacket` flag bits.
pub const EFFECT_FLAG_AMBIENT: u8 = 1;
pub const EFFECT_FLAG_VISIBLE: u8 = 2;
pub const EFFECT_FLAG_SHOW_ICON: u8 = 4;
pub const EFFECT_FLAG_BLEND: u8 = 8;

/// The status effects the server models explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StatusEffectKind {
    Speed,
    Slowness,
    Haste,
    MiningFatigue,
    Strength,
    InstantHealth,
    InstantDamage,
    JumpBoost,
    Nausea,
    Regeneration,
    Resistance,
    FireResistance,
    WaterBreathing,
    Invisibility,
    Blindness,
    NightVision,
    Hunger,
    Weakness,
    Poison,
    Wither,
    HealthBoost,
    Absorption,
    Saturation,
    Glowing,
    Levitation,
    SlowFalling,
    Darkness,
    Luck,
    Unluck,
    ConduitPower,
    DolphinsGrace,
    BadOmen,
    HeroOfTheVillage,
    TrialOmen,
    RaidOmen,
    WindCharged,
    Weaving,
    Oozing,
    Infested,
    BreathOfTheNautilus,
}

impl StatusEffectKind {
    /// Every effect in `MobEffects` registration order.
    pub const ALL: [Self; 40] = [
        Self::Speed,
        Self::Slowness,
        Self::Haste,
        Self::MiningFatigue,
        Self::Strength,
        Self::InstantHealth,
        Self::InstantDamage,
        Self::JumpBoost,
        Self::Nausea,
        Self::Regeneration,
        Self::Resistance,
        Self::FireResistance,
        Self::WaterBreathing,
        Self::Invisibility,
        Self::Blindness,
        Self::NightVision,
        Self::Hunger,
        Self::Weakness,
        Self::Poison,
        Self::Wither,
        Self::HealthBoost,
        Self::Absorption,
        Self::Saturation,
        Self::Glowing,
        Self::Levitation,
        Self::Luck,
        Self::Unluck,
        Self::SlowFalling,
        Self::ConduitPower,
        Self::DolphinsGrace,
        Self::BadOmen,
        Self::HeroOfTheVillage,
        Self::Darkness,
        Self::TrialOmen,
        Self::RaidOmen,
        Self::WindCharged,
        Self::Weaving,
        Self::Oozing,
        Self::Infested,
        Self::BreathOfTheNautilus,
    ];

    /// Whether this effect applies its instant payload when added.
    pub fn is_instant(self) -> bool {
        matches!(self, Self::InstantHealth | Self::InstantDamage)
    }

    /// Whether the effect damages the entity on its periodic tick.
    pub fn is_harmful_tick(self) -> bool {
        matches!(self, Self::Poison | Self::Wither)
    }

    /// The vanilla `MobEffect` category used by `isBeneficial`.
    pub fn is_beneficial(self) -> bool {
        matches!(
            self,
            Self::Speed
                | Self::Haste
                | Self::Strength
                | Self::InstantHealth
                | Self::JumpBoost
                | Self::Regeneration
                | Self::Resistance
                | Self::FireResistance
                | Self::WaterBreathing
                | Self::Invisibility
                | Self::NightVision
                | Self::HealthBoost
                | Self::Absorption
                | Self::Saturation
                | Self::SlowFalling
                | Self::Luck
                | Self::ConduitPower
                | Self::DolphinsGrace
                | Self::HeroOfTheVillage
                | Self::BreathOfTheNautilus
        )
    }

    /// `BuiltInRegistries.MOB_EFFECT` id from the 1.21.11 `MobEffects`
    /// registration order. These ids are written directly by
    /// `MobEffect.STREAM_CODEC` (`ByteBufCodecs.holderRegistry`).
    pub const fn registry_id(self) -> i32 {
        match self {
            Self::Speed => 0,
            Self::Slowness => 1,
            Self::Haste => 2,
            Self::MiningFatigue => 3,
            Self::Strength => 4,
            Self::InstantHealth => 5,
            Self::InstantDamage => 6,
            Self::JumpBoost => 7,
            Self::Nausea => 8,
            Self::Regeneration => 9,
            Self::Resistance => 10,
            Self::FireResistance => 11,
            Self::WaterBreathing => 12,
            Self::Invisibility => 13,
            Self::Blindness => 14,
            Self::NightVision => 15,
            Self::Hunger => 16,
            Self::Weakness => 17,
            Self::Poison => 18,
            Self::Wither => 19,
            Self::HealthBoost => 20,
            Self::Absorption => 21,
            Self::Saturation => 22,
            Self::Glowing => 23,
            Self::Levitation => 24,
            Self::Luck => 25,
            Self::Unluck => 26,
            Self::SlowFalling => 27,
            Self::ConduitPower => 28,
            Self::DolphinsGrace => 29,
            Self::BadOmen => 30,
            Self::HeroOfTheVillage => 31,
            Self::Darkness => 32,
            Self::TrialOmen => 33,
            Self::RaidOmen => 34,
            Self::WindCharged => 35,
            Self::Weaving => 36,
            Self::Oozing => 37,
            Self::Infested => 38,
            Self::BreathOfTheNautilus => 39,
        }
    }

    /// Resolve an effect registry id. Unknown ids return `None`; packet
    /// decoders turn that into an explicit invalid-data error instead of
    /// guessing an effect.
    pub fn from_registry_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::Speed,
            1 => Self::Slowness,
            2 => Self::Haste,
            3 => Self::MiningFatigue,
            4 => Self::Strength,
            5 => Self::InstantHealth,
            6 => Self::InstantDamage,
            7 => Self::JumpBoost,
            8 => Self::Nausea,
            9 => Self::Regeneration,
            10 => Self::Resistance,
            11 => Self::FireResistance,
            12 => Self::WaterBreathing,
            13 => Self::Invisibility,
            14 => Self::Blindness,
            15 => Self::NightVision,
            16 => Self::Hunger,
            17 => Self::Weakness,
            18 => Self::Poison,
            19 => Self::Wither,
            20 => Self::HealthBoost,
            21 => Self::Absorption,
            22 => Self::Saturation,
            23 => Self::Glowing,
            24 => Self::Levitation,
            25 => Self::Luck,
            26 => Self::Unluck,
            27 => Self::SlowFalling,
            28 => Self::ConduitPower,
            29 => Self::DolphinsGrace,
            30 => Self::BadOmen,
            31 => Self::HeroOfTheVillage,
            32 => Self::Darkness,
            33 => Self::TrialOmen,
            34 => Self::RaidOmen,
            35 => Self::WindCharged,
            36 => Self::Weaving,
            37 => Self::Oozing,
            38 => Self::Infested,
            39 => Self::BreathOfTheNautilus,
            _ => return None,
        })
    }

    /// Resolve a vanilla `MobEffect` registry id. Both `minecraft:speed` and
    /// `speed` are accepted, matching [`crate::item_components::normalize_key`].
    pub fn from_id(id: &str) -> Option<Self> {
        let kind = match id.strip_prefix("minecraft:").unwrap_or(id) {
            "speed" => Self::Speed,
            "slowness" => Self::Slowness,
            "haste" => Self::Haste,
            "mining_fatigue" => Self::MiningFatigue,
            "strength" => Self::Strength,
            "instant_health" => Self::InstantHealth,
            "instant_damage" => Self::InstantDamage,
            "jump_boost" => Self::JumpBoost,
            "nausea" => Self::Nausea,
            "regeneration" => Self::Regeneration,
            "resistance" => Self::Resistance,
            "fire_resistance" => Self::FireResistance,
            "water_breathing" => Self::WaterBreathing,
            "invisibility" => Self::Invisibility,
            "blindness" => Self::Blindness,
            "night_vision" => Self::NightVision,
            "hunger" => Self::Hunger,
            "weakness" => Self::Weakness,
            "poison" => Self::Poison,
            "wither" => Self::Wither,
            "health_boost" => Self::HealthBoost,
            "absorption" => Self::Absorption,
            "saturation" => Self::Saturation,
            "glowing" => Self::Glowing,
            "levitation" => Self::Levitation,
            "luck" => Self::Luck,
            "unluck" => Self::Unluck,
            "slow_falling" => Self::SlowFalling,
            "conduit_power" => Self::ConduitPower,
            "dolphins_grace" => Self::DolphinsGrace,
            "bad_omen" => Self::BadOmen,
            "hero_of_the_village" => Self::HeroOfTheVillage,
            "darkness" => Self::Darkness,
            "trial_omen" => Self::TrialOmen,
            "raid_omen" => Self::RaidOmen,
            "wind_charged" => Self::WindCharged,
            "weaving" => Self::Weaving,
            "oozing" => Self::Oozing,
            "infested" => Self::Infested,
            "breath_of_the_nautilus" => Self::BreathOfTheNautilus,
            _ => return None,
        };
        Some(kind)
    }
}

/// One active effect instance (`MobEffectInstance`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEffect {
    pub kind: StatusEffectKind,
    /// Zero-based amplifier, matching `MobEffectInstance.getAmplifier`.
    pub amplifier: u8,
    pub duration: i32,
    pub ambient: bool,
    pub visible: bool,
    /// `MobEffectInstance.showIcon()`, serialized as bit 2 of the update
    /// packet flags.
    pub show_icon: bool,
}

impl StatusEffect {
    pub const fn new(kind: StatusEffectKind, duration: i32, amplifier: u8) -> Self {
        Self {
            kind,
            amplifier,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }

    /// Set the three client-visible flags at once (`ambient`, `visible`,
    /// `show_icon`).
    pub const fn with_flags(mut self, ambient: bool, visible: bool, show_icon: bool) -> Self {
        self.ambient = ambient;
        self.visible = visible;
        self.show_icon = show_icon;
        self
    }

    /// Flags byte used by `ClientboundUpdateMobEffectPacket`. The blend bit is
    /// supplied per packet because it is not stored on the instance.
    pub const fn flags(&self) -> u8 {
        let mut flags = 0;
        if self.ambient {
            flags |= EFFECT_FLAG_AMBIENT;
        }
        if self.visible {
            flags |= EFFECT_FLAG_VISIBLE;
        }
        if self.show_icon {
            flags |= EFFECT_FLAG_SHOW_ICON;
        }
        flags
    }

    /// Convert a `minecraft:custom_effects` entry into the runtime effect. The
    /// component id uses the registry name (`minecraft:speed`), while the
    /// runtime enum is typed.
    pub fn from_component(effect: &MobEffectInstance) -> Option<Self> {
        Some(Self {
            kind: StatusEffectKind::from_id(&effect.id)?,
            amplifier: u8::try_from(effect.amplifier.max(0)).unwrap_or(u8::MAX),
            duration: effect.duration,
            ambient: effect.ambient,
            visible: effect.show_particles,
            show_icon: effect.show_icon,
        })
    }

    /// `MobEffectInstance.update`: adopt a stronger amplifier or, at equal
    /// strength, the longer duration. Flag changes are copied regardless, and
    /// a new non-ambient instance clears the ambient bit. Returns true when any
    /// field changed.
    pub fn merge(&mut self, other: &StatusEffect) -> bool {
        let mut changed = false;
        let other_infinite = other.duration == -1;
        let self_infinite = self.duration == -1;
        if other.amplifier > self.amplifier {
            self.amplifier = other.amplifier;
            self.duration = other.duration;
            changed = true;
        } else if other.amplifier == self.amplifier
            && (other_infinite && !self_infinite || other.duration > self.duration)
        {
            self.duration = other.duration;
            changed = true;
        }
        if !other.ambient && self.ambient {
            self.ambient = false;
            changed = true;
        }
        if other.visible != self.visible {
            self.visible = other.visible;
            changed = true;
        }
        if other.show_icon != self.show_icon {
            self.show_icon = other.show_icon;
            changed = true;
        }
        changed
    }

    /// `LivingEntity.forceAddEffect`: replace every field without merging.
    pub fn replace_with(&mut self, other: &StatusEffect) -> bool {
        if self == other {
            return false;
        }
        *self = other.clone();
        true
    }
}

/// Result of adding one effect to a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectUpdate {
    /// A new active effect was inserted.
    Added,
    /// The existing effect was refreshed or had its flags changed.
    Updated,
    /// Nothing changed (weaker and shorter, identical, or zero duration).
    Unchanged,
    /// Instant health/damage was applied immediately and never stored.
    Instant,
}

/// Observable result of [`LivingEntity::apply_effect`].
#[derive(Debug, Clone, PartialEq)]
pub struct EffectApplyResult {
    pub update: EffectUpdate,
    /// Ready-to-send update packets. Instant effects emit health metadata
    /// packets instead of a `ClientboundUpdateMobEffect` packet.
    pub packets: Vec<Vec<u8>>,
}

/// What happened during one effect tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectTick {
    None,
    Damage { amount: i32, kind: StatusEffectKind },
    Heal { amount: i32, kind: StatusEffectKind },
    Expired(StatusEffectKind),
}

/// Active effects on one entity, in add order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusEffectSet {
    effects: Vec<StatusEffect>,
}

impl StatusEffectSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = &StatusEffect> {
        self.effects.iter()
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn get(&self, kind: StatusEffectKind) -> Option<&StatusEffect> {
        self.effects.iter().find(|effect| effect.kind == kind)
    }

    pub fn has(&self, kind: StatusEffectKind) -> bool {
        self.get(kind).is_some()
    }

    pub fn amplifier(&self, kind: StatusEffectKind) -> Option<u8> {
        self.get(kind).map(|effect| effect.amplifier)
    }

    /// Adds or refreshes an effect, following `MobEffectInstance.update`.
    /// Instant effects are reported as [`EffectUpdate::Instant`] and never
    /// enter the active list (vanilla applies them immediately and does not
    /// store them).
    pub fn apply(&mut self, effect: StatusEffect) -> EffectUpdate {
        if effect.kind.is_instant() {
            return EffectUpdate::Instant;
        }
        if effect.duration == 0 || effect.duration < -1 {
            return EffectUpdate::Unchanged;
        }
        if let Some(existing) = self
            .effects
            .iter_mut()
            .find(|existing| existing.kind == effect.kind)
        {
            return if existing.merge(&effect) {
                EffectUpdate::Updated
            } else {
                EffectUpdate::Unchanged
            };
        }
        self.effects.push(effect);
        EffectUpdate::Added
    }

    /// `LivingEntity.forceAddEffect`: replace an existing instance instead of
    /// merging. A new kind is inserted normally.
    pub fn replace(&mut self, effect: StatusEffect) -> EffectUpdate {
        if effect.kind.is_instant() {
            return EffectUpdate::Instant;
        }
        if effect.duration == 0 || effect.duration < -1 {
            return EffectUpdate::Unchanged;
        }
        if let Some(existing) = self
            .effects
            .iter_mut()
            .find(|existing| existing.kind == effect.kind)
        {
            return if existing.replace_with(&effect) {
                EffectUpdate::Updated
            } else {
                EffectUpdate::Unchanged
            };
        }
        self.effects.push(effect);
        EffectUpdate::Added
    }

    /// Backwards-compatible add used by potion consumption callers: returns
    /// the instant payload marker exactly once.
    pub fn add(&mut self, effect: StatusEffect) -> Option<EffectTick> {
        match self.apply(effect) {
            EffectUpdate::Instant => Some(EffectTick::None),
            _ => None,
        }
    }

    pub fn remove(&mut self, kind: StatusEffectKind) -> bool {
        let before = self.effects.len();
        self.effects.retain(|effect| effect.kind != kind);
        before != self.effects.len()
    }

    pub fn clear(&mut self) {
        self.effects.clear();
    }

    /// Ticks all effects. `entity_tick` is the absolute tick counter; tests and
    /// callers pass the entity's own age. Instant effects never last here.
    ///
    /// Vanilla computes poison/wither/regeneration intervals as
    /// `25 >> amplifier`, `40 >> amplifier`, and `50 >> amplifier`, clamped to
    /// at least one tick.
    pub fn tick(&mut self, entity_tick: u64) -> Vec<EffectTick> {
        let mut events = Vec::new();
        let mut kept = Vec::with_capacity(self.effects.len());
        for mut effect in std::mem::take(&mut self.effects) {
            if effect.kind.is_instant() {
                continue;
            }
            if effect.duration != -1 {
                effect.duration -= 1;
                if effect.duration <= 0 {
                    events.push(EffectTick::Expired(effect.kind));
                    continue;
                }
            }
            let interval = match effect.kind {
                StatusEffectKind::Poison => (25u64 >> effect.amplifier.min(7)).max(1),
                StatusEffectKind::Wither => (40u64 >> effect.amplifier.min(7)).max(1),
                StatusEffectKind::Regeneration => (50u64 >> effect.amplifier.min(7)).max(1),
                _ => 0,
            };
            if interval > 0 && entity_tick.is_multiple_of(interval) {
                match effect.kind {
                    StatusEffectKind::Poison | StatusEffectKind::Wither => {
                        events.push(EffectTick::Damage {
                            amount: 1,
                            kind: effect.kind,
                        });
                    }
                    StatusEffectKind::Regeneration => {
                        events.push(EffectTick::Heal {
                            amount: 1,
                            kind: effect.kind,
                        });
                    }
                    _ => {}
                }
            }
            kept.push(effect);
        }
        self.effects = kept;
        events
    }

    /// `LivingEntity.getSpeed` multiplier. Speed is +20% and slowness -15% per
    /// level, clamped at zero.
    pub fn movement_multiplier(&self) -> f32 {
        let speed = self
            .amplifier(StatusEffectKind::Speed)
            .map_or(0.0, |level| 0.2 * f32::from(level) + 0.2);
        let slow = self
            .amplifier(StatusEffectKind::Slowness)
            .map_or(0.0, |level| 0.15 * f32::from(level) + 0.15);
        (1.0 + speed - slow).max(0.0)
    }

    /// `LivingEntity.getDamageAfterMagicAbsorb` helper for Resistance.
    pub fn resistance_multiplier(&self) -> f32 {
        let level = self
            .amplifier(StatusEffectKind::Resistance)
            .map_or(0, |level| i32::from(level) + 1);
        (1.0 - 0.2 * level as f32).max(0.0)
    }

    /// Attack damage modifier used by `LivingEntity.getDamage`.
    pub fn attack_damage_modifier(&self) -> f32 {
        let strength = self
            .amplifier(StatusEffectKind::Strength)
            .map_or(0.0, |level| 3.0 * (f32::from(level) + 1.0));
        let weakness = self
            .amplifier(StatusEffectKind::Weakness)
            .map_or(0.0, |level| 4.0 * (f32::from(level) + 1.0));
        strength - weakness
    }

    pub fn jump_boost(&self) -> u8 {
        self.amplifier(StatusEffectKind::JumpBoost)
            .map_or(0, |level| level + 1)
    }

    pub fn has_fire_resistance(&self) -> bool {
        self.has(StatusEffectKind::FireResistance)
    }

    /// One update packet per active effect, in add order. Used when an entity
    /// enters a viewer's tracking range (`PlayerList.sendActiveEffects`).
    pub fn update_packets(&self, entity_id: i32) -> Vec<Vec<u8>> {
        self.effects
            .iter()
            .map(|effect| update_mob_effect_packet(entity_id, effect, false))
            .collect()
    }
}

/// `ClientboundUpdateMobEffectPacket` body (packet id included).
pub fn update_mob_effect_packet(entity_id: i32, effect: &StatusEffect, blend: bool) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_UPDATE_MOB_EFFECT)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(entity_id)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(effect.kind.registry_id())
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(i32::from(effect.amplifier))
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(effect.duration)
        .expect("writing to Vec cannot fail");
    let flags = effect.flags() | if blend { EFFECT_FLAG_BLEND } else { 0 };
    writer.write_u8(flags).expect("writing to Vec cannot fail");
    writer.into_inner()
}

/// Decode a complete `ClientboundUpdateMobEffectPacket`. Unknown registry ids
/// are an error so a packet from a different protocol version cannot be
/// silently misinterpreted.
pub fn read_update_mob_effect(packet: &[u8]) -> io::Result<(i32, StatusEffect, bool)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_UPDATE_MOB_EFFECT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "expected update_mob_effect id {CLIENTBOUND_UPDATE_MOB_EFFECT}, got {packet_id}"
            ),
        ));
    }
    let entity_id = reader.read_varint()?;
    let effect_id = reader.read_varint()?;
    let kind = StatusEffectKind::from_registry_id(effect_id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown mob effect registry id {effect_id}"),
        )
    })?;
    let amplifier = reader.read_varint()?;
    let amplifier = u8::try_from(amplifier).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("mob effect amplifier {amplifier} is out of range"),
        )
    })?;
    let duration = reader.read_varint()?;
    let flags = reader.read_u8()?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after update_mob_effect",
        ));
    }
    Ok((
        entity_id,
        StatusEffect {
            kind,
            amplifier,
            duration,
            ambient: flags & EFFECT_FLAG_AMBIENT != 0,
            visible: flags & EFFECT_FLAG_VISIBLE != 0,
            show_icon: flags & EFFECT_FLAG_SHOW_ICON != 0,
        },
        flags & EFFECT_FLAG_BLEND != 0,
    ))
}

/// `ClientboundRemoveMobEffectPacket` body (packet id included).
pub fn remove_mob_effect_packet(entity_id: i32, kind: StatusEffectKind) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_REMOVE_MOB_EFFECT)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(entity_id)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(kind.registry_id())
        .expect("writing to Vec cannot fail");
    writer.into_inner()
}

pub fn read_remove_mob_effect(packet: &[u8]) -> io::Result<(i32, StatusEffectKind)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_REMOVE_MOB_EFFECT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "expected remove_mob_effect id {CLIENTBOUND_REMOVE_MOB_EFFECT}, got {packet_id}"
            ),
        ));
    }
    let entity_id = reader.read_varint()?;
    let effect_id = reader.read_varint()?;
    let kind = StatusEffectKind::from_registry_id(effect_id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown mob effect registry id {effect_id}"),
        )
    })?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after remove_mob_effect",
        ));
    }
    Ok((entity_id, kind))
}

/// A named potion's base effect list, matching `net.minecraft.world.item.alchemy.Potions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PotionDefinition {
    pub potion: &'static str,
    pub effects: &'static [StatusEffect],
}

const fn potion_effect(kind: StatusEffectKind, duration: i32, amplifier: u8) -> StatusEffect {
    StatusEffect::new(kind, duration, amplifier)
}

const NO_EFFECTS: &[StatusEffect] = &[];

/// The 1.21.11 potion registry effect table.
pub const POTION_DEFINITIONS: &[PotionDefinition] = &[
    PotionDefinition {
        potion: "minecraft:water",
        effects: NO_EFFECTS,
    },
    PotionDefinition {
        potion: "minecraft:mundane",
        effects: NO_EFFECTS,
    },
    PotionDefinition {
        potion: "minecraft:thick",
        effects: NO_EFFECTS,
    },
    PotionDefinition {
        potion: "minecraft:awkward",
        effects: NO_EFFECTS,
    },
    PotionDefinition {
        potion: "minecraft:night_vision",
        effects: &[potion_effect(StatusEffectKind::NightVision, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_night_vision",
        effects: &[potion_effect(StatusEffectKind::NightVision, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:invisibility",
        effects: &[potion_effect(StatusEffectKind::Invisibility, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_invisibility",
        effects: &[potion_effect(StatusEffectKind::Invisibility, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:leaping",
        effects: &[potion_effect(StatusEffectKind::JumpBoost, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_leaping",
        effects: &[potion_effect(StatusEffectKind::JumpBoost, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_leaping",
        effects: &[potion_effect(StatusEffectKind::JumpBoost, 1800, 1)],
    },
    PotionDefinition {
        potion: "minecraft:fire_resistance",
        effects: &[potion_effect(StatusEffectKind::FireResistance, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_fire_resistance",
        effects: &[potion_effect(StatusEffectKind::FireResistance, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:swiftness",
        effects: &[potion_effect(StatusEffectKind::Speed, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_swiftness",
        effects: &[potion_effect(StatusEffectKind::Speed, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_swiftness",
        effects: &[potion_effect(StatusEffectKind::Speed, 1800, 1)],
    },
    PotionDefinition {
        potion: "minecraft:slowness",
        effects: &[potion_effect(StatusEffectKind::Slowness, 1800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_slowness",
        effects: &[potion_effect(StatusEffectKind::Slowness, 4800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_slowness",
        effects: &[potion_effect(StatusEffectKind::Slowness, 400, 3)],
    },
    PotionDefinition {
        potion: "minecraft:turtle_master",
        effects: &[
            potion_effect(StatusEffectKind::Slowness, 400, 3),
            potion_effect(StatusEffectKind::Resistance, 400, 2),
        ],
    },
    PotionDefinition {
        potion: "minecraft:long_turtle_master",
        effects: &[
            potion_effect(StatusEffectKind::Slowness, 800, 3),
            potion_effect(StatusEffectKind::Resistance, 800, 2),
        ],
    },
    PotionDefinition {
        potion: "minecraft:strong_turtle_master",
        effects: &[
            potion_effect(StatusEffectKind::Slowness, 400, 5),
            potion_effect(StatusEffectKind::Resistance, 400, 3),
        ],
    },
    PotionDefinition {
        potion: "minecraft:water_breathing",
        effects: &[potion_effect(StatusEffectKind::WaterBreathing, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_water_breathing",
        effects: &[potion_effect(StatusEffectKind::WaterBreathing, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:healing",
        effects: &[potion_effect(StatusEffectKind::InstantHealth, 1, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_healing",
        effects: &[potion_effect(StatusEffectKind::InstantHealth, 1, 1)],
    },
    PotionDefinition {
        potion: "minecraft:harming",
        effects: &[potion_effect(StatusEffectKind::InstantDamage, 1, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_harming",
        effects: &[potion_effect(StatusEffectKind::InstantDamage, 1, 1)],
    },
    PotionDefinition {
        potion: "minecraft:poison",
        effects: &[potion_effect(StatusEffectKind::Poison, 900, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_poison",
        effects: &[potion_effect(StatusEffectKind::Poison, 1800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_poison",
        effects: &[potion_effect(StatusEffectKind::Poison, 432, 1)],
    },
    PotionDefinition {
        potion: "minecraft:regeneration",
        effects: &[potion_effect(StatusEffectKind::Regeneration, 900, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_regeneration",
        effects: &[potion_effect(StatusEffectKind::Regeneration, 1800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_regeneration",
        effects: &[potion_effect(StatusEffectKind::Regeneration, 450, 1)],
    },
    PotionDefinition {
        potion: "minecraft:strength",
        effects: &[potion_effect(StatusEffectKind::Strength, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_strength",
        effects: &[potion_effect(StatusEffectKind::Strength, 9600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:strong_strength",
        effects: &[potion_effect(StatusEffectKind::Strength, 1800, 1)],
    },
    PotionDefinition {
        potion: "minecraft:weakness",
        effects: &[potion_effect(StatusEffectKind::Weakness, 1800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_weakness",
        effects: &[potion_effect(StatusEffectKind::Weakness, 4800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:luck",
        effects: &[potion_effect(StatusEffectKind::Luck, 6000, 0)],
    },
    PotionDefinition {
        potion: "minecraft:slow_falling",
        effects: &[potion_effect(StatusEffectKind::SlowFalling, 1800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:long_slow_falling",
        effects: &[potion_effect(StatusEffectKind::SlowFalling, 4800, 0)],
    },
    PotionDefinition {
        potion: "minecraft:wind_charged",
        effects: &[potion_effect(StatusEffectKind::WindCharged, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:weaving",
        effects: &[potion_effect(StatusEffectKind::Weaving, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:oozing",
        effects: &[potion_effect(StatusEffectKind::Oozing, 3600, 0)],
    },
    PotionDefinition {
        potion: "minecraft:infested",
        effects: &[potion_effect(StatusEffectKind::Infested, 3600, 0)],
    },
];

/// Base effects for a potion id. Unknown ids return `None`; the four base
/// potions with no effects return an empty slice.
pub fn potion_effects(potion: &str) -> Option<&'static [StatusEffect]> {
    let potion = normalize_key(potion);
    POTION_DEFINITIONS
        .iter()
        .find(|definition| definition.potion == potion)
        .map(|definition| definition.effects)
}

/// Resolve a `PotionContents` component to runtime effects: the base potion's
/// effects followed by the component's custom effects, in Java order.
pub fn potion_contents_effects(contents: &PotionContents) -> Vec<StatusEffect> {
    let mut effects = Vec::new();
    if let Some(potion) = &contents.potion
        && let Some(base) = potion_effects(potion)
    {
        effects.extend_from_slice(base);
    }
    effects.extend(
        contents
            .custom_effects
            .iter()
            .filter_map(StatusEffect::from_component),
    );
    effects
}

/// Apply a potion component to an entity's active effects, returning instant
/// payload events for the caller to apply to health.
pub fn apply_potion_contents(
    effects: &mut StatusEffectSet,
    contents: &PotionContents,
) -> Vec<EffectTick> {
    let mut events = Vec::new();
    for effect in potion_contents_effects(contents) {
        if let Some(event) = effects.add(effect) {
            events.push(event);
        }
    }
    events
}

/// Vanilla armor point values for the supported materials, per slot:
/// boots, leggings, chestplate, helmet.
fn armor_values(item: &str) -> Option<[i32; 4]> {
    let name = item.strip_prefix("minecraft:").unwrap_or(item);
    let material = if name.starts_with("leather_") {
        [1, 2, 3, 1]
    } else if name.starts_with("chainmail_") {
        [1, 4, 5, 2]
    } else if name.starts_with("iron_") {
        [2, 5, 6, 2]
    } else if name.starts_with("golden_") {
        [1, 3, 5, 2]
    } else if name.starts_with("diamond_") || name.starts_with("netherite_") {
        [3, 6, 8, 3]
    } else if name.starts_with("turtle_") {
        [0, 0, 0, 2]
    } else {
        return None;
    };
    Some(material)
}

fn armor_slot_index(slot: EquipmentSlot) -> Option<usize> {
    match slot {
        EquipmentSlot::Feet => Some(0),
        EquipmentSlot::Legs => Some(1),
        EquipmentSlot::Chest | EquipmentSlot::Body => Some(2),
        EquipmentSlot::Head => Some(3),
        _ => None,
    }
}

/// Armor points and toughness from the four armor slots.
pub fn armor_values_from_equipment(equipment: &Equipment) -> (i32, i32) {
    let mut points = 0;
    let mut toughness = 0;
    for slot in [
        EquipmentSlot::Feet,
        EquipmentSlot::Legs,
        EquipmentSlot::Chest,
        EquipmentSlot::Head,
    ] {
        let stack: &ItemStack = equipment.get(slot);
        if stack.is_empty() {
            continue;
        }
        let Some(index) = armor_slot_index(slot) else {
            continue;
        };
        let Some(values) = armor_values(&stack.item) else {
            continue;
        };
        points += values[index];
        if stack.item.contains("diamond_") || stack.item.contains("netherite_") {
            toughness += if stack.item.contains("netherite_") {
                3
            } else {
                2
            };
        }
    }
    (points, toughness)
}

/// `CombatRules.getDamageAfterAbsorb` using the vanilla armor/toughness curve.
pub fn damage_after_armor(amount: f32, armor: i32, toughness: i32) -> f32 {
    if amount <= 0.0 {
        return 0.0;
    }
    let f = 2.0 + toughness as f32 / 4.0;
    let g = (armor as f32 - amount / f).clamp(armor as f32 * 0.2, 20.0);
    amount * (1.0 - g / 25.0)
}

/// Damage categories used by the live player-damage path for enchantment
/// protection. `Void` bypasses enchantment protection; armor bypass is handled
/// separately by the caller in this slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DamageKind {
    Physical,
    Projectile,
    Fire,
    Explosion,
    Fall,
    Void,
}

/// Raw enchantment protection points from the four armor slots for `kind`.
/// Includes generic `minecraft:protection` for all non-void kinds plus the
/// type-specific enchantment; values are summed across pieces.
pub fn enchantment_damage_protection(equipment: &Equipment, kind: DamageKind) -> i32 {
    if kind == DamageKind::Void {
        return 0;
    }
    let mut total = 0;
    for slot in [
        EquipmentSlot::Feet,
        EquipmentSlot::Legs,
        EquipmentSlot::Chest,
        EquipmentSlot::Head,
    ] {
        let stack: &ItemStack = equipment.get(slot);
        if stack.is_empty() {
            continue;
        }
        total += stack.enchantment_level("minecraft:protection");
        total += match kind {
            DamageKind::Projectile => stack.enchantment_level("minecraft:projectile_protection"),
            DamageKind::Fire => stack.enchantment_level("minecraft:fire_protection"),
            DamageKind::Explosion => stack.enchantment_level("minecraft:blast_protection"),
            DamageKind::Fall => stack.enchantment_level("minecraft:feather_falling"),
            DamageKind::Physical | DamageKind::Void => 0,
        };
    }
    total
}

/// Vanilla `CombatRules.getDamageAfterMagicAbsorb` for enchantment protection.
pub fn damage_after_enchantments(amount: f32, protection: i32) -> f32 {
    if protection <= 0 {
        amount
    } else {
        amount * (1.0 - protection.clamp(0, 20) as f32 / 25.0)
    }
}

/// Damage categories with the vanilla bypass flags this slice models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageType {
    Generic,
    MobAttack,
    PlayerAttack,
    Fall,
    Fire,
    Lava,
    Drowning,
    Starvation,
    Poison,
    Wither,
    Magic,
    Explosion,
    Void,
    Freeze,
}

impl DamageType {
    pub fn bypasses_armor(self) -> bool {
        matches!(
            self,
            Self::Drowning
                | Self::Starvation
                | Self::Poison
                | Self::Wither
                | Self::Magic
                | Self::Void
                | Self::Freeze
        )
    }

    pub fn bypasses_invulnerability(self) -> bool {
        matches!(self, Self::Void)
    }

    pub fn is_fire(self) -> bool {
        matches!(self, Self::Fire | Self::Lava)
    }
}

/// Outcome of the armor/effect damage pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageOutcome {
    pub raw: f32,
    pub after_armor: f32,
    pub after_resistance: f32,
    pub armor_points: i32,
    pub armor_toughness: i32,
    pub hurt: HurtResult,
}

/// Applies armor, resistance, then the living-entity hurt rules.
pub fn apply_damage(
    entity: &mut LivingEntity,
    amount: f32,
    damage_type: DamageType,
    effects: &StatusEffectSet,
) -> DamageOutcome {
    let (armor_points, armor_toughness) = if damage_type.bypasses_armor() {
        (0, 0)
    } else {
        armor_values_from_equipment(&entity.equipment)
    };
    let after_armor = damage_after_armor(amount, armor_points, armor_toughness);
    let after_resistance = after_armor * effects.resistance_multiplier();
    let hurt = entity.hurt_with(after_resistance, damage_type.bypasses_invulnerability());
    DamageOutcome {
        raw: amount,
        after_armor,
        after_resistance,
        armor_points,
        armor_toughness,
        hurt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_curve_matches_java() {
        assert_eq!(damage_after_armor(10.0, 0, 0), 10.0);
        assert!((damage_after_armor(10.0, 15, 0) - 6.0).abs() < 1e-5);
        assert!((damage_after_armor(10.0, 20, 8) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn effect_intervals_and_modifiers() {
        let mut set = StatusEffectSet::new();
        set.add(StatusEffect::new(StatusEffectKind::Poison, 100, 0));
        set.add(StatusEffect::new(StatusEffectKind::Speed, 100, 1));
        assert!((set.movement_multiplier() - 1.4).abs() < 1e-5);
        assert_eq!(
            set.tick(25),
            vec![EffectTick::Damage {
                amount: 1,
                kind: StatusEffectKind::Poison
            }]
        );
        assert_eq!(set.tick(26), vec![]);
    }

    #[test]
    fn enchantment_protection_math_and_clamping() {
        assert_eq!(damage_after_enchantments(10.0, 0), 10.0);
        assert!((damage_after_enchantments(10.0, 10) - 6.0).abs() < 1e-5);
        assert!((damage_after_enchantments(10.0, 20) - 2.0).abs() < 1e-5);
        assert!((damage_after_enchantments(10.0, 25) - 2.0).abs() < 1e-5);
        assert_eq!(damage_after_enchantments(10.0, -1), 10.0);
    }

    #[test]
    fn enchantment_protection_selects_kind_specific_levels() {
        use crate::item_components::ComponentValue;
        use std::collections::BTreeMap;

        let mut equipment = Equipment::new();
        let mut chest = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
        chest.set_component(ComponentValue::Enchantments(BTreeMap::from([(
            "minecraft:protection".to_string(),
            2,
        )])));
        *equipment.get_mut(EquipmentSlot::Chest) = chest;
        assert_eq!(
            enchantment_damage_protection(&equipment, DamageKind::Physical),
            2
        );
        assert_eq!(
            enchantment_damage_protection(&equipment, DamageKind::Void),
            0
        );

        for (kind, name) in [
            (DamageKind::Projectile, "minecraft:projectile_protection"),
            (DamageKind::Fire, "minecraft:fire_protection"),
            (DamageKind::Explosion, "minecraft:blast_protection"),
            (DamageKind::Fall, "minecraft:feather_falling"),
        ] {
            let mut equipment = Equipment::new();
            let mut chest = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
            chest.set_component(ComponentValue::Enchantments(BTreeMap::from([(
                name.to_string(),
                4,
            )])));
            *equipment.get_mut(EquipmentSlot::Chest) = chest;
            assert_eq!(enchantment_damage_protection(&equipment, kind), 4);
            assert_eq!(
                enchantment_damage_protection(&equipment, DamageKind::Physical),
                0
            );
        }
    }
}
