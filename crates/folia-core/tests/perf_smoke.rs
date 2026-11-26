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
use folia_core::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use folia_core::chunk_packet::{ChunkPacketTemplate, flat_chunk};
use folia_core::entity_ai::{ActorView, AiDriver, MemoryWorld, PathOptions, find_path};
use folia_core::light;
use folia_core::math::{BlockPos, Vec3};
use folia_core::tick;
use folia_core::world::WorldStorage;
use folia_core::worldgen::diagnostics::chunk_digest;
use folia_core::worldgen::feature::{
    FeatureData, FeatureLevel, WorldGenRegion, scan_motion_blocking_top,
};
use folia_core::worldgen::generator::WorldGenerator;
use folia_random::LegacyRandomSource;
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
#[ignore = "focused release benchmark; run explicitly with --ignored"]
fn ore_placement_smoke() {
    const CASES: &[(&str, i32, i32, i32, i64)] = &[
        ("minecraft:ore_coal", 3, -4, 5, 11),
        ("minecraft:ore_iron", 6, -28, 4, 12),
        ("minecraft:ore_copper_small", 9, -12, 9, 13),
        ("minecraft:ore_gold_buried", 12, -20, 7, 14),
        ("minecraft:ore_diamond_medium", 5, -32, 11, 15),
        ("minecraft:ore_redstone", 10, -36, 3, 16),
        ("minecraft:ore_lapis_buried", 14, -8, 13, 17),
    ];
    const SAMPLES: usize = 15;
    const BATCHES: usize = 64;

    let generator = WorldGenerator::overworld(12_345).expect("overworld generator");
    let base = generator
        .generate_chunk_with_steps(0, 0, &[])
        .expect("terrain-only chunk");
    let data = FeatureData::vanilla();
    let features: Vec<_> = CASES
        .iter()
        .map(|&(id, _, _, _, _)| {
            data.configured(id)
                .unwrap_or_else(|| panic!("{id} should be supported"))
        })
        .collect();
    let mut chunks = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        chunks.push(base.clone());
    }

    let mut timings = Vec::with_capacity(SAMPLES);
    let mut digest = 0u64;
    let mut placements = 0usize;
    for mut chunk in chunks {
        let start = Instant::now();
        let mut successes = 0usize;
        for batch in 0..BATCHES {
            for (index, &(_, x, y, z, seed)) in CASES.iter().enumerate() {
                let mut region =
                    WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());
                let mut random = LegacyRandomSource::new(seed + batch as i64);
                successes += usize::from(
                    black_box(features[index].place(
                        &mut region,
                        &mut random,
                        BlockPos::new(x, y, z),
                    ))
                    .expect("ore feature places"),
                );
            }
        }
        let elapsed = start.elapsed();
        placements += successes;
        digest ^= chunk_digest(black_box(&chunk));
        timings.push(elapsed);
        guard("ore_placement_smoke", elapsed);
    }

    timings.sort_unstable();
    let median = timings[SAMPLES / 2];
    eprintln!(
        "ore_placement_smoke: {} samples of {} placements in {median:?} ({:.2} us/placement), min {:?}, max {:?}, placements={placements}, digest={digest:#018x}",
        SAMPLES,
        CASES.len() * BATCHES,
        median.as_secs_f64() * 1_000_000.0 / (CASES.len() * BATCHES) as f64,
        timings[0],
        timings[SAMPLES - 1],
    );
    assert!(
        placements > 0,
        "benchmark must execute successful ore placements"
    );
    assert_eq!(digest, 0x3a73_cac7_98af_d1c2);
}

#[test]
fn ore_height_cache_smoke() {
    const ROUNDS: usize = 32;
    const THRESHOLD: i32 = 0;

    let generator = WorldGenerator::overworld(12_345).expect("overworld generator");
    let mut chunk = generator
        .generate_chunk_with_steps(0, 0, &[])
        .expect("terrain-only chunk");
    let region = WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());
    let queries = ROUNDS * 16 * 16;

    let mut uncached_digest = 0u64;
    let start = Instant::now();
    for _ in 0..ROUNDS {
        for x in 0..16 {
            for z in 0..16 {
                let top = black_box(scan_motion_blocking_top(&region, x, z));
                let hit = THRESHOLD <= generator.min_y() || top.is_some_and(|top| top >= THRESHOLD);
                uncached_digest =
                    uncached_digest.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(hit);
            }
        }
    }
    let uncached = start.elapsed();

    let mut cached_digest = 0u64;
    let start = Instant::now();
    for _ in 0..ROUNDS {
        for x in 0..16 {
            for z in 0..16 {
                let hit = black_box(region.has_motion_blocking_at_or_above(x, z, THRESHOLD));
                cached_digest = cached_digest.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(hit);
            }
        }
    }
    let cached = start.elapsed();

    eprintln!(
        "ore_height_cache_smoke: {queries} threshold queries, cached {cached:?} ({:.2} ns/query), uncached scan {uncached:?} ({:.2} ns/query), {} cached columns",
        cached.as_nanos() as f64 / queries as f64,
        uncached.as_nanos() as f64 / queries as f64,
        region.motion_blocking_cache_len()
    );
    guard("ore_height_cache_smoke cached", cached);
    guard("ore_height_cache_smoke uncached", uncached);
    assert_eq!(cached_digest, uncached_digest, "cache must be transparent");
    assert_eq!(region.motion_blocking_cache_len(), 16 * 16);
}

/// Region construction and cold column scans were repeated once per placed
/// feature. Reusing one region and resetting only the per-placement outputs
/// keeps the same blocks and answers while amortising both costs.
#[test]
fn region_reuse_smoke() {
    const PLACEMENTS: usize = 4096;
    const MIX: u64 = 0x0000_0100_0000_01b3;

    let generator = WorldGenerator::overworld(12_345).expect("overworld generator");
    let base = generator
        .generate_chunk_with_steps(0, 0, &[])
        .expect("terrain-only chunk");
    // Writes stay in the x/z 8..16 quadrant so the probed column (0, 0) is
    // never invalidated: a reused region should serve it from the cache.
    let position = |index: usize| {
        BlockPos::new(
            8 + (index % 8) as i32,
            generator.min_y() + (index % 8) as i32,
            8 + ((index / 8) % 8) as i32,
        )
    };

    let mut fresh_chunk = base.clone();
    let mut fresh_digest = 0u64;
    let start = Instant::now();
    for index in 0..PLACEMENTS {
        let pos = position(index);
        let mut region =
            WorldGenRegion::new(&mut fresh_chunk, generator.min_y(), generator.height());
        let placed = region.set_block(pos, state("minecraft:stone"));
        let blocked = region.has_motion_blocking_at_or_above(0, 0, 0);
        fresh_digest = fresh_digest.wrapping_mul(MIX) ^ u64::from(placed && blocked);
    }
    let fresh = start.elapsed();

    let mut shared_chunk = base.clone();
    let mut shared_digest = 0u64;
    let shared_cache_columns;
    let start = Instant::now();
    {
        let mut region =
            WorldGenRegion::new(&mut shared_chunk, generator.min_y(), generator.height());
        for index in 0..PLACEMENTS {
            region.reset_feature_output();
            let pos = position(index);
            let placed = region.set_block(pos, state("minecraft:stone"));
            let blocked = region.has_motion_blocking_at_or_above(0, 0, 0);
            shared_digest = shared_digest.wrapping_mul(MIX) ^ u64::from(placed && blocked);
        }
        shared_cache_columns = region.motion_blocking_cache_len();
    }
    let shared = start.elapsed();

    eprintln!(
        "region_reuse_smoke: {PLACEMENTS} placements, fresh regions {fresh:?} ({:.2} ns/placement), one reused region {shared:?} ({:.2} ns/placement), {} cached columns",
        fresh.as_nanos() as f64 / PLACEMENTS as f64,
        shared.as_nanos() as f64 / PLACEMENTS as f64,
        shared_cache_columns
    );
    guard("region_reuse_smoke fresh", fresh);
    guard("region_reuse_smoke shared", shared);
    assert_eq!(fresh_digest, shared_digest, "cache must be transparent");
    assert_eq!(chunk_digest(&fresh_chunk), chunk_digest(&shared_chunk));
}

fn light_invalidation_fixture(with_light: bool) -> Chunk {
    let air = state("minecraft:air");
    let mut sections = Vec::with_capacity(24);
    for y in -4i8..=19 {
        sections.push(ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![air.clone()],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: folia_core::light::SectionLight {
                sky: with_light.then(|| folia_core::light::DataLayer::filled(15)),
                block: with_light.then(folia_core::light::DataLayer::new),
            },
        });
    }
    Chunk {
        status: folia_core::chunk::ChunkStatus::Full,
        light_present: with_light,
        x: 0,
        z: 0,
        sections,
    }
}

#[test]
fn light_invalidation_smoke() {
    let writes = 200_000usize;
    let stone = state("minecraft:stone");
    let air = air();

    let mut no_light = light_invalidation_fixture(false);
    assert!(!no_light.has_light());
    let start = Instant::now();
    for i in 0..writes {
        let y = -64 + (i % 384) as i32;
        let state = if i % 2 == 0 { &stone } else { &air };
        black_box(no_light.set_block_state_ref(i % 16, y, (i / 16) % 16, state));
    }
    let cold = start.elapsed();

    // Simulate the pre-optimization path: an unconditional whole-chunk clear
    // on every write, even though the chunk stores no light.
    let mut forced = light_invalidation_fixture(false);
    let start = Instant::now();
    for i in 0..writes {
        let y = -64 + (i % 384) as i32;
        let state = if i % 2 == 0 { &stone } else { &air };
        black_box(forced.set_block_state_ref(i % 16, y, (i / 16) % 16, state));
        forced.clear_light();
    }
    let forced_clear = start.elapsed();

    let invalidations = 256usize;
    let mut lit_chunks: Vec<Chunk> = (0..invalidations)
        .map(|_| light_invalidation_fixture(true))
        .collect();
    assert!(lit_chunks.iter().all(|chunk| chunk.has_light()));
    let start = Instant::now();
    for chunk in &mut lit_chunks {
        black_box(chunk.set_block_state_ref(8, -60, 8, &stone));
    }
    let warm = start.elapsed();

    eprintln!(
        "light_invalidation_smoke: {writes} writes without light {cold:?} ({:.2} ns/write) vs forced whole-chunk clear {forced_clear:?} ({:.2} ns/write); {invalidations} invalidation writes with light {warm:?} ({:.2} ns/write)",
        cold.as_nanos() as f64 / writes as f64,
        forced_clear.as_nanos() as f64 / writes as f64,
        warm.as_nanos() as f64 / invalidations as f64
    );
    guard("light_invalidation_smoke cold", cold);
    guard("light_invalidation_smoke forced", forced_clear);
    guard("light_invalidation_smoke warm", warm);
    assert!(!no_light.has_light(), "no-light writes never add light");
    assert!(
        lit_chunks.iter().all(|chunk| !chunk.has_light()),
        "writes invalidate existing light"
    );
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
