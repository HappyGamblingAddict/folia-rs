//! Shared per-viewer entity tracking tests.
//!
//! `EntityTracker` owns entity records once, keyed by network id. Each viewer
//! has its own squared-distance range set; enter/leave/update packets are
//! generated in ascending id order so multi-viewer runs are deterministic.

use folia_core::effects::{
    CLIENTBOUND_REMOVE_MOB_EFFECT, CLIENTBOUND_UPDATE_MOB_EFFECT, StatusEffect, StatusEffectKind,
};
use folia_core::entity::{
    CLIENTBOUND_ADD_ENTITY, CLIENTBOUND_ENTITY_POSITION_SYNC, CLIENTBOUND_REMOVE_ENTITIES, Entity,
    EntityTracker, SessionEntityTracker, TrackedEntity,
};
use folia_core::entity_metadata::{
    CLIENTBOUND_SET_ENTITY_DATA, CLIENTBOUND_SET_EQUIPMENT, LivingEntity,
};
use folia_core::protocol::PacketReader;

fn packet_id(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().expect("packet id")
}

fn add_entity_id(packet: &[u8]) -> i32 {
    assert_eq!(packet_id(packet), CLIENTBOUND_ADD_ENTITY);
    let mut reader = PacketReader::new(packet);
    let _ = reader.read_varint().expect("packet id");
    reader.read_varint().expect("entity id")
}

fn remove_entity_ids(packet: &[u8]) -> Vec<i32> {
    assert_eq!(packet_id(packet), CLIENTBOUND_REMOVE_ENTITIES);
    let mut reader = PacketReader::new(packet);
    let _ = reader.read_varint().expect("packet id");
    let count = reader.read_varint().expect("count");
    (0..count)
        .map(|_| reader.read_varint().expect("entity id"))
        .collect()
}

fn tracked(id: i32, x: f64, y: f64, z: f64) -> TrackedEntity {
    let mut entity = Entity::new(id, [id as u8; 16], 5, x, y, z);
    entity.width = 0.6;
    entity.height = 1.8;
    TrackedEntity {
        entity,
        living: LivingEntity::new(id),
        ai: None,
    }
}

#[test]
fn enter_and_leave_happen_at_the_exact_range_boundary() {
    let mut tracker = EntityTracker::new();
    tracker.insert(tracked(1, 10.0, 0.0, 0.0)); // exactly 10 blocks away
    tracker.insert(tracked(2, 10.001, 0.0, 0.0)); // just outside
    assert!(tracker.add_viewer(7, (0.0, 0.0, 0.0), 10.0));

    let packets = tracker.update_viewer(7);
    let entered: Vec<i32> = packets
        .iter()
        .filter(|packet| packet_id(packet) == CLIENTBOUND_ADD_ENTITY)
        .map(|packet| add_entity_id(packet))
        .collect();
    assert_eq!(entered, vec![1], "the boundary entity is visible");

    // Each entering entity pairs Add Entity with full metadata and equipment.
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_SET_ENTITY_DATA)
            .count(),
        1
    );
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_SET_EQUIPMENT)
            .count(),
        1
    );
    assert!(tracker.is_tracked_by(7, 1));
    assert!(!tracker.is_tracked_by(7, 2));
    assert!(tracker.viewer(7).expect("viewer").tracks(1));

    // Moving outside the range removes the tracked id.
    assert!(tracker.set_viewer_position(7, (100.0, 0.0, 0.0)));
    let packets = tracker.update_viewer(7);
    let removals: Vec<Vec<i32>> = packets
        .iter()
        .filter(|packet| packet_id(packet) == CLIENTBOUND_REMOVE_ENTITIES)
        .map(|packet| remove_entity_ids(packet))
        .collect();
    assert_eq!(removals, vec![vec![1]]);
    assert!(!tracker.is_tracked_by(7, 1));
    assert!(
        tracker
            .viewer(7)
            .expect("viewer")
            .tracked_ids()
            .next()
            .is_none()
    );
}

#[test]
fn per_viewer_ranges_filter_independently() {
    let mut tracker = EntityTracker::new();
    tracker.insert(tracked(1, 5.0, 0.0, 0.0));
    tracker.insert(tracked(2, -5.0, 0.0, 0.0));

    // Viewer 5 sees both; viewer 2 is shifted right and only sees id 1.
    tracker.add_viewer(5, (0.0, 0.0, 0.0), 6.0);
    tracker.add_viewer(2, (5.0, 0.0, 0.0), 6.0);
    let updates = tracker.update_all_viewers();

    assert_eq!(
        updates
            .iter()
            .map(|(viewer, _)| *viewer)
            .collect::<Vec<_>>(),
        vec![2, 5],
        "viewer updates are ordered by viewer id"
    );
    let viewer2 = &updates[0].1;
    let viewer5 = &updates[1].1;
    let ids = |packets: &[Vec<u8>]| -> Vec<i32> {
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_ADD_ENTITY)
            .map(|packet| add_entity_id(packet))
            .collect()
    };
    assert_eq!(ids(viewer2), vec![1]);
    assert_eq!(ids(viewer5), vec![1, 2]);

    let tracked2: Vec<i32> = tracker.viewer(2).expect("viewer 2").tracked_ids().collect();
    let tracked5: Vec<i32> = tracker.viewer(5).expect("viewer 5").tracked_ids().collect();
    assert_eq!(tracked2, vec![1]);
    assert_eq!(tracked5, vec![1, 2]);
}

#[test]
fn range_changes_do_not_leak_between_viewers() {
    let mut tracker = EntityTracker::new();
    tracker.insert(tracked(1, 5.0, 0.0, 0.0));
    tracker.add_viewer(1, (0.0, 0.0, 0.0), 10.0);
    tracker.add_viewer(2, (0.0, 0.0, 0.0), 2.0);

    let updates = tracker.update_all_viewers();
    assert_eq!(updates.len(), 1, "only viewer 1 has packets");
    assert_eq!(updates[0].0, 1);

    // Expanding viewer 2's range enters the entity without touching viewer 1.
    assert!(tracker.set_viewer_range(2, 5.0));
    let packets = tracker.update_viewer(2);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_ADD_ENTITY)
            .map(|packet| add_entity_id(packet))
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert!(tracker.is_tracked_by(1, 1));
    assert!(tracker.is_tracked_by(2, 1));

    // Shrinking viewer 1 removes only viewer 1's tracking; viewer 2 keeps it.
    assert!(tracker.set_viewer_range(1, 1.0));
    let packets = tracker.update_viewer(1);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_REMOVE_ENTITIES)
            .map(|packet| remove_entity_ids(packet))
            .collect::<Vec<_>>(),
        vec![vec![1]]
    );
    assert!(!tracker.is_tracked_by(1, 1));
    assert!(tracker.is_tracked_by(2, 1));
}

#[test]
fn enter_leave_and_update_order_is_deterministic() {
    let mut tracker = EntityTracker::new();
    // Insertion order intentionally differs from network-id order.
    tracker.insert(tracked(30, 0.0, 0.0, 0.0));
    tracker.insert(tracked(10, 1.0, 0.0, 0.0));
    tracker.insert(tracked(20, 2.0, 0.0, 0.0));
    tracker.add_viewer(9, (0.0, 0.0, 0.0), 20.0);

    let packets = tracker.update_viewer(9);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet_id(packet) == CLIENTBOUND_ADD_ENTITY)
            .map(|packet| add_entity_id(packet))
            .collect::<Vec<_>>(),
        vec![10, 20, 30]
    );

    // Deterministic entity iteration and ids.
    assert_eq!(tracker.ids().collect::<Vec<_>>(), vec![10, 20, 30]);
    assert_eq!(
        tracker
            .iter()
            .map(|tracked| tracked.entity.id)
            .collect::<Vec<_>>(),
        vec![10, 20, 30]
    );

    // Leaves are batched in ascending id order.
    tracker.set_viewer_position(9, (1000.0, 0.0, 0.0));
    let packets = tracker.update_viewer(9);
    assert_eq!(packets.len(), 1);
    assert_eq!(remove_entity_ids(&packets[0]), vec![10, 20, 30]);

    // Viewers are iterated in ascending id order as well.
    tracker.add_viewer(3, (0.0, 0.0, 0.0), 20.0);
    tracker.add_viewer(1, (0.0, 0.0, 0.0), 20.0);
    assert_eq!(
        tracker.viewer_ids().collect::<Vec<_>>(),
        vec![1, 3, 9],
        "viewer 9 remains registered with an empty range set"
    );
}

#[test]
fn position_updates_are_only_emitted_when_the_snapshot_changes() {
    let mut tracker = EntityTracker::new();
    tracker.insert(tracked(10, 0.0, 0.0, 0.0));
    tracker.add_viewer(1, (0.0, 0.0, 0.0), 10.0);

    tracker.update_viewer(1);
    assert!(tracker.update_viewer(1).is_empty(), "nothing moved");

    tracker.get_mut(10).expect("entity").entity.x = 1.5;
    let packets = tracker.update_viewer(1);
    assert_eq!(packets.len(), 1);
    assert_eq!(packet_id(&packets[0]), CLIENTBOUND_ENTITY_POSITION_SYNC);
    let mut reader = PacketReader::new(&packets[0]);
    let _ = reader.read_varint().expect("packet id");
    assert_eq!(reader.read_varint().expect("entity id"), 10);

    assert!(tracker.update_viewer(1).is_empty(), "snapshot is current");

    tracker.get_mut(10).expect("entity").entity.yaw = 90.0;
    let packets = tracker.update_viewer(1);
    assert_eq!(packets.len(), 1);
    assert_eq!(packet_id(&packets[0]), CLIENTBOUND_ENTITY_POSITION_SYNC);
}

#[test]
fn removing_an_entity_queues_one_removal_per_tracking_viewer() {
    let mut tracker = EntityTracker::new();
    tracker.insert(tracked(10, 0.0, 0.0, 0.0));
    tracker.add_viewer(1, (0.0, 0.0, 0.0), 10.0);
    tracker.add_viewer(2, (20.0, 0.0, 0.0), 2.0);
    tracker.update_all_viewers();

    assert!(tracker.remove_entity(10).is_some());
    assert!(tracker.get(10).is_none());

    let viewer1 = tracker.update_viewer(1);
    assert_eq!(viewer1.len(), 1);
    assert_eq!(remove_entity_ids(&viewer1[0]), vec![10]);
    assert!(
        tracker.update_viewer(2).is_empty(),
        "viewer 2 never tracked the entity"
    );
}

#[test]
fn entering_pairs_active_effects_with_update_packets() {
    let mut tracker = EntityTracker::new();
    let mut entity = tracked(4, 0.0, 0.0, 0.0);
    entity
        .living
        .apply_effect(StatusEffect::new(StatusEffectKind::Speed, 200, 1));
    entity
        .living
        .apply_effect(StatusEffect::new(StatusEffectKind::NightVision, 200, 0));
    tracker.insert(entity);
    tracker.add_viewer(1, (0.0, 0.0, 0.0), 10.0);

    let packets = tracker.update_viewer(1);
    let effects: Vec<i32> = packets
        .iter()
        .filter(|packet| packet_id(packet) == CLIENTBOUND_UPDATE_MOB_EFFECT)
        .map(|packet| {
            let mut reader = PacketReader::new(packet);
            let _ = reader.read_varint().expect("packet id");
            let _ = reader.read_varint().expect("entity id");
            reader.read_varint().expect("effect id")
        })
        .collect();
    assert_eq!(effects, vec![0, 15], "speed then night vision, add order");
}

#[test]
fn session_tracker_consumes_the_shared_entity_records() {
    let mut session = SessionEntityTracker::deterministic((0.5, -60.0, 0.5), -60, 7);
    assert_eq!(session.shared().len(), 3);
    assert_eq!(
        session.shared().ids().collect::<Vec<_>>(),
        session.tracked_ids()
    );

    let id = session
        .tracked_ids()
        .into_iter()
        .find(|id| session.entity(*id).expect("entity").entity.height > 1.9)
        .expect("zombie");
    let old_x = session.entity(id).expect("zombie").entity.x;

    // Mutating the shared tracker is visible through the session wrapper: the
    // session does not keep a second copy of entity state.
    session.shared_mut().get_mut(id).expect("zombie").entity.x = old_x + 2.0;
    assert_eq!(session.entity(id).expect("zombie").entity.x, old_x + 2.0);

    session
        .shared_mut()
        .get_mut(id)
        .expect("zombie")
        .living
        .apply_effect(StatusEffect::new(StatusEffectKind::Speed, 100, 0));
    let join = session.join_packets();
    assert!(
        join.iter().any(
            |packet| packet_id(packet) == CLIENTBOUND_UPDATE_MOB_EFFECT && {
                let mut reader = PacketReader::new(packet);
                let _ = reader.read_varint().expect("packet id");
                let entity = reader.read_varint().expect("entity id");
                entity == id
            }
        ),
        "join pairing includes active effects from the shared record"
    );
    assert!(
        !join
            .iter()
            .any(|packet| packet_id(packet) == CLIENTBOUND_REMOVE_MOB_EFFECT),
        "no remove packet before expiry"
    );
}
