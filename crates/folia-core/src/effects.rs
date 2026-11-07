//! Status effects, armor calculation, and typed damage.
//!
//! This is the P5 damage slice: it models the subset of
//! `net/minecraft/world/effect/MobEffectInstance`, `LivingEntity` armor
//! reduction, and vanilla `DamageType` flags that the server already tracks.
//! Effects are stored beside a [`LivingEntity`] by callers; this module owns
//! the tick math and the damage pipeline, not entity storage.

use crate::entity_metadata::{Equipment, EquipmentSlot, HurtResult, LivingEntity};
use crate::item_components::ItemStack;

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
        )
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
    pub fn new(kind: StatusEffectKind, duration: i32, amplifier: u8) -> Self {
        Self {
            kind,
            amplifier,
            duration,
            ambient: false,
            visible: true,
        }
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
    /// so it can apply their payload exactly once.
    pub fn add(&mut self, effect: StatusEffect) -> Option<EffectTick> {
        if effect.duration <= 0 && !effect.kind.is_instant() {
            return None;
        }
        if let Some(existing) = self
            .effects
            .iter_mut()
            .find(|existing| existing.kind == effect.kind)
        {
            existing.merge(&effect);
            if effect.kind.is_instant() {
                return Some(EffectTick::None);
            }
            return None;
        }
        let kind = effect.kind;
        self.effects.push(effect);
        if kind.is_instant() {
            Some(EffectTick::None)
        } else {
            None
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
