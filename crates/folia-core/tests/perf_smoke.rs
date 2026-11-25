//! Dependency-free performance *smoke* suite for representative hot paths.
//!
//! This is deliberately NOT a Criterion-style benchmark suite: no `benches/`
//! targets, no external dependency, no statistical sampling, and no per-test
//! regression budgets. Each test performs a small, fixed, deterministic amount
//! of work, prints the measured wall-clock time and a derived throughput to
//! stderr (`--nocapture` shows it), and completes. Timings are for humans and CI
//! logs; they are not asserted against a budget because a shared, loaded,
//! one-core CI host makes any tight bound flaky. The only assertion tied to time
//! is a deliberately generous 120 s guard per phase that can only trip on a
//! genuine hang or a catastrophic regression.
//!
//! Cover the P11.4 hot paths that have a simple public API: chunk generation,
//! lighting, the world tick loop, entity AI, pathfinding, and packet encoding.
//! Memory-per-player/chunk and cold-login latency are intentionally omitted:
//! there is no public allocation-tracking API and no full session fixture that
//! can be timed without network I/O, so a print-only number there would be
//! misleading. See `docs/ACCEPTANCE_GATES.md` for the still-unknown 20 TPS and
//! cold-login performance gates.

use folia_core::anvil::Compression;
use folia_core::block_update::block_update_packet;
use folia_core::chunk::BlockState;
use folia_core::chunk_packet::{ChunkPacketTemplate, flat_chunk};
use folia_core::entity_ai::{ActorView, AiDriver, MemoryWorld, PathOptions, find_path};
use folia_core::light;
use folia_core::math::{BlockPos, Vec3};
use folia_core::tick;
use folia_core::world::WorldStorage;
use folia_core::worldgen::feature::{FeatureLevel, WorldGenRegion};
use folia_core::worldgen::generator::WorldGenerator;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A hang guard, not a performance budget. Every phase below finishes far
/// quicker than this on a debug build; the assertion exists only so a deadlock
/// or an accidental O(n^3) loop fails the test instead of stalling CI forever.
const HANG_GUARD: Duration = Duration::from_secs(120);

fn guard(label: &str, elapsed: Duration) {
    assert!(
        elapsed < HANG_GUARD,
        "{label} took {elapsed:?}; expected under {HANG_GUARD:?} (hang guard, not a performance budget)"
    );
}

fn rate(count: usize, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64().max(1.0e-9)
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-perf-smoke-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn torch() -> BlockState {
    state("minecraft:torch")
}

fn air() -> BlockState {
    state("minecraft:air")
}

#[test]
fn chunk_generation_smoke() {
    let generator = WorldGenerator::overworld(12_345).expect("overworld generator");
    let coords = [(0, 0), (1, 0), (0, 1), (-1, -1)];

    let start = Instant::now();
    let mut sections = 0usize;
    for &(chunk_x, chunk_z) in &coords {
        let chunk = generator
            .generate_chunk(chunk_x, chunk_z)
            .expect("generated chunk");
        sections += chunk.sections.len();
    }
    let elapsed = start.elapsed();

    eprintln!(
        "chunk_generation_smoke: {} chunks / {} sections in {elapsed:?} ({:.2} chunks/s)",
        coords.len(),
        sections,
        rate(coords.len(), elapsed)
    );
    guard("chunk_generation_smoke", elapsed);
    assert!(
        sections >= coords.len(),
        "every generated chunk has sections"
    );
}

#[test]
fn feature_block_lookup_smoke() {
    let generator = WorldGenerator::overworld(12_345).expect("overworld generator");
    let mut chunk = generator
        .generate_chunk_with_steps(0, 0, &[])
        .expect("terrain-only chunk");
    let region = WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());

    let reads = 2_000_000usize;
    let start = Instant::now();
    let mut found = 0usize;
    for index in 0..reads {
        let x = ((index * 7) & 15) as i32;
        let z = ((index * 11) & 15) as i32;
        let y = generator.min_y() + ((index * 37) % generator.height() as usize) as i32;
        found += usize::from(black_box(region.get_block(BlockPos::new(x, y, z))).is_some());
    }
    let elapsed = start.elapsed();
    drop(region);

    fn linear_block_state(
        chunk: &folia_core::chunk::Chunk,
        x: usize,
        y: i32,
        z: usize,
    ) -> Option<&folia_core::chunk::BlockState> {
        if x >= 16 || z >= 16 {
            return None;
        }
        let section_y = y.div_euclid(16);
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&section_y) {
            return None;
        }
        let section = chunk
            .sections
            .iter()
            .find(|section| section.y == section_y as i8)?;
        let index = (y.rem_euclid(16) as usize * 16 + z) * 16 + x;
        section.block_states.get(index)
    }

    let start = Instant::now();
    let mut linear_found = 0usize;
    for index in 0..reads {
        let x = (index * 7) & 15;
        let z = (index * 11) & 15;
        let y = generator.min_y() + ((index * 37) % generator.height() as usize) as i32;
        linear_found += usize::from(black_box(linear_block_state(&chunk, x, y, z)).is_some());
    }
    let linear_elapsed = start.elapsed();

    eprintln!(
        "feature_block_lookup_smoke: {reads} generated-chunk reads in {elapsed:?} ({:.2} ns/read), linear-section reference {linear_elapsed:?} ({:.2} ns/read)",
        elapsed.as_nanos() as f64 / reads as f64,
        linear_elapsed.as_nanos() as f64 / reads as f64
    );
    guard("feature_block_lookup_smoke", elapsed);
    guard(
        "feature_block_lookup_smoke linear reference",
        linear_elapsed,
    );
    assert_eq!(found, reads, "every generated-chunk read resolves a state");
    assert_eq!(linear_found, found, "lookup paths resolve identical states");
}

#[test]
fn lighting_smoke() {
    let mut chunk = flat_chunk(0, 0);
    let start = Instant::now();
    light::recompute_chunk_light(&mut chunk);
    let full = start.elapsed();
    eprintln!("lighting_smoke: one full chunk light recompute in {full:?}");
    guard("lighting_smoke full recompute", full);
    assert!(chunk.has_light(), "recompute materializes light layers");

    let mut center = flat_chunk(0, 0);
    let mut neighbours = [
        flat_chunk(1, 0),
        flat_chunk(-1, 0),
        flat_chunk(0, 1),
        flat_chunk(0, -1),
    ];
    let mut scene = light::LightScene::new(&mut center, &mut neighbours);
    scene.clear_dirty();

    let edits = 64usize;
    let start = Instant::now();
    for i in 0..edits {
        let index = i / 2;
        let x = (index % 16) as i32;
        let z = ((index / 16) % 16) as i32;
        let next = if i % 2 == 0 { torch() } else { air() };
        assert!(
            scene.set_block_state(x, -60, z, next),
            "edit {i} changed a block"
        );
    }
    let incremental = start.elapsed();

    eprintln!(
        "lighting_smoke: {edits} incremental light edits in {incremental:?} ({:.0} edits/s)",
        rate(edits, incremental)
    );
    guard("lighting_smoke incremental", incremental);
    assert!(!scene.dirty().is_empty(), "edits mark sections dirty");
}

#[test]
fn tick_loop_smoke() {
    let root = temp_dir("tick-loop");
    let mut storage = WorldStorage::new(root.join("region"));
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .expect("cache a small fixture chunk");
    storage.flush().expect("flush fixture");
    storage.set_tick_seed(4_242);

    let ticks = 100u64;
    let start = Instant::now();
    for tick in 1..=ticks {
        tick::tick_world(&mut storage, tick).expect("world tick");
    }
    let elapsed = start.elapsed();

    eprintln!(
        "tick_loop_smoke: {ticks} world ticks in {elapsed:?} ({:.0} ticks/s)",
        rate(ticks as usize, elapsed)
    );
    guard("tick_loop_smoke", elapsed);
    assert_eq!(storage.tick_state().current_tick, ticks);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entity_ai_smoke() {
    let mut world = MemoryWorld::flat(-64, 320, 63, 32);
    world.set_default_biome("minecraft:plains");
    world.set_default_sky_light(0);
    world.set_default_block_light(0);

    let mut driver = AiDriver::new();
    let zombie = driver.add_zombie(1, Vec3::new(0.5, 64.0, 0.5), 7);
    driver.add_player(ActorView::player(99, Vec3::new(8.5, 64.0, 0.5)));

    let steps = 200usize;
    let start = Instant::now();
    let mut events = 0usize;
    for _ in 0..steps {
        events += driver.tick(&world).len();
    }
    let elapsed = start.elapsed();

    eprintln!(
        "entity_ai_smoke: {steps} AI driver ticks in {elapsed:?} ({:.0} ticks/s, {events} events)",
        rate(steps, elapsed)
    );
    guard("entity_ai_smoke", elapsed);
    assert!(driver.agent(zombie).is_some(), "the zombie stays tracked");
}

#[test]
fn pathfinding_smoke() {
    let mut world = MemoryWorld::flat(-64, 320, 63, 32);
    world.set_default_biome("minecraft:plains");
    world.set_default_sky_light(0);
    world.set_default_block_light(0);
    for z in -2..=2 {
        for y in 64..=65 {
            world.set_block_name(2, y, z, "minecraft:stone");
        }
    }

    let runs = 50usize;
    let start = Instant::now();
    let mut nodes = 0usize;
    for _ in 0..runs {
        let path = find_path(
            &world,
            BlockPos::new(0, 64, 0),
            BlockPos::new(5, 64, 0),
            &PathOptions::default(),
        )
        .expect("the wall has a detour");
        nodes += path.points().len();
    }
    let elapsed = start.elapsed();

    eprintln!(
        "pathfinding_smoke: {runs} bounded A* searches in {elapsed:?} ({:.0} searches/s, {nodes} path nodes)",
        rate(runs, elapsed)
    );
    guard("pathfinding_smoke", elapsed);
    assert!(
        nodes >= runs,
        "every search produced at least one path node"
    );
}

#[test]
fn packet_encoding_smoke() {
    let chunk = flat_chunk(0, 0);

    let iterations = 200usize;
    let start = Instant::now();
    let mut bytes = 0usize;
    for _ in 0..iterations {
        let packet = ChunkPacketTemplate::build_dynamic(&chunk).expect("chunk packet");
        bytes += packet.len();
    }
    let elapsed = start.elapsed();
    eprintln!(
        "packet_encoding_smoke: {iterations} full chunk packets, {bytes} bytes in {elapsed:?} ({:.2} MB/s, {:.0} packets/s)",
        (bytes as f64 / elapsed.as_secs_f64().max(1.0e-9)) / (1024.0 * 1024.0),
        rate(iterations, elapsed)
    );
    guard("packet_encoding_smoke chunk packets", elapsed);
    assert!(bytes > 0, "encoded packets carry bytes");

    let iterations = 50_000usize;
    let start = Instant::now();
    let mut sink = 0usize;
    for i in 0..iterations {
        let i = i as i32;
        sink += block_update_packet(i % 16, -60, (i / 16) % 16, 1).len();
    }
    let elapsed = start.elapsed();
    eprintln!(
        "packet_encoding_smoke: {iterations} block-update packets in {elapsed:?} ({:.0} packets/s)",
        rate(iterations, elapsed)
    );
    guard("packet_encoding_smoke block updates", elapsed);
    assert!(sink > 0, "encoded block-update packets carry bytes");
}
