use folia_core::block_shapes::Aabb;
use folia_core::entity::FlatCollider;
use folia_core::projectile::{Projectile, ProjectileKind, ProjectileStore};

#[test]
fn arrow_gravity_and_drag_follow_the_model() {
    let mut projectile = Projectile::spawn(
        ProjectileKind::Arrow,
        1,
        None,
        0.5,
        64.5,
        0.5,
        [0.0, 0.0, 0.0],
    );
    let mut collider = FlatCollider { ground_y: -1000 };
    let tick = projectile.tick(&mut collider);
    assert!(tick.moved);
    assert!((projectile.velocity[1] + 0.0495).abs() < 1e-9);
    assert!(projectile.y < 64.5);
}

#[test]
fn arrow_sticks_on_a_floor() {
    let mut projectile = Projectile::spawn(
        ProjectileKind::Arrow,
        2,
        None,
        0.5,
        1.5,
        0.5,
        [0.0, -1.0, 0.0],
    );
    let mut collider = FlatCollider { ground_y: 0 };
    while !projectile.stuck && projectile.age < 100 {
        projectile.tick(&mut collider);
    }
    assert!(projectile.stuck);
    assert_eq!(projectile.velocity, [0.0, 0.0, 0.0]);
    let age = projectile.age;
    projectile.tick(&mut collider);
    assert_eq!(projectile.age, age + 1);
}

#[test]
fn arrow_damage_is_ceil_of_speed_times_two() {
    let mut arrow = Projectile::spawn(ProjectileKind::Arrow, 3, None, 0.0, 0.0, 0.0, [0.0; 3]);
    assert_eq!(arrow.hit_damage(1.1), 3.0);
    assert_eq!(arrow.hit_damage(2.0), 4.0);

    arrow.critical = true;
    assert_eq!(arrow.hit_damage(1.1), 6.0);
    assert_eq!(arrow.hit_damage(2.0), 8.0);

    let snowball = Projectile::spawn(ProjectileKind::Snowball, 4, None, 0.0, 0.0, 0.0, [0.0; 3]);
    assert_eq!(snowball.hit_damage(5.0), 0.0);

    let egg = Projectile::spawn(ProjectileKind::Egg, 5, None, 0.0, 0.0, 0.0, [0.0; 3]);
    assert_eq!(egg.hit_damage(5.0), 0.0);
}

#[test]
fn try_hit_matches_the_first_target() {
    let mut projectile = Projectile::spawn(
        ProjectileKind::Arrow,
        6,
        None,
        0.5,
        64.5,
        0.5,
        [1.0, 0.0, 0.0],
    );
    let aabb = Aabb::new(0.0, 64.0, 0.0, 1.0, 65.0, 1.0);
    let hit = projectile.try_hit(&[(7, aabb)]).unwrap();
    assert_eq!(hit.target_id, 7);
    assert_eq!(hit.damage, 2.0);
    assert!(!hit.critical);
    assert!(projectile.stuck);
    assert_eq!(projectile.velocity, [0.0, 0.0, 0.0]);
}

#[test]
fn try_hit_inflates_the_target_box() {
    let mut projectile =
        Projectile::spawn(ProjectileKind::Arrow, 7, None, 1.2, 64.5, 0.5, [0.0; 3]);
    let aabb = Aabb::new(0.0, 64.0, 0.0, 1.0, 65.0, 1.0);
    assert!(projectile.try_hit(&[(9, aabb)]).is_some());
}

#[test]
fn swept_arrow_hits_a_target_it_passes_through() {
    let mut projectile = Projectile::spawn(
        ProjectileKind::Arrow,
        20,
        None,
        0.0,
        64.5,
        0.5,
        [10.0, 0.0, 0.0],
    );
    let aabb = Aabb::new(2.0, 64.0, 0.0, 3.0, 65.0, 1.0);
    let mut collider = FlatCollider { ground_y: -1000 };
    let tick = projectile.tick(&mut collider);
    assert!(tick.moved);
    let hit = projectile.try_hit(&[(21, aabb)]);
    assert!(hit.is_some());
    let hit = hit.unwrap();
    assert_eq!(hit.target_id, 21);
    assert!(projectile.stuck);
}

#[test]
fn zero_velocity_projectile_inside_inflated_box_hits() {
    let mut projectile =
        Projectile::spawn(ProjectileKind::Arrow, 22, None, 1.2, 64.5, 0.5, [0.0; 3]);
    let aabb = Aabb::new(0.0, 64.0, 0.0, 1.0, 65.0, 1.0);
    assert!(projectile.try_hit(&[(23, aabb)]).is_some());
}

#[test]
fn stuck_projectile_does_not_move() {
    let mut projectile = Projectile::spawn(
        ProjectileKind::Arrow,
        8,
        None,
        0.5,
        64.5,
        0.5,
        [1.0, 2.0, 3.0],
    );
    projectile.stuck = true;
    let velocity = projectile.velocity;
    let position = (projectile.x, projectile.y, projectile.z);
    let mut collider = FlatCollider { ground_y: -1000 };
    let tick = projectile.tick(&mut collider);
    assert!(!tick.moved);
    assert!(!tick.hit_block);
    assert_eq!(projectile.velocity, velocity);
    assert_eq!((projectile.x, projectile.y, projectile.z), position);
}

#[test]
fn projectiles_despawn_at_lifetime() {
    let mut projectile = Projectile::spawn(ProjectileKind::Arrow, 9, None, 0.0, 0.0, 0.0, [0.0; 3]);
    projectile.age = 1199;
    assert!(!projectile.should_despawn());

    let mut projectile =
        Projectile::spawn(ProjectileKind::Arrow, 10, None, 0.0, 0.0, 0.0, [0.0; 3]);
    projectile.age = 1200;
    assert!(projectile.should_despawn());
}

#[test]
fn packet_builders_use_registry_type_ids() {
    let projectile = Projectile::spawn(ProjectileKind::Arrow, 1, None, 0.0, 0.0, 0.0, [0.0; 3]);
    let add = projectile.add_packet();
    assert_eq!(add[0], 1);
    assert_eq!(
        ProjectileKind::Arrow.entity_type_id(),
        folia_core::registry::entity_type_id("minecraft:arrow").unwrap()
    );
    assert_eq!(projectile.metadata_packet()[0], 97);
}

#[test]
fn projectile_store_spawn_emits_add_and_metadata() {
    let mut store = ProjectileStore::new();
    let packets = store.spawn(ProjectileKind::Arrow, None, [0.0, 64.0, 0.0], [0.0; 3]);
    assert_eq!(store.len(), 1);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0][0], 1);
    assert_eq!(packets[1][0], 97);
}

#[test]
fn projectile_store_tick_emits_position_sync() {
    let mut store = ProjectileStore::new();
    store.spawn(
        ProjectileKind::Arrow,
        None,
        [0.5, 64.5, 0.5],
        [1.0, 0.0, 0.0],
    );
    let mut collider = FlatCollider { ground_y: -1000 };
    let tick = store.tick(&mut collider, &[]);
    assert_eq!(tick.hits.len(), 0);
    assert_eq!(tick.removed.len(), 0);
    assert!(!tick.packets.is_empty());
}

#[test]
fn projectile_store_try_hit_removes_and_reports() {
    let mut store = ProjectileStore::new();
    store.spawn(ProjectileKind::Arrow, None, [0.5, 64.5, 0.5], [0.0; 3]);
    let aabb = Aabb::new(0.0, 64.0, 0.0, 1.0, 65.0, 1.0);
    let mut collider = FlatCollider { ground_y: -1000 };
    let tick = store.tick(&mut collider, &[(7, aabb)]);
    assert_eq!(tick.hits.len(), 1);
    assert_eq!(tick.hits[0].target_id, 7);
    assert_eq!(tick.removed.len(), 1);
    assert!(tick.packets.iter().any(|packet| packet[0] == 75));
    assert!(store.is_empty());
}

#[test]
fn projectile_store_despawn_removal() {
    let mut store = ProjectileStore::new();
    store.spawn(ProjectileKind::Arrow, None, [0.0, 0.0, 0.0], [0.0; 3]);
    let mut collider = FlatCollider { ground_y: -1000 };
    let mut removed = false;
    for _ in 0..1201 {
        let tick = store.tick(&mut collider, &[]);
        if !tick.removed.is_empty() {
            removed = true;
            break;
        }
    }
    assert!(removed);
    assert!(store.is_empty());
}

#[test]
fn projectile_store_clear() {
    let mut store = ProjectileStore::new();
    store.spawn(ProjectileKind::Arrow, None, [0.0, 0.0, 0.0], [0.0; 3]);
    store.clear();
    assert!(store.is_empty());
}
