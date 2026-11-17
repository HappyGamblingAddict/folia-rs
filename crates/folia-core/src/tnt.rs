//! Primed TNT and deterministic block-destruction explosions.
//!
//! This is the first vertical slice of P6.6. It models the client-facing
//! primed-TNT entity (id/type/UUID and a fuse that emits an explosion event)
//! plus the vanilla 16x16x16 shell ray shape used to choose which blocks an
//! explosion destroys. The ray shape is the same algorithm as vanilla
//! `Explosion`: for each shell direction a `LegacyRandomSource` seeded per
//! explosion picks the ray strength, and the ray walks 0.3 blocks at a time,
//! losing power to each block's blast resistance.
//!
//! Physics and simplifications in this slice:
//! - Primed TNT reuses the shared `Entity::tick_physics` for gravity, drag and
//!   AABB collision against the world, so it falls and comes to rest on solid
//!   blocks before detonating. That path uses the shared living-entity gravity
//!   constant (`0.08`) rather than vanilla TNT's `0.04`; drag (`0.98`) matches.
//!   TNT therefore falls about twice as fast as vanilla, a deliberate
//!   approximation to avoid duplicating a second physics implementation.
//! - TNT does not bounce or inherit a launch velocity from explosions; only
//!   the shared vertical gravity/collision path is reused.
//! - Block drops (with a 1/radius decay approximation), TNT chain reactions,
//!   and their add-entity packets are produced by `join.rs` around this shape.
//! - No entity/player damage, knockback, or sounds/particles are produced by
//!   the explosion shape itself.
//! - The block set is written to the world by `join.rs`, which owns the
//!   session and the broadcast path.

use crate::entity::{self, Entity};
use crate::math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource};
use std::collections::BTreeSet;

/// One primed TNT detonation, positioned at the entity's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplosionEvent {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A single primed-TNT entity with a tick countdown.
///
/// Motion lives in the shared [`Entity`]; only the fuse is TNT-specific.
#[derive(Clone, Debug, PartialEq)]
struct PrimedTnt {
    entity: Entity,
    fuse: i32,
}

/// Packets and explosion events produced by one `TntStore::tick`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TntTick {
    pub packets: Vec<Vec<u8>>,
    pub explosions: Vec<ExplosionEvent>,
}

/// Session-local collection of primed TNT entities.
#[derive(Debug, Clone)]
pub struct TntStore {
    tnts: Vec<PrimedTnt>,
    next_id: i32,
}

impl TntStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.tnts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tnts.len()
    }

    /// Create a primed TNT entity and return its add-entity packet.
    ///
    /// The UUID is derived from the entity id, matching the other
    /// session-local entity stores. No metadata packet is needed yet: the
    /// client renders the type from the add-entity packet alone.
    pub fn ignite(&mut self, position: [f64; 3], fuse: i32) -> Vec<Vec<u8>> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut uuid = [0u8; 16];
        uuid[0..4].copy_from_slice(&id.to_le_bytes());
        let entity_type = crate::registry::entity_type_id("minecraft:tnt").unwrap_or(132);
        let mut entity = Entity::new(id, uuid, entity_type, position[0], position[1], position[2]);
        entity.width = 0.98;
        entity.height = 0.98;
        let packet = entity::add_entity_packet(&entity);
        self.tnts.push(PrimedTnt { entity, fuse });
        vec![packet]
    }

    /// Advance gravity/collision for every TNT, then decrement its fuse and
    /// detonate those that reach zero.
    ///
    /// Each TNT reuses [`Entity::tick_physics`] so it falls and collides with
    /// the world before the fuse is advanced. When the entity moved this tick,
    /// a position-sync packet is emitted. Detonated entities are removed with
    /// a single remove-entities packet and reported as [`ExplosionEvent`]s at
    /// their current (post-physics) position.
    ///
    /// See the module docs for the gravity approximation: shared living-entity
    /// gravity (`0.08`) is used instead of vanilla TNT's `0.04`.
    pub fn tick(&mut self, collider: &mut impl crate::entity::BlockCollider) -> TntTick {
        let mut tick = TntTick::default();
        let mut kept = Vec::with_capacity(self.tnts.len());
        let mut removed = Vec::new();

        for mut tnt in self.tnts.drain(..) {
            if tnt.entity.tick_physics(collider) {
                tick.packets
                    .push(entity::entity_position_sync_packet(&tnt.entity));
            }
            tnt.fuse = tnt.fuse.wrapping_sub(1);
            if tnt.fuse <= 0 {
                removed.push(tnt.entity.id);
                tick.explosions.push(ExplosionEvent {
                    x: tnt.entity.x,
                    y: tnt.entity.y,
                    z: tnt.entity.z,
                });
            } else {
                kept.push(tnt);
            }
        }

        if !removed.is_empty() {
            tick.packets.push(entity::remove_entities_packet(&removed));
        }
        self.tnts = kept;
        tick
    }
}

impl Default for TntStore {
    fn default() -> Self {
        Self {
            tnts: Vec::new(),
            next_id: 40_000,
        }
    }
}

/// Vanilla `Explosion` damage at `distance` from the centre of an explosion of
/// `radius`, returned as `f32` like the entity health it is applied to.
///
/// The exposure factor is treated as `1.0`: this is the damage an entity with
/// an unobstructed line of sight to the explosion centre takes. Exposure
/// occlusion (rays cast against blocks between the centre and the victim) is
/// not modelled in this slice.
pub fn explosion_damage(radius: f32, distance: f64) -> f32 {
    if distance >= f64::from(radius) {
        return 0.0;
    }
    let d = 1.0 - distance / f64::from(radius);
    let raw = (d * d + d) / 2.0 * 7.0 * f64::from(radius) + 1.0;
    raw as f32
}

/// Fuse length for primed TNT ignited by another explosion's blast.
///
/// Vanilla TNT's `onCaughtInExplosion` arms a new primed-TNT entity with
/// `10 + random.nextInt(21)` ticks, i.e. an inclusive `10..=30` range. The
/// supplied source is the explosion's drop RNG, so a chain fuse is
/// reproducible for a given seed.
pub fn chain_fuse(random: &mut impl RandomSource) -> i32 {
    10 + random.next_int_bound(21)
}

/// Return the sorted, unique set of blocks an explosion of `radius` at
/// `center` would destroy, using `seed` for the vanilla ray-strength draws.
///
/// `resistance` returns the blast resistance of a solid block, or `None` for
/// air/unloaded space: `None` blocks are not destroyed but the ray keeps
/// walking through them. A block is included only if the ray still has power
/// after paying its resistance.
pub fn explosion_blocks(
    center: [f64; 3],
    radius: f32,
    seed: i64,
    mut resistance: impl FnMut(BlockPos) -> Option<f32>,
) -> Vec<BlockPos> {
    let mut random = LegacyRandomSource::new(seed);
    // `BlockPos` has no `Ord`, so key the set by coordinate tuples. The tuple
    // ordering is lexicographic on (x, y, z), which is the returned order.
    let mut blocks: BTreeSet<(i32, i32, i32)> = BTreeSet::new();

    for i in 0..16 {
        for j in 0..16 {
            for k in 0..16 {
                if i != 0 && i != 15 && j != 0 && j != 15 && k != 0 && k != 15 {
                    continue;
                }

                // Vanilla computes these three components in f32, then the
                // length and normalization in f64.
                let fx = (i as f32 / 15.0) * 2.0 - 1.0;
                let fy = (j as f32 / 15.0) * 2.0 - 1.0;
                let fz = (k as f32 / 15.0) * 2.0 - 1.0;
                let dx = f64::from(fx);
                let dy = f64::from(fy);
                let dz = f64::from(fz);
                let length = (dx * dx + dy * dy + dz * dz).sqrt();
                let dx = dx / length;
                let dy = dy / length;
                let dz = dz / length;

                let mut power = radius * (0.7 + random.next_float() * 0.6);
                let (mut x, mut y, mut z) = (center[0], center[1], center[2]);
                while power > 0.0 {
                    let pos = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
                    if let Some(value) = resistance(pos) {
                        power -= (value + 0.3) * 0.3;
                        if power > 0.0 {
                            blocks.insert((pos.x(), pos.y(), pos.z()));
                        }
                    }
                    power -= 0.225;
                    x += dx * 0.3;
                    y += dy * 0.3;
                    z += dz * 0.3;
                }
            }
        }
    }

    blocks
        .into_iter()
        .map(|(x, y, z)| BlockPos::new(x, y, z))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::FlatCollider;

    fn resistance_at_none(_pos: BlockPos) -> Option<f32> {
        None
    }

    #[test]
    fn explosion_damage_is_zero_at_and_beyond_radius() {
        assert_eq!(explosion_damage(4.0, 4.0), 0.0, "at the edge");
        assert_eq!(explosion_damage(4.0, 5.0), 0.0, "beyond the edge");
        assert_eq!(explosion_damage(0.0, 0.0), 0.0, "zero radius never damages");
    }

    #[test]
    fn explosion_damage_is_positive_and_decreases_inward() {
        let radius = 4.0;
        let mut previous = f32::INFINITY;
        for step in 0..=8 {
            let distance = step as f64 * 0.5;
            let amount = explosion_damage(radius, distance);
            if distance >= f64::from(radius) {
                assert_eq!(amount, 0.0, "no damage at distance {distance}");
            } else {
                assert!(amount > 0.0, "damage must be positive at {distance}");
                assert!(
                    amount < previous,
                    "damage must fall as distance grows: {amount} at {distance}, previous {previous}"
                );
            }
            previous = amount;
        }
    }

    #[test]
    fn explosion_damage_center_is_maximum_for_radius_four() {
        let center = explosion_damage(4.0, 0.0);
        assert_eq!(
            center, 29.0,
            "vanilla yields (2/2)*7*4 + 1 = 29 at the centre"
        );
        for step in 1..40 {
            let distance = step as f64 * 0.1;
            assert!(
                explosion_damage(4.0, distance) < center,
                "no point inside radius 4 may exceed the centre damage"
            );
        }
    }

    #[test]
    fn chain_fuse_is_between_ten_and_thirty() {
        let mut saw_min = false;
        let mut saw_max = false;
        for seed in 0..2048_i64 {
            let mut random = LegacyRandomSource::new(seed);
            let fuse = chain_fuse(&mut random);
            assert!(
                (10..=30).contains(&fuse),
                "seed {seed} produced out-of-range fuse {fuse}"
            );
            saw_min |= fuse == 10;
            saw_max |= fuse == 30;
        }
        assert!(saw_min && saw_max, "both fuse endpoints must be reachable");
    }

    #[test]
    fn explosion_blocks_is_deterministic_and_sorted() {
        let center = [4.5, 70.0, -8.5];
        let solid = |_pos: BlockPos| Some(0.0_f32);
        let first = explosion_blocks(center, 4.0, 0xE7C1, solid);
        let second = explosion_blocks(center, 4.0, 0xE7C1, solid);
        assert_eq!(first, second);
        assert!(!first.is_empty(), "radius 4 over solid blocks hits some");

        let mut sorted = first.clone();
        sorted.sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        sorted.dedup();
        assert_eq!(first, sorted, "output must be sorted and unique");
    }

    #[test]
    fn radius_zero_has_no_blocks() {
        assert!(
            explosion_blocks([0.0, 0.0, 0.0], 0.0, 7, resistance_at_none).is_empty(),
            "a zero-radius explosion has no ray power"
        );
    }

    #[test]
    fn resistant_blocks_stop_the_ray() {
        let hard = [
            BlockPos::new(1, 0, 0),
            BlockPos::new(0, 1, 0),
            BlockPos::new(0, 0, 1),
        ];
        let resistance = |pos: BlockPos| {
            if hard.contains(&pos) {
                Some(1200.0)
            } else {
                Some(0.0)
            }
        };
        let blocks = explosion_blocks([0.5, 0.5, 0.5], 4.0, 99, resistance);
        assert!(!blocks.is_empty());
        for pos in &blocks {
            assert!(
                !hard.contains(pos),
                "a block with resistance 1200 must stop its ray before insertion: {pos:?}"
            );
        }
    }

    #[test]
    fn tnt_store_fuse_ignites_and_explodes() {
        let mut store = TntStore::new();
        let mut collider = FlatCollider { ground_y: 0 };
        assert!(store.is_empty());

        let packets = store.ignite([1.5, 64.0, 2.5], 2);
        assert_eq!(store.len(), 1);
        assert_eq!(packets.len(), 1);

        // add-entity packet: packet id 1, entity id, 16-byte UUID, type id.
        let packet = &packets[0];
        let mut reader = crate::protocol::PacketReader::new(packet);
        assert_eq!(
            reader.read_varint().unwrap(),
            entity::CLIENTBOUND_ADD_ENTITY
        );
        let entity_id = reader.read_varint().unwrap();
        assert_eq!(entity_id, 40_000);
        let _uuid = reader.read_bytes(16).unwrap();
        assert_eq!(
            reader.read_varint().unwrap(),
            132,
            "minecraft:tnt entity type id"
        );

        let first = store.tick(&mut collider);
        assert!(first.explosions.is_empty(), "fuse 1 has not detonated");
        assert_eq!(
            first.packets,
            vec![entity::entity_position_sync_packet(&store.tnts[0].entity)],
            "the falling TNT emits a position-sync packet"
        );
        assert_eq!(store.len(), 1);

        let second = store.tick(&mut collider);
        assert_eq!(second.explosions.len(), 1);
        let event = second.explosions[0];
        assert_eq!([event.x, event.z], [1.5, 2.5]);
        assert!(
            event.y < 64.0,
            "the TNT fell under gravity before detonating"
        );
        assert_eq!(
            second.packets.last(),
            Some(&entity::remove_entities_packet(&[entity_id])),
            "detonation removes the entity"
        );
        assert!(store.is_empty());
    }

    #[test]
    fn tnt_falls_and_stops_on_the_ground() {
        let mut store = TntStore::new();
        let mut collider = FlatCollider { ground_y: 1 };
        store.ignite([0.5, 5.0, 0.5], 40);
        assert_eq!(store.len(), 1);

        let mut last_y = store.tnts[0].entity.y;
        let mut saw_decrease = false;
        let mut stopped_y = None;
        let mut explosion = None;
        for _ in 0..80 {
            let tick = store.tick(&mut collider);
            if let Some(event) = tick.explosions.first() {
                explosion = Some(*event);
                break;
            }
            let y = store.tnts[0].entity.y;
            if y < last_y {
                saw_decrease = true;
            } else if saw_decrease && stopped_y.is_none() {
                stopped_y = Some(y);
            }
            last_y = y;
        }

        assert!(saw_decrease, "the TNT must fall under gravity");
        let stopped = stopped_y.expect("the TNT must come to rest before detonating");
        assert!(
            stopped >= 1.0,
            "the TNT must rest on top of the floor at y=1, got {stopped}"
        );
        let event = explosion.expect("the TNT must detonate within 80 ticks");
        assert!(
            (event.y - stopped).abs() < 1.0e-9,
            "explosion must occur at the resting position: {} vs {}",
            event.y,
            stopped
        );
        assert!((event.x - 0.5).abs() < 1.0e-9 && (event.z - 0.5).abs() < 1.0e-9);
    }

    #[test]
    fn tnt_does_not_move_through_blocks() {
        let mut store = TntStore::new();
        let ground_y = 5;
        let mut collider = FlatCollider { ground_y };
        store.ignite([0.5, 20.0, 0.5], 80);

        for _ in 0..80 {
            let tick = store.tick(&mut collider);
            if tick.explosions.is_empty() {
                let y = store.tnts[0].entity.y;
                assert!(
                    y >= f64::from(ground_y),
                    "the TNT must not fall through the solid floor: y={y}"
                );
            }
        }
    }
}
