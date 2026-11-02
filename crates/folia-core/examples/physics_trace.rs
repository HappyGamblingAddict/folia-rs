//! Print the Rust armor-stand fall positions used by the Java differential
//! test (`tests/entity_physics.rs`).
//!
//! Run with:
//! `cargo run -p folia-core --example physics_trace`

use folia_core::entity::{BlockCollider, Entity, FlatCollider};

fn main() {
    let mut entity = Entity::armor_stand(2, [0; 16], 0.0, 50.0, 0.0);
    let mut collider = FlatCollider { ground_y: -60 };

    for tick in 1..=64 {
        entity.tick_physics(&mut collider);
        if (8..=59).contains(&tick) && (tick - 8) % 3 == 0 {
            println!("tick {tick:2}: y={:.6}", entity.y);
        }
    }

    println!(
        "landed: y={:.6} on_ground={} velocity_y={:.6}",
        entity.y, entity.on_ground, entity.velocity[1]
    );
    assert!(!collider.is_solid(0, -60, 0));
    assert!(collider.is_solid(0, -61, 0));
}
