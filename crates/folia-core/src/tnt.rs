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
//! Explicit simplifications in this slice:
//! - Primed TNT is stationary: no gravity, no motion, no bouncing. The
//!   fuse simply counts down in ticks and then detonates in place.
//! - No item drops, entity/player damage, knockback, chain reactions, or
//!   sounds/particles are produced by the explosion.
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

/// A single stationary primed-TNT entity with a tick countdown.
#[derive(Clone, Debug, PartialEq)]
struct PrimedTnt {
    id: i32,
    x: f64,
    y: f64,
    z: f64,
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
        self.tnts.push(PrimedTnt {
            id,
            x: position[0],
            y: position[1],
            z: position[2],
            fuse,
        });
        vec![entity::add_entity_packet(&entity)]
    }

    /// Decrement every fuse, detonating those that reach zero.
    ///
    /// Detonated entities are removed with a single remove-entities packet
    /// and reported as [`ExplosionEvent`]s at their current position.
    pub fn tick(&mut self) -> TntTick {
        let mut tick = TntTick::default();
        let mut kept = Vec::with_capacity(self.tnts.len());
        let mut removed = Vec::new();

        for mut tnt in self.tnts.drain(..) {
            tnt.fuse = tnt.fuse.wrapping_sub(1);
            if tnt.fuse <= 0 {
                removed.push(tnt.id);
                tick.explosions.push(ExplosionEvent {
                    x: tnt.x,
                    y: tnt.y,
                    z: tnt.z,
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

    fn resistance_at_none(_pos: BlockPos) -> Option<f32> {
        None
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

        let first = store.tick();
        assert!(first.explosions.is_empty(), "fuse 1 has not detonated");
        assert!(first.packets.is_empty());
        assert_eq!(store.len(), 1);

        let second = store.tick();
        assert_eq!(second.explosions.len(), 1);
        let event = second.explosions[0];
        assert_eq!([event.x, event.y, event.z], [1.5, 64.0, 2.5]);
        assert_eq!(
            second.packets,
            vec![entity::remove_entities_packet(&[entity_id])]
        );
        assert!(store.is_empty());
    }
}
