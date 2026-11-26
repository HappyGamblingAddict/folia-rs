//! Focused tests for P3.1: shared chunk tickets and reference-counted player
//! view loaders. These exercise the ticket authority directly so the
//! refcount/unload behaviour is deterministic and does not depend on sockets.

use folia_core::chunk_tickets::{
    ChunkTicketManager, TicketChunk, TicketId, TicketKind, TicketLevel, UNLOAD_DELAY_TICKS,
    footprint,
};
use folia_core::region::ChunkPos as Pos;
use folia_core::world::Dimension;

fn ov(x: i32, z: i32) -> TicketChunk {
    TicketChunk::overworld(x, z)
}

#[test]
fn footprint_is_sorted_unique_and_clamped() {
    let area = footprint(Dimension::Overworld, Pos::new(0, 0), 2);
    assert_eq!(area.len(), 25);
    let sorted: Vec<_> = area.iter().copied().collect();
    assert!(sorted.windows(2).all(|pair| pair[0] < pair[1]));

    let single = footprint(Dimension::Overworld, Pos::new(-3, 7), 0);
    assert_eq!(single, [ov(-3, 7)].into_iter().collect());

    let clamped = footprint(Dimension::Overworld, Pos::new(1, 1), -5);
    assert_eq!(clamped, [ov(1, 1)].into_iter().collect());

    let negative = footprint(Dimension::Nether, Pos::new(-2, -2), 1);
    assert_eq!(negative.len(), 9);
    assert!(
        negative
            .iter()
            .all(|chunk| chunk.dimension == Dimension::Nether)
    );
}

#[test]
fn overlapping_players_share_one_chunk_without_duplicate_tickets() {
    let mut manager = ChunkTicketManager::new();
    let a = 1;
    let b = 2;

    manager.set_player_footprint(a, Dimension::Overworld, Pos::new(0, 0), 1);
    let change = manager.set_player_footprint(b, Dimension::Overworld, Pos::new(1, 0), 1);

    assert_eq!(change.added.len(), 3, "only B's exclusive columns are new");
    assert!(change.removed.is_empty());
    assert_eq!(manager.loaded_chunks().len(), 12);
    assert_eq!(
        manager.refcount(ov(0, 0)),
        2,
        "the overlap is one shared chunk"
    );
    assert_eq!(manager.refcount(ov(1, -1)), 2);
    assert_eq!(manager.refcount(ov(-1, 0)), 1);
    assert_eq!(manager.refcount(ov(2, 0)), 1);

    // No column is duplicated in either player's ticket list.
    for owner in [a, b] {
        let chunks = manager.player_chunks(owner);
        assert_eq!(chunks.len(), 9);
        assert!(chunks.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

#[test]
fn a_second_player_only_loads_its_exclusive_columns() {
    let mut manager = ChunkTicketManager::new();
    manager.set_player_footprint(1, Dimension::Overworld, Pos::new(0, 0), 1);
    for chunk in manager.player_chunks(1) {
        manager.mark_materialized(chunk);
    }
    let second = manager.set_player_footprint(2, Dimension::Overworld, Pos::new(1, 0), 1);

    // Six of the second player's nine columns are already held and
    // materialized by the first, so they are not generated a second time.
    assert_eq!(second.added.len(), 3);
    assert!(
        second.added.iter().all(|chunk| chunk.pos.x == 2),
        "only the exclusive column is a new load"
    );
    assert!(
        second
            .added
            .iter()
            .all(|chunk| !manager.is_materialized(*chunk))
    );
    for chunk in &second.added {
        manager.mark_materialized(*chunk);
    }
    assert_eq!(manager.materialized_chunks().len(), 12);

    // When the players leave, A's exclusive column is queued first while the
    // six shared columns stay loaded for B. Once B leaves they are queued
    // exactly once each, despite having been held by two players.
    let first_queued = manager.remove_owner(1);
    assert_eq!(first_queued.len(), 3);
    let queued = manager.remove_owner(2);
    assert_eq!(queued.len(), 9);
    let mut all: Vec<_> = first_queued.into_iter().chain(queued).collect();
    let total = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), total, "no column is queued twice");
}

#[test]
fn movement_adds_the_new_footprint_and_removes_the_old() {
    let mut manager = ChunkTicketManager::new();
    let owner = 42;
    manager.set_player_footprint(owner, Dimension::Overworld, Pos::new(0, 0), 1);

    let change = manager.set_player_footprint(owner, Dimension::Overworld, Pos::new(2, 0), 1);
    let added: Vec<_> = change.added.iter().map(|c| (c.pos.x, c.pos.z)).collect();
    let removed: Vec<_> = change.removed.iter().map(|c| (c.pos.x, c.pos.z)).collect();
    assert_eq!(added.len(), 6);
    assert_eq!(removed.len(), 6);
    assert!(added.iter().all(|(x, _)| *x >= 2));
    assert!(removed.iter().all(|(x, _)| *x <= 0));

    assert_eq!(manager.loaded_chunks().len(), 9, "the set stays bounded");
    assert_eq!(manager.refcount(ov(1, 0)), 1, "the shared column survives");
    assert_eq!(manager.pending_unloads().len(), 6);
    assert!(
        manager
            .pending_unloads()
            .iter()
            .all(|chunk| removed.contains(&(chunk.pos.x, chunk.pos.z)))
    );
}

#[test]
fn disconnect_cleanup_keeps_other_players_and_shared_chunks() {
    let mut manager = ChunkTicketManager::new();
    let a = 10;
    let b = 11;
    manager.set_player_footprint(a, Dimension::Overworld, Pos::new(0, 0), 1);
    manager.set_player_footprint(b, Dimension::Overworld, Pos::new(1, 0), 0);
    manager.set_spawn_footprint(Dimension::Overworld, Pos::new(0, 0), 0);
    manager.force_chunk(ov(9, 9));

    let queued = manager.remove_owner(a);
    assert_eq!(
        queued.len(),
        7,
        "A's seven exclusive columns are pending unload"
    );
    assert_eq!(
        manager.refcount(ov(0, 0)),
        1,
        "the spawn ticket still holds it"
    );
    assert_eq!(manager.refcount(ov(1, 0)), 1, "B's own column is untouched");
    assert!(manager.player_chunks(a).is_empty());
    assert_eq!(manager.player_chunks(b), vec![ov(1, 0)]);

    let persistent = manager.persistent_chunks();
    assert_eq!(persistent, vec![ov(0, 0), ov(9, 9)]);
    assert!(manager.is_loaded(ov(9, 9)));
    assert!(manager.is_loaded(ov(0, 0)));
}

#[test]
fn spawn_and_forced_tickets_survive_every_player_disconnect() {
    let mut manager = ChunkTicketManager::new();
    manager.set_spawn_footprint(Dimension::Overworld, Pos::new(10, 10), 1);
    manager.force_chunk(ov(20, 20));
    manager.set_player_footprint(7, Dimension::Overworld, Pos::new(10, 10), 0);

    assert_eq!(manager.remove_owner(7), Vec::<TicketChunk>::new());
    assert!(manager.pending_unloads().is_empty());
    assert_eq!(manager.loaded_chunks().len(), 10, "9 spawn + 1 forced");
    assert_eq!(manager.refcount(ov(10, 10)), 1, "spawn still holds it");
    assert!(manager.is_loaded(ov(20, 20)));

    // Shrinking the spawn footprint releases exactly its outer ring; the
    // explicit forced column is unaffected.
    manager.set_spawn_footprint(Dimension::Overworld, Pos::new(10, 10), 0);
    assert_eq!(manager.pending_unloads().len(), 8);
    assert!(manager.is_loaded(ov(10, 10)));
    assert!(manager.is_loaded(ov(20, 20)));
    assert!(manager.unforce_chunk(ov(20, 20)));
    assert!(!manager.is_loaded(ov(20, 20)));
}

#[test]
fn a_readded_ticket_cancels_the_pending_unload() {
    let mut manager = ChunkTicketManager::new();
    manager.set_player_footprint(1, Dimension::Overworld, Pos::new(3, 3), 0);
    assert!(manager.remove_ticket(ov(3, 3), TicketId::player(1)));
    assert_eq!(manager.pending_unloads(), vec![ov(3, 3)]);

    assert!(manager.add_for(ov(3, 3), TicketId::player(1)));
    assert!(manager.pending_unloads().is_empty());
    for _ in 0..UNLOAD_DELAY_TICKS {
        assert!(manager.tick().is_empty());
    }
    assert!(manager.is_loaded(ov(3, 3)));
}

#[test]
fn the_unload_queue_drains_deterministically_after_the_grace_window() {
    let mut manager = ChunkTicketManager::new();
    for (owner, x) in [(1, -4), (2, 0), (3, 7)] {
        manager.set_player_footprint(owner, Dimension::Overworld, Pos::new(x, x), 0);
        manager.remove_owner(owner);
    }
    let expected = vec![ov(-4, -4), ov(0, 0), ov(7, 7)];
    assert_eq!(manager.pending_unloads(), expected);

    for _ in 0..UNLOAD_DELAY_TICKS - 1 {
        assert!(manager.tick().is_empty());
    }
    assert_eq!(manager.tick(), expected);
    assert!(manager.pending_unloads().is_empty());
    assert!(manager.loaded_chunks().is_empty());
}

#[test]
fn levels_track_the_most_loaded_holder() {
    let mut manager = ChunkTicketManager::new();
    let forced = TicketId::new(100, TicketKind::Forced);
    let player = TicketId::player(1);

    manager.add_ticket(ov(0, 0), forced, forced.kind.default_level());
    manager.add_ticket(ov(0, 0), player, player.kind.default_level());
    assert_eq!(
        manager.effective_level(ov(0, 0)),
        Some(TicketLevel::ENTITY_TICKING)
    );
    assert!(manager.is_ticking(ov(0, 0)));

    assert!(!manager.remove_ticket(ov(0, 0), player));
    assert_eq!(
        manager.effective_level(ov(0, 0)),
        Some(TicketLevel::BLOCK_TICKING)
    );
    assert!(manager.remove_ticket(ov(0, 0), forced));
    assert_eq!(manager.effective_level(ov(0, 0)), None);
    assert!(!manager.is_ticking(ov(0, 0)));
    assert!(TicketLevel::FULL.is_ticking());
    assert!(!TicketLevel::UNLOADED.is_ticking());
}

#[test]
fn dimensions_do_not_share_the_same_coordinates() {
    let mut manager = ChunkTicketManager::new();
    manager.set_player_footprint(1, Dimension::Overworld, Pos::new(0, 0), 0);
    manager.set_player_footprint(2, Dimension::Nether, Pos::new(0, 0), 0);

    assert_eq!(manager.loaded_chunks().len(), 2);
    assert_eq!(manager.refcount(ov(0, 0)), 1);
    assert_eq!(
        manager.refcount(TicketChunk::new(Dimension::Nether, 0, 0)),
        1
    );
    assert_eq!(manager.remove_owner(1), vec![ov(0, 0)]);
    assert!(manager.is_loaded(TicketChunk::new(Dimension::Nether, 0, 0)));
}

#[test]
fn materialized_chunks_are_recorded_once_and_survive_unload() {
    let mut manager = ChunkTicketManager::new();
    let chunk = ov(4, 5);
    manager.set_player_footprint(1, Dimension::Overworld, Pos::new(4, 5), 0);
    assert!(!manager.is_materialized(chunk));
    manager.mark_materialized(chunk);
    manager.mark_materialized(chunk);
    assert!(manager.is_materialized(chunk));
    assert_eq!(manager.materialized_chunks(), vec![chunk]);

    manager.remove_owner(1);
    let mut drained = Vec::new();
    for _ in 0..UNLOAD_DELAY_TICKS {
        drained.extend(manager.tick());
    }
    assert_eq!(drained, vec![chunk]);
    assert!(!manager.is_loaded(chunk));
    assert!(
        manager.is_materialized(chunk),
        "a cached chunk can be reused later"
    );
}
