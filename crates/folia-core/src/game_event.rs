//! Game events and the vibration model consumed by sculk listeners.
//!
//! This is a deterministic *model*, not a Java parity claim. The local
//! checkout has no decompiled Folia 1.21.11 sources (see `BLOCKERS.md`), so the
//! frequency and range tables below are vanilla-style approximations. The
//! anchors treated as confidently vanilla are the `0..=15` frequency domain
//! and the ordering low (step/place) < medium (hits/death) < high (explosion).
//! Every other number is a chosen approximation, pinned by the unit tests so a
//! later Java oracle dump can replace the table in one place without silently
//! changing behavior.
//!
//! The process-wide event queue is an append-only staging area for a future
//! sensor tick. Nothing drains or dispatches it in this slice; the consumer
//! that lands later is responsible for reading [`recent_events`] and clearing
//! or rotating the queue, otherwise a long-running server will grow it without
//! bound.

use std::sync::{Mutex, OnceLock};

/// Kinds of game event that a sculk listener can react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameEventKind {
    Step,
    PlaceBlock,
    BreakBlock,
    Explosion,
    TntPrime,
    EntityHit,
    ProjectileHit,
    Ignite,
    EntityDeath,
    FluidPlace,
    FluidPickup,
}

/// Vibration frequency of an event, in vanilla's `0..=15` domain.
///
/// The values are approximations, not an oracle dump: `Step`, `PlaceBlock`,
/// `BreakBlock`, `Ignite`, and the fluid events sit in the LOW band (5),
/// `EntityHit`, `ProjectileHit`, and `EntityDeath` in the NORMAL band (10), and
/// the two explosion events in the HIGH band (15). Tests pin this exact table.
pub fn frequency(kind: GameEventKind) -> u8 {
    match kind {
        GameEventKind::Step
        | GameEventKind::PlaceBlock
        | GameEventKind::BreakBlock
        | GameEventKind::Ignite
        | GameEventKind::FluidPlace
        | GameEventKind::FluidPickup => 5,
        GameEventKind::EntityHit | GameEventKind::ProjectileHit | GameEventKind::EntityDeath => 10,
        GameEventKind::Explosion | GameEventKind::TntPrime => 15,
    }
}

/// Default dispatch range of an event, in blocks.
///
/// These radii are approximations chosen to be useful to the listener model
/// (local actions are short-range, explosions are long-range); they are not
/// claimed to match Java's registry. Tests pin the table.
pub fn default_range(kind: GameEventKind) -> f32 {
    match kind {
        GameEventKind::Step => 1.0,
        GameEventKind::PlaceBlock | GameEventKind::BreakBlock | GameEventKind::Ignite => 6.0,
        GameEventKind::TntPrime
        | GameEventKind::EntityHit
        | GameEventKind::ProjectileHit
        | GameEventKind::EntityDeath => 8.0,
        GameEventKind::Explosion => 16.0,
        GameEventKind::FluidPlace | GameEventKind::FluidPickup => 6.0,
    }
}

/// One game event at a world position. Positions are kept as `f64` so events
/// can be emitted directly from entity/blast coordinates without rounding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    pub kind: GameEventKind,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A vibration listener, such as a sculk sensor's position and frequency band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VibrationListener {
    pub pos: (f64, f64, f64),
    pub range: f32,
    pub min_frequency: u8,
    pub max_frequency: u8,
}

/// Squared distance avoids a `sqrt` and gives a stable ordering for ties.
fn squared_distance(listener: &VibrationListener, event: GameEvent) -> f64 {
    let dx = event.x - listener.pos.0;
    let dy = event.y - listener.pos.1;
    let dz = event.z - listener.pos.2;
    dx * dx + dy * dy + dz * dz
}

/// Tolerance added to the squared range so a point exactly on the boundary is
/// heard despite floating-point rounding, while a point even slightly outside
/// is not.
const DISTANCE_EPSILON_SQ: f64 = 1e-9;

/// Whether `listener` can hear `event`: the event's frequency must fall inside
/// the listener's band and its distance must be within both the event's
/// default range and the listener's own range.
pub fn can_hear(listener: &VibrationListener, event: GameEvent) -> bool {
    let event_frequency = frequency(event.kind);
    if event_frequency < listener.min_frequency || event_frequency > listener.max_frequency {
        return false;
    }
    let range = f64::from(default_range(event.kind).min(listener.range).max(0.0));
    squared_distance(listener, event) <= range * range + DISTANCE_EPSILON_SQ
}

/// Index of the closest listener that can hear `event`, or `None` when no
/// listener does. Squared distance is compared directly and the strict `<`
/// keeps the earliest index on an exact tie.
pub fn nearest_listener(listeners: &[VibrationListener], event: GameEvent) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, listener) in listeners.iter().enumerate() {
        if !can_hear(listener, event) {
            continue;
        }
        let distance_squared = squared_distance(listener, event);
        if best.is_none_or(|(_, best_distance)| distance_squared < best_distance) {
            best = Some((index, distance_squared));
        }
    }
    best.map(|(index, _)| index)
}

static RECENT_EVENTS: OnceLock<Mutex<Vec<GameEvent>>> = OnceLock::new();

fn queue() -> &'static Mutex<Vec<GameEvent>> {
    RECENT_EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Append an event to the process-wide staging queue.
pub fn emit(event: GameEvent) {
    let mut events = queue()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    events.push(event);
}

/// Snapshot the staged events. This does not drain the queue; the future
/// sensor tick owns consumption and must decide when to clear it.
pub fn recent_events() -> Vec<GameEvent> {
    let events = queue()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    events.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KINDS: [GameEventKind; 11] = [
        GameEventKind::Step,
        GameEventKind::PlaceBlock,
        GameEventKind::BreakBlock,
        GameEventKind::Explosion,
        GameEventKind::TntPrime,
        GameEventKind::EntityHit,
        GameEventKind::ProjectileHit,
        GameEventKind::Ignite,
        GameEventKind::EntityDeath,
        GameEventKind::FluidPlace,
        GameEventKind::FluidPickup,
    ];

    #[test]
    fn frequencies_are_within_vanilla_zero_to_fifteen_range() {
        for kind in ALL_KINDS {
            let value = frequency(kind);
            assert!(value <= 15, "{kind:?} frequency {value} exceeds 15");
            assert!(default_range(kind) > 0.0, "{kind:?} needs a positive range");
        }
        assert_eq!(frequency(GameEventKind::Step), 5);
        assert!(frequency(GameEventKind::Step) < frequency(GameEventKind::EntityHit));
        assert_eq!(frequency(GameEventKind::Explosion), 15);
        assert!(frequency(GameEventKind::Explosion) > frequency(GameEventKind::EntityHit));
    }

    #[test]
    fn listener_hears_only_in_range_and_frequency_band() {
        let listener = VibrationListener {
            pos: (0.0, 0.0, 0.0),
            range: 8.0,
            min_frequency: 5,
            max_frequency: 10,
        };
        let step = |x| GameEvent {
            kind: GameEventKind::Step,
            x,
            y: 0.0,
            z: 0.0,
        };
        assert!(can_hear(&listener, step(0.5)), "inside both bands");
        assert!(!can_hear(&listener, step(2.0)), "outside the event range");

        let explosion = GameEvent {
            kind: GameEventKind::Explosion,
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        assert!(!can_hear(&listener, explosion), "above the frequency band");

        let hit = GameEvent {
            kind: GameEventKind::EntityHit,
            x: 8.0,
            y: 0.0,
            z: 0.0,
        };
        assert!(can_hear(&listener, hit), "exactly on the boundary");
        let just_outside = GameEvent { x: 8.0001, ..hit };
        assert!(!can_hear(&listener, just_outside), "just past the boundary");

        let short_listener = VibrationListener {
            range: 1.0,
            ..listener
        };
        assert!(
            !can_hear(&short_listener, hit),
            "listener range also caps hearing"
        );
    }

    #[test]
    fn nearest_listener_is_deterministic_on_ties() {
        let event = GameEvent {
            kind: GameEventKind::Explosion,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let left = VibrationListener {
            pos: (-1.0, 0.0, 0.0),
            range: 16.0,
            min_frequency: 0,
            max_frequency: 15,
        };
        let right = VibrationListener {
            pos: (1.0, 0.0, 0.0),
            ..left
        };
        assert_eq!(nearest_listener(&[left, right], event), Some(0));
        assert_eq!(nearest_listener(&[right, left], event), Some(0));

        let closer = VibrationListener {
            pos: (0.25, 0.0, 0.0),
            ..left
        };
        assert_eq!(nearest_listener(&[left, right, closer], event), Some(2));

        let deaf = VibrationListener {
            min_frequency: 0,
            max_frequency: 4,
            ..closer
        };
        assert_eq!(nearest_listener(&[deaf], event), None);
        assert_eq!(nearest_listener(&[], event), None);
    }

    #[test]
    fn events_carry_world_position() {
        let event = GameEvent {
            kind: GameEventKind::FluidPickup,
            x: 1.5,
            y: -2.25,
            z: 3.75,
        };
        assert_eq!(event.x, 1.5);
        assert_eq!(event.y, -2.25);
        assert_eq!(event.z, 3.75);
        let at_event = VibrationListener {
            pos: (event.x, event.y, event.z),
            range: 1.0,
            min_frequency: 0,
            max_frequency: 15,
        };
        assert!(can_hear(&at_event, event));
    }

    #[test]
    fn emitted_events_are_available_to_consumers() {
        let first = GameEvent {
            kind: GameEventKind::TntPrime,
            x: 101.5,
            y: 2.0,
            z: -7.0,
        };
        let second = GameEvent {
            kind: GameEventKind::Explosion,
            x: 202.5,
            y: 3.0,
            z: -8.0,
        };
        emit(first);
        emit(second);
        let events = recent_events();
        assert!(events.contains(&first));
        assert!(events.contains(&second));
    }
}
