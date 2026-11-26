//! Resumable chunk-status upgrade pipeline.
//!
//! [`ChunkStatusPipeline`] drives a chunk from the status it currently has to a
//! requested target, one [`ChunkStatus`] at a time. After each stage it records
//! the new status both on the [`Chunk`] and in an internal per-column map, so a
//! cancelled or failed upgrade resumes at the next stage instead of repeating
//! work that is already checkpointed.

use crate::chunk::Chunk;
use crate::chunk_status::ChunkStatus;
use std::collections::HashMap;
use std::io;

/// Chunk coordinates used as the stable key for checkpointed statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkCoord {
    pub x: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub const fn of(chunk: &Chunk) -> Self {
        Self {
            x: chunk.x,
            z: chunk.z,
        }
    }
}

impl From<&Chunk> for ChunkCoord {
    fn from(chunk: &Chunk) -> Self {
        Self::of(chunk)
    }
}

/// Executes a single generation stage on a chunk.
///
/// Implementations are stateless with respect to the pipeline; a stage may
/// use interior mutability for its own caches, but the authoritative progress
/// is the status recorded by the pipeline after the stage returns.
pub trait ChunkStageRunner {
    fn run_stage(
        &self,
        chunk: &mut Chunk,
        coord: ChunkCoord,
        status: ChunkStatus,
    ) -> io::Result<()>;
}

impl<R: ChunkStageRunner + ?Sized> ChunkStageRunner for &R {
    fn run_stage(
        &self,
        chunk: &mut Chunk,
        coord: ChunkCoord,
        status: ChunkStatus,
    ) -> io::Result<()> {
        (**self).run_stage(chunk, coord, status)
    }
}

/// A runner that performs no work. Useful for stages a dimension does not
/// implement yet, and for tests that only exercise checkpoint bookkeeping.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpStages;

impl ChunkStageRunner for NoOpStages {
    fn run_stage(
        &self,
        _chunk: &mut Chunk,
        _coord: ChunkCoord,
        _status: ChunkStatus,
    ) -> io::Result<()> {
        Ok(())
    }
}

/// Outcome of one [`ChunkStatusPipeline::upgrade`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeReport {
    pub coord: ChunkCoord,
    pub from: ChunkStatus,
    pub target: ChunkStatus,
    /// Stages actually executed and checkpointed during this call.
    pub completed: Vec<ChunkStatus>,
    /// Status reached when the call returned (the last checkpoint).
    pub reached: ChunkStatus,
    /// True when the chunk was already at or past the target.
    pub already_complete: bool,
    /// True when a cancellation stopped the upgrade before the target.
    pub cancelled: bool,
}

impl UpgradeReport {
    pub fn ran(&self, status: ChunkStatus) -> bool {
        self.completed.contains(&status)
    }
}

/// One recorded checkpoint in an audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    pub coord: ChunkCoord,
    pub status: ChunkStatus,
}

/// Drives chunks through named statuses, checkpointing after every stage.
#[derive(Debug)]
pub struct ChunkStatusPipeline<R> {
    runner: R,
    statuses: HashMap<ChunkCoord, ChunkStatus>,
    checkpoints: Vec<Checkpoint>,
    cancel_requested: bool,
}

impl<R: ChunkStageRunner> ChunkStatusPipeline<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            statuses: HashMap::new(),
            checkpoints: Vec::new(),
            cancel_requested: false,
        }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }

    pub fn runner_mut(&mut self) -> &mut R {
        &mut self.runner
    }

    /// Last recorded status for a column, or [`ChunkStatus::Empty`] when the
    /// pipeline has never seen it.
    pub fn status_at(&self, coord: ChunkCoord) -> ChunkStatus {
        self.statuses.get(&coord).copied().unwrap_or_default()
    }

    /// Status of a chunk in the pipeline, taking the further of the pipeline's
    /// checkpoint and the status carried on the chunk itself.
    pub fn status_of(&self, chunk: &Chunk) -> ChunkStatus {
        self.status_at(ChunkCoord::of(chunk)).max(chunk.status())
    }

    /// Directly record a status for a column and install it on the chunk.
    pub fn record(&mut self, chunk: &mut Chunk, status: ChunkStatus) {
        let coord = ChunkCoord::of(chunk);
        self.statuses.insert(coord, status);
        chunk.set_status(status);
        self.checkpoints.push(Checkpoint { coord, status });
    }

    /// Request cancellation. The current stage is allowed to finish; the next
    /// one is skipped and the chunk keeps the status of the last checkpoint.
    pub fn cancel(&mut self) {
        self.cancel_requested = true;
    }

    pub fn cancellation_requested(&self) -> bool {
        self.cancel_requested
    }

    /// Clear a pending cancellation so the pipeline can be reused.
    pub fn clear_cancel(&mut self) {
        self.cancel_requested = false;
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Upgrade a chunk to `target`, running only the stages after its current
    /// status. Earlier statuses are never re-executed.
    pub fn upgrade(&mut self, chunk: &mut Chunk, target: ChunkStatus) -> io::Result<UpgradeReport> {
        let coord = ChunkCoord::of(chunk);
        let from = self.status_of(chunk);
        let mut report = UpgradeReport {
            coord,
            from,
            target,
            completed: Vec::new(),
            reached: from,
            already_complete: false,
            cancelled: false,
        };
        if from.at_or_after(target) {
            report.already_complete = true;
            return Ok(report);
        }

        // `from` is already checkpointed, so its stage (and `empty`, which is
        // the initial state rather than an operation) are not run again.
        for status in ChunkStatus::steps_between(from, target)
            .into_iter()
            .filter(|status| *status != from && *status != ChunkStatus::Empty)
        {
            if self.cancel_requested {
                report.cancelled = true;
                break;
            }
            self.runner.run_stage(chunk, coord, status)?;
            self.record(chunk, status);
            report.completed.push(status);
            report.reached = status;
        }
        Ok(report)
    }

    /// Upgrade a chunk all the way to [`ChunkStatus::Full`].
    pub fn upgrade_to_full(&mut self, chunk: &mut Chunk) -> io::Result<UpgradeReport> {
        self.upgrade(chunk, ChunkStatus::Full)
    }

    /// Resume an interrupted upgrade. This is the same operation as calling
    /// [`Self::upgrade`] again with the same target: the current status is read
    /// back from the chunk/pipeline and only later stages run.
    pub fn resume(&mut self, chunk: &mut Chunk, target: ChunkStatus) -> io::Result<UpgradeReport> {
        self.clear_cancel();
        self.upgrade(chunk, target)
    }
}

/// Convenience wrapper: upgrade one chunk to a target with a temporary runner.
pub fn upgrade_chunk<R: ChunkStageRunner>(
    runner: &R,
    chunk: &mut Chunk,
    target: ChunkStatus,
) -> io::Result<UpgradeReport> {
    let mut pipeline = ChunkStatusPipeline::new(runner);
    pipeline.upgrade(chunk, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<(ChunkCoord, ChunkStatus)>>>;

    #[derive(Clone)]
    struct RecordingRunner {
        log: Log,
        fail_at: Option<ChunkStatus>,
    }

    impl ChunkStageRunner for RecordingRunner {
        fn run_stage(
            &self,
            _chunk: &mut Chunk,
            coord: ChunkCoord,
            status: ChunkStatus,
        ) -> io::Result<()> {
            if self.fail_at == Some(status) {
                return Err(io::Error::other("stage failed"));
            }
            self.log.borrow_mut().push((coord, status));
            Ok(())
        }
    }

    fn empty_chunk(x: i32, z: i32) -> Chunk {
        Chunk {
            status: ChunkStatus::Empty,
            x,
            z,
            sections: Vec::new(),
            light_present: false,
        }
    }

    #[test]
    fn upgrade_runs_each_stage_once_and_records_status_on_the_chunk() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: None,
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(3, -4);

        let report = pipeline.upgrade(&mut chunk, ChunkStatus::Surface).unwrap();
        assert_eq!(report.from, ChunkStatus::Empty);
        assert_eq!(report.reached, ChunkStatus::Surface);
        assert_eq!(chunk.status(), ChunkStatus::Surface);
        assert_eq!(pipeline.status_of(&chunk), ChunkStatus::Surface);
        assert_eq!(
            log.borrow().as_slice(),
            &[
                (ChunkCoord::new(3, -4), ChunkStatus::StructureStarts),
                (ChunkCoord::new(3, -4), ChunkStatus::StructureReferences),
                (ChunkCoord::new(3, -4), ChunkStatus::Biomes),
                (ChunkCoord::new(3, -4), ChunkStatus::Noise),
                (ChunkCoord::new(3, -4), ChunkStatus::Surface),
            ]
        );
        assert!(!pipeline.checkpoints().is_empty());
    }

    #[test]
    fn reaching_full_runs_every_non_empty_stage() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: None,
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(0, 0);
        let report = pipeline.upgrade_to_full(&mut chunk).unwrap();
        assert_eq!(report.reached, ChunkStatus::Full);
        assert_eq!(chunk.status(), ChunkStatus::Full);
        assert_eq!(log.borrow().len(), ChunkStatus::count() - 1);
        assert!(!report.ran(ChunkStatus::Empty));
        assert!(report.ran(ChunkStatus::Features));
        assert!(report.ran(ChunkStatus::Full));
    }

    #[test]
    fn resuming_from_a_partial_status_skips_earlier_stages() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: None,
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(1, 1);
        chunk.set_status(ChunkStatus::Noise);
        pipeline.record(&mut chunk, ChunkStatus::Noise);

        let report = pipeline.upgrade(&mut chunk, ChunkStatus::Features).unwrap();
        assert_eq!(report.from, ChunkStatus::Noise);
        assert_eq!(
            log.borrow().as_slice(),
            &[
                (ChunkCoord::new(1, 1), ChunkStatus::Surface),
                (ChunkCoord::new(1, 1), ChunkStatus::Carvers),
                (ChunkCoord::new(1, 1), ChunkStatus::Features),
            ]
        );
        assert_eq!(chunk.status(), ChunkStatus::Features);
    }

    #[test]
    fn cancellation_stops_at_last_checkpoint_and_resume_continues() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: None,
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(2, 2);

        let first = pipeline.upgrade(&mut chunk, ChunkStatus::Biomes).unwrap();
        assert_eq!(first.reached, ChunkStatus::Biomes);
        assert_eq!(chunk.status(), ChunkStatus::Biomes);

        pipeline.cancel();
        let second = pipeline.upgrade_to_full(&mut chunk).unwrap();
        assert!(second.cancelled);
        assert_eq!(second.completed.len(), 0);
        assert_eq!(chunk.status(), ChunkStatus::Biomes);

        let third = pipeline.resume(&mut chunk, ChunkStatus::Full).unwrap();
        assert!(!third.cancelled);
        assert_eq!(third.reached, ChunkStatus::Full);
        assert_eq!(third.from, ChunkStatus::Biomes);
        assert_eq!(chunk.status(), ChunkStatus::Full);
        // The three stages before Biomes were not repeated by either pass.
        let before_biomes = log
            .borrow()
            .iter()
            .filter(|(_, status)| status.is_before(ChunkStatus::Biomes))
            .count();
        assert_eq!(before_biomes, 2);
    }

    #[test]
    fn failure_keeps_the_last_checkpoint_for_resume() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: Some(ChunkStatus::Carvers),
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(5, 6);
        let error = pipeline.upgrade_to_full(&mut chunk).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(chunk.status(), ChunkStatus::Surface);
        assert_eq!(pipeline.status_of(&chunk), ChunkStatus::Surface);

        pipeline.runner_mut().fail_at = None;
        let report = pipeline.upgrade_to_full(&mut chunk).unwrap();
        assert_eq!(report.from, ChunkStatus::Surface);
        assert_eq!(report.reached, ChunkStatus::Full);
        assert!(!report.ran(ChunkStatus::Noise));
        assert!(report.ran(ChunkStatus::Carvers));
    }

    #[test]
    fn already_complete_chunks_run_no_stages() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let runner = RecordingRunner {
            log: Rc::clone(&log),
            fail_at: None,
        };
        let mut pipeline = ChunkStatusPipeline::new(runner);
        let mut chunk = empty_chunk(0, 0);
        chunk.set_status(ChunkStatus::Full);
        let report = pipeline.upgrade_to_full(&mut chunk).unwrap();
        assert!(report.already_complete);
        assert!(report.completed.is_empty());
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn downgrade_target_performs_no_work() {
        let pipeline = ChunkStatusPipeline::new(NoOpStages);
        let mut chunk = empty_chunk(0, 0);
        chunk.set_status(ChunkStatus::Features);
        let report = upgrade_chunk(&NoOpStages, &mut chunk, ChunkStatus::Noise).unwrap();
        assert_eq!(report.reached, ChunkStatus::Features);
        assert!(report.completed.is_empty());
        assert_eq!(pipeline.status_of(&chunk), ChunkStatus::Features);
    }
}
