use folia_core::chunk::{Chunk, ChunkStatus};
use folia_core::chunk_status_pipeline::{ChunkCoord, ChunkStageRunner, ChunkStatusPipeline};
use folia_core::worldgen::diagnostics::chunk_digest;
use folia_core::worldgen::generator::{
    Dimension, GeneratorStageRunner, WorldGenerator, generate_dimension,
};
use std::cell::RefCell;
use std::io;
use std::rc::Rc;

const FULL_DIGESTS: [(i64, i32, i32, u64); 3] = [
    (12_345, 0, -1, 0x9976_5447_f433_a939),
    (0, 1, 0, 0x5fdf_c192_eb1c_8119),
    (-1, -2, 3, 0xc5c1_10a8_b9b9_3b5a),
];

struct CountingRunner<'a> {
    inner: GeneratorStageRunner<'a>,
    log: Rc<RefCell<Vec<ChunkStatus>>>,
}

impl ChunkStageRunner for CountingRunner<'_> {
    fn run_stage(
        &self,
        chunk: &mut Chunk,
        coord: ChunkCoord,
        status: ChunkStatus,
    ) -> io::Result<()> {
        self.log.borrow_mut().push(status);
        self.inner.run_stage(chunk, coord, status)
    }
}

fn counted_pipeline<'a>(
    generator: &'a WorldGenerator,
    log: &Rc<RefCell<Vec<ChunkStatus>>>,
) -> ChunkStatusPipeline<CountingRunner<'a>> {
    let runner = CountingRunner {
        inner: GeneratorStageRunner::new(generator, &WorldGenerator::DEFAULT_DECORATION_STEPS),
        log: Rc::clone(log),
    };
    ChunkStatusPipeline::new(runner)
}

#[test]
fn generate_chunk_through_pipeline_preserves_block_digests() {
    for (seed, x, z, expected) in FULL_DIGESTS {
        let generator = WorldGenerator::overworld(seed).unwrap();
        let chunk = generator.generate_chunk(x, z).unwrap();
        assert_eq!(chunk.status(), ChunkStatus::Full, "seed {seed}");
        assert_eq!(
            chunk_digest(&chunk),
            expected,
            "seed {seed} chunk ({x},{z})"
        );
    }
}

#[test]
fn a_partial_chunk_resumes_to_full_without_repeating_stages() {
    let generator = WorldGenerator::overworld(12_345).unwrap();
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut pipeline = counted_pipeline(&generator, &log);

    let mut chunk = generator.empty_chunk_at(0, -1);
    let first = pipeline.upgrade(&mut chunk, ChunkStatus::Noise).unwrap();
    assert_eq!(first.reached, ChunkStatus::Noise);
    assert_eq!(chunk.status(), ChunkStatus::Noise);
    let noise_digest = chunk_digest(&chunk);
    assert_ne!(noise_digest, FULL_DIGESTS[0].3);

    log.borrow_mut().clear();
    let second = pipeline.upgrade_to_full(&mut chunk).unwrap();
    assert_eq!(second.from, ChunkStatus::Noise);
    assert_eq!(second.reached, ChunkStatus::Full);
    assert_eq!(chunk.status(), ChunkStatus::Full);
    assert!(!second.completed.contains(&ChunkStatus::Noise));
    assert!(!second.completed.contains(&ChunkStatus::Biomes));
    assert!(!log.borrow().contains(&ChunkStatus::Noise));
    assert!(!log.borrow().contains(&ChunkStatus::Biomes));
    assert_eq!(chunk_digest(&chunk), FULL_DIGESTS[0].3);
}

#[test]
fn cancellation_checkpoints_and_resume_continues_from_there() {
    let generator = WorldGenerator::overworld(0).unwrap();
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut pipeline = counted_pipeline(&generator, &log);
    let mut chunk = generator.empty_chunk_at(1, 0);

    let first = pipeline.upgrade(&mut chunk, ChunkStatus::Biomes).unwrap();
    assert_eq!(first.reached, ChunkStatus::Biomes);
    assert_eq!(
        log.borrow().as_slice(),
        &[
            ChunkStatus::StructureStarts,
            ChunkStatus::StructureReferences,
            ChunkStatus::Biomes,
        ]
    );

    log.borrow_mut().clear();
    pipeline.cancel();
    let stopped = pipeline.upgrade_to_full(&mut chunk).unwrap();
    assert!(stopped.cancelled);
    assert!(stopped.completed.is_empty());
    assert_eq!(chunk.status(), ChunkStatus::Biomes);
    assert!(log.borrow().is_empty());

    let resumed = pipeline.resume(&mut chunk, ChunkStatus::Full).unwrap();
    assert_eq!(resumed.from, ChunkStatus::Biomes);
    assert_eq!(resumed.reached, ChunkStatus::Full);
    assert!(!resumed.completed.contains(&ChunkStatus::Biomes));
    assert_eq!(chunk_digest(&chunk), FULL_DIGESTS[1].3);
}

#[test]
fn generate_chunk_to_status_is_resumable_with_upgrade_chunk_to() {
    let generator = WorldGenerator::overworld(-1).unwrap();
    let mut chunk = generator
        .generate_chunk_to_status(-2, 3, ChunkStatus::Surface)
        .unwrap();
    assert_eq!(chunk.status(), ChunkStatus::Surface);
    let report = generator
        .upgrade_chunk_to(&mut chunk, ChunkStatus::Full)
        .unwrap();
    assert_eq!(report.from, ChunkStatus::Surface);
    assert!(!report.ran(ChunkStatus::Surface));
    assert_eq!(chunk.status(), ChunkStatus::Full);
    assert_eq!(chunk_digest(&chunk), FULL_DIGESTS[2].3);
}

#[test]
fn a_checkpointed_status_survives_anvil_round_trip() {
    let generator = WorldGenerator::overworld(12_345).unwrap();
    let chunk = generator
        .generate_chunk_to_status(0, -1, ChunkStatus::Carvers)
        .unwrap();
    let tag = folia_core::chunk::encode_chunk(&chunk).unwrap();
    assert_eq!(
        tag.get("Status").and_then(folia_core::nbt::Tag::as_str),
        Some("minecraft:carvers")
    );
    let mut restored = folia_core::chunk::decode_chunk(&tag).unwrap();
    assert_eq!(restored.status(), ChunkStatus::Carvers);
    let report = generator
        .upgrade_chunk_to(&mut restored, ChunkStatus::Full)
        .unwrap();
    assert_eq!(report.from, ChunkStatus::Carvers);
    assert_eq!(restored.status(), ChunkStatus::Full);
    assert_eq!(chunk_digest(&restored), FULL_DIGESTS[0].3);
}

#[test]
fn direct_dimension_generation_marks_chunks_full() {
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let chunk = generate_dimension(dimension, 0, 0, 7).unwrap();
        assert_eq!(chunk.status(), ChunkStatus::Full, "{dimension:?}");
    }
}
