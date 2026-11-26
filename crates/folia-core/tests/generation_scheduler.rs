use folia_core::chunk_packet::ChunkPacketTemplate;
use folia_core::dimension::generator_dimension;
use folia_core::generation_scheduler::{
    ChunkGenerator, GenerationError, GenerationKey, GenerationScheduler, RequestPriority,
};
use folia_core::join::{generate_join_column, join_generation_scheduler};
use folia_core::world::Dimension;
use folia_core::worldgen::generator;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn key(x: i32, z: i32) -> GenerationKey {
    GenerationKey::overworld(1234, x, z)
}

fn priority(x: i32, z: i32) -> RequestPriority {
    RequestPriority::from_center(x, z, 0, 0)
}

fn spin_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for worker");
        thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Clone)]
struct GatedGenerator {
    started: Arc<Mutex<Vec<GenerationKey>>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl GatedGenerator {
    fn new() -> Self {
        Self {
            started: Arc::new(Mutex::new(Vec::new())),
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn release(&self) {
        let (lock, cv) = &*self.gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    fn starts(&self) -> Vec<GenerationKey> {
        self.started.lock().unwrap().clone()
    }
}

impl ChunkGenerator<i32> for GatedGenerator {
    fn generate(&self, key: GenerationKey) -> Result<i32, String> {
        self.started.lock().unwrap().push(key);
        let (lock, cv) = &*self.gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = cv.wait(released).unwrap();
        }
        Ok(key.x)
    }
}

#[test]
fn two_requesters_share_one_generation_and_one_result() {
    let generator = GatedGenerator::new();
    let scheduler = GenerationScheduler::new(generator.clone(), 1, 4);
    let requested = key(7, 2);
    let first = scheduler.try_request(requested, priority(7, 2)).unwrap();
    let second = scheduler.try_request(requested, priority(7, 2)).unwrap();

    assert_eq!(scheduler.in_flight_requests(), vec![requested]);
    spin_until(|| !generator.starts().is_empty());
    assert_eq!(generator.starts(), vec![requested]);

    generator.release();
    let first_result = first.wait().unwrap();
    let second_result = second.wait().unwrap();
    assert_eq!(first_result, second_result);
    assert_eq!(*first_result, 7);
    assert_eq!(generator.starts().len(), 1);
    assert_eq!(scheduler.generated_count(), 1);
    scheduler.shutdown();
}

struct RecordingGenerator(Arc<Mutex<Vec<GenerationKey>>>);

impl ChunkGenerator<i32> for RecordingGenerator {
    fn generate(&self, key: GenerationKey) -> Result<i32, String> {
        self.0.lock().unwrap().push(key);
        Ok(key.x)
    }
}

#[test]
fn queued_work_is_ordered_by_center_distance_then_z_then_x() {
    let scheduler = GenerationScheduler::new(|key: GenerationKey| Ok(key.x), 0, 8);
    let requested = [(3, 0), (-2, 0), (0, 0), (1, 0), (0, 1)];
    let mut requests = Vec::new();
    for &(x, z) in &requested {
        requests.push(
            scheduler
                .try_request(key(x, z), priority(x, z))
                .expect("queue has room"),
        );
    }
    let queued: Vec<_> = scheduler
        .queued_requests()
        .into_iter()
        .map(|(key, _)| (key.x, key.z))
        .collect();
    assert_eq!(queued, vec![(0, 0), (1, 0), (0, 1), (-2, 0), (3, 0)]);
    scheduler.shutdown();
}

#[test]
fn a_single_worker_serves_nearest_columns_first() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let scheduler = GenerationScheduler::new(RecordingGenerator(Arc::clone(&order)), 1, 8);
    let requested = [(3, 0), (-2, 0), (0, 0), (1, 0), (0, 1)];
    let mut requests = Vec::new();
    for &(x, z) in &requested {
        requests.push(scheduler.try_request(key(x, z), priority(x, z)).unwrap());
    }
    for request in requests {
        request.wait().unwrap();
    }
    let served: Vec<_> = order
        .lock()
        .unwrap()
        .iter()
        .map(|key| (key.x, key.z))
        .collect();
    assert_eq!(served, vec![(0, 0), (1, 0), (0, 1), (-2, 0), (3, 0)]);
    scheduler.shutdown();
}

#[test]
fn cancellation_releases_queued_work_and_frees_capacity() {
    let scheduler = GenerationScheduler::new(|key: GenerationKey| Ok(key.x), 0, 2);
    let first = scheduler.try_request(key(1, 0), priority(1, 0)).unwrap();
    let _second = scheduler
        .try_request(key(0, 1), priority(0, 1))
        .expect("second slot");
    assert!(matches!(
        scheduler.try_request(key(2, 0), priority(2, 0)),
        Err(GenerationError::QueueFull)
    ));

    first.cancel();
    assert!(first.is_cancelled());
    assert_eq!(
        scheduler
            .queued_requests()
            .into_iter()
            .map(|(key, _)| (key.x, key.z))
            .collect::<Vec<_>>(),
        vec![(0, 1)]
    );
    assert!(matches!(first.wait(), Err(GenerationError::Cancelled)));
    scheduler
        .try_request(key(2, 0), priority(2, 0))
        .expect("cancelled work released the slot");
    scheduler.shutdown();
}

#[test]
fn cancelling_one_waiter_keeps_the_shared_job_for_the_other() {
    let scheduler = GenerationScheduler::new(|key: GenerationKey| Ok(key.x), 1, 4);
    let first = scheduler.try_request(key(4, 1), priority(4, 1)).unwrap();
    let second = scheduler.try_request(key(4, 1), priority(4, 1)).unwrap();

    first.cancel();
    assert_eq!(scheduler.queued_requests().len(), 1);
    assert_eq!(*second.wait().unwrap(), 4);
    assert_eq!(scheduler.generated_count(), 1);
    scheduler.shutdown();
}

#[test]
fn cancelling_an_in_flight_job_wakes_its_waiter_without_requeuing() {
    let generator = GatedGenerator::new();
    let scheduler = GenerationScheduler::new(generator.clone(), 1, 4);
    let request = scheduler.try_request(key(9, 0), priority(9, 0)).unwrap();
    spin_until(|| !generator.starts().is_empty());

    request.cancel();
    assert!(matches!(request.wait(), Err(GenerationError::Cancelled)));
    assert!(scheduler.queued_requests().is_empty());
    assert!(scheduler.in_flight_requests().is_empty());

    generator.release();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(scheduler.generated_count(), 1);
    scheduler.shutdown();
}

#[test]
fn a_bounded_queue_applies_backpressure_and_dedupe_does_not_take_a_slot() {
    let scheduler = GenerationScheduler::new(|key: GenerationKey| Ok(key.x), 0, 2);
    let first = scheduler.try_request(key(1, 1), priority(1, 1)).unwrap();
    let _second = scheduler.try_request(key(2, 2), priority(2, 2)).unwrap();
    assert_eq!(scheduler.queued_requests().len(), 2);
    scheduler
        .try_request(key(1, 1), priority(1, 1))
        .expect("a duplicate must reuse the in-flight job");
    assert_eq!(scheduler.queued_requests().len(), 2);
    assert!(matches!(
        scheduler.try_request(key(3, 3), priority(3, 3)),
        Err(GenerationError::QueueFull)
    ));
    drop(first);
    assert_eq!(scheduler.queued_requests().len(), 1);
    scheduler.shutdown();
}

#[test]
fn a_completed_job_is_reused_from_the_bounded_cache() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let scheduler = GenerationScheduler::new(RecordingGenerator(Arc::clone(&order)), 1, 2);
    let first = scheduler.request(key(5, 5), priority(5, 5)).unwrap();
    assert_eq!(*first.wait().unwrap(), 5);
    let second = scheduler.request(key(5, 5), priority(5, 5)).unwrap();
    assert_eq!(*second.wait().unwrap(), 5);
    assert_eq!(scheduler.generated_count(), 1);
    assert_eq!(order.lock().unwrap().len(), 1);
    scheduler.shutdown();
}

#[test]
fn join_scheduler_generation_matches_direct_generation_byte_for_byte() {
    generator::configure(4242).expect("configure generator");
    let requested = GenerationKey::overworld(4242, 2, -3);
    let scheduler = GenerationScheduler::new(generate_join_column, 2, 4);
    let generated = scheduler
        .request(requested, priority(2, -3))
        .unwrap()
        .wait()
        .unwrap();

    let chunk =
        generator::generate_dimension(generator_dimension(Dimension::Overworld), 2, -3, 4242)
            .expect("direct generation");
    let packet = ChunkPacketTemplate::build_dynamic(&chunk).expect("direct packet");
    assert_eq!(generated.chunk, chunk, "blocks and biomes are unchanged");
    assert_eq!(
        generated.packet, packet,
        "the chunk packet is byte-identical"
    );
    scheduler.shutdown();
}

#[test]
fn overlapping_join_requesters_generate_a_column_once() {
    generator::configure(4242).expect("configure generator");
    let scheduler = join_generation_scheduler();
    let requested = GenerationKey::overworld(4242, 31, -27);
    let before = scheduler.generated_count();
    let first = scheduler.try_request(requested, priority(31, -27)).unwrap();
    let second = scheduler.try_request(requested, priority(31, -27)).unwrap();

    let first_result = first.wait().unwrap();
    let second_result = second.wait().unwrap();
    assert_eq!(first_result, second_result);
    assert_eq!(scheduler.generated_count(), before + 1);
    assert_eq!(first_result.chunk.x, 31);
    assert_eq!(first_result.chunk.z, -27);
}
