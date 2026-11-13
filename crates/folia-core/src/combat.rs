//! Player attack cooldown and critical-hit helpers.

/// Player attack cooldown state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackCooldown {
    ticks_since_attack: i32,
    attack_speed: f64,
}

impl AttackCooldown {
    pub fn new(attack_speed: f64) -> Self {
        Self {
            ticks_since_attack: 0,
            attack_speed: Self::sanitized_speed(attack_speed),
        }
    }

    pub fn attack_speed(&self) -> f64 {
        self.attack_speed
    }

    pub fn set_attack_speed(&mut self, attack_speed: f64) {
        self.attack_speed = Self::sanitized_speed(attack_speed);
    }

    pub fn tick(&mut self) {
        let cooldown = self.cooldown_ticks();
        self.ticks_since_attack = self.ticks_since_attack.saturating_add(1).min(cooldown);
    }

    pub fn reset(&mut self) {
        self.ticks_since_attack = 0;
    }

    pub fn cooldown_ticks(&self) -> i32 {
        (20.0 / self.attack_speed).ceil() as i32
    }

    pub fn strength_scale(&self, partial_ticks: f32) -> f32 {
        let partial = partial_ticks.clamp(0.0, 1.0);
        let cooldown = self.cooldown_ticks() as f32;
        ((self.ticks_since_attack as f32 + partial) / cooldown).clamp(0.0, 1.0)
    }

    fn sanitized_speed(attack_speed: f64) -> f64 {
        if !attack_speed.is_finite() || attack_speed <= 0.0 {
            4.0
        } else {
            attack_speed
        }
    }
}

pub fn damage_scale(strength_scale: f32) -> f32 {
    0.2 + strength_scale.clamp(0.0, 1.0).powi(2) * 0.8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CriticalConditions {
    pub falling: bool,
    pub on_ground: bool,
    pub in_water: bool,
    pub on_climbable: bool,
    pub blind: bool,
    pub passenger: bool,
    pub sprinting: bool,
}

pub fn is_critical(strength_scale: f32, conditions: &CriticalConditions) -> bool {
    strength_scale > 0.9
        && conditions.falling
        && !conditions.on_ground
        && !conditions.in_water
        && !conditions.on_climbable
        && !conditions.blind
        && !conditions.passenger
        && !conditions.sprinting
}

pub const CRITICAL_MULTIPLIER: f32 = 1.5;

/// Computes final attack damage.
///
/// A non-finite or negative `base_damage` yields `0.0`.
pub fn attack_damage(base_damage: f32, strength_scale: f32, critical: bool) -> f32 {
    if !base_damage.is_finite() || base_damage < 0.0 {
        return 0.0;
    }
    let mut result = base_damage * damage_scale(strength_scale);
    if critical {
        result *= CRITICAL_MULTIPLIER;
    }
    result
}
