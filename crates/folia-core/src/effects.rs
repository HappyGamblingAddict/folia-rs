//! Status effects, armor calculation, and typed damage.
//!
//! This is the P5 damage slice: it models the subset of
//! `net/minecraft/world/effect/MobEffectInstance`, `LivingEntity` armor
//! reduction, and vanilla `DamageType` flags that the server already tracks.
//! Effects are stored beside a [`LivingEntity`] by callers; this module owns
//! the tick math and the damage pipeline, not entity storage.

use crate::entity_metadata::{Equipment, EquipmentSlot, HurtResult, LivingEntity};
use crate::item_components::{ItemStack, MobEffectInstance, PotionContents, normalize_key};

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
}

impl StatusEffect {
    pub const fn new(kind: StatusEffectKind, duration: i32, amplifier: u8) -> Self {
        Self {
            kind,
            amplifier,
            duration,
            ambient: false,
            visible: true,
        }
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
        })
    }

    /// `MobEffectInstance.update`: refresh to the stronger/longer values and
    /// return whether the effect remains active.
    pub fn merge(&mut self, other: &StatusEffect) -> bool {
        if other.amplifier > self.amplifier
            || (other.amplifier == self.amplifier && other.duration > self.duration)
        {
            self.amplifier = other.amplifier;
            self.duration = other.duration;
            self.ambient = other.ambient;
            self.visible = other.visible;
        }
        self.duration > 0
    }
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

    pub fn get(&self, kind: StatusEffectKind) -> Option<&StatusEffect> {
        self.effects.iter().find(|effect| effect.kind == kind)
    }

    pub fn has(&self, kind: StatusEffectKind) -> bool {
        self.get(kind).is_some()
    }

    pub fn amplifier(&self, kind: StatusEffectKind) -> Option<u8> {
        self.get(kind).map(|effect| effect.amplifier)
    }

    /// Adds or refreshes an effect. Instant effects are returned to the caller
    /// so it can apply their payload exactly once; they never enter the active
    /// effect list (vanilla applies them immediately and does not store them).
    pub fn add(&mut self, effect: StatusEffect) -> Option<EffectTick> {
        if effect.kind.is_instant() {
            return Some(EffectTick::None);
        }
        if effect.duration <= 0 {
            return None;
        }
        if let Some(existing) = self
            .effects
            .iter_mut()
            .find(|existing| existing.kind == effect.kind)
        {
            existing.merge(&effect);
            return None;
        }
        self.effects.push(effect);
        None
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
            effect.duration -= 1;
            if effect.duration <= 0 {
                events.push(EffectTick::Expired(effect.kind));
                continue;
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
}
