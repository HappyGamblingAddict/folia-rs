use crate::world::Dimension;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

pub type GenerationResult<T> = Result<Arc<T>, GenerationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationError {
    Cancelled,
    QueueFull,
    Stopped,
    Failed(String),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "chunk generation was cancelled"),
            Self::QueueFull => write!(f, "chunk generation queue is full"),
            Self::Stopped => write!(f, "chunk generation scheduler is stopped"),
            Self::Failed(message) => write!(f, "chunk generation failed: {message}"),
        }
    }
}

impl std::error::Error for GenerationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationKey {
    pub dimension: Dimension,
    pub seed: i64,
    pub x: i32,
    pub z: i32,
}

impl GenerationKey {
    pub fn new(dimension: Dimension, seed: i64, x: i32, z: i32) -> Self {
        Self {
            dimension,
            seed,
            x,
            z,
        }
    }

    pub fn overworld(seed: i64, x: i32, z: i32) -> Self {
        Self::new(Dimension::Overworld, seed, x, z)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestPriority {
    pub distance_sq: u64,
    pub dz: i32,
    pub dx: i32,
}

impl RequestPriority {
    pub fn from_center(x: i32, z: i32, center_x: i32, center_z: i32) -> Self {
        let dx = x.wrapping_sub(center_x);
        let dz = z.wrapping_sub(center_z);
        let distance = i64::from(dx) * i64::from(dx) + i64::from(dz) * i64::from(dz);
        Self {
            distance_sq: distance.max(0) as u64,
            dz,
            dx,
        }
    }
}

pub trait ChunkGenerator<T>: Send + Sync + 'static {
    fn generate(&self, key: GenerationKey) -> Result<T, String>;
}

impl<T, F> ChunkGenerator<T> for F
where
    F: Fn(GenerationKey) -> Result<T, String> + Send + Sync + 'static,
{
    fn generate(&self, key: GenerationKey) -> Result<T, String> {
        self(key)
    }
}

#[derive(Debug, Clone)]
pub struct CancellationToken {
    id: u64,
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.flag.store(true, AtomicOrdering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(AtomicOrdering::Acquire)
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

struct JobSignal<T> {
    ready: Mutex<Option<GenerationResult<T>>>,
    cv: Condvar,
}

impl<T> JobSignal<T> {
    fn new() -> Self {
        Self {
            ready: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    fn with_result(result: GenerationResult<T>) -> Self {
        Self {
            ready: Mutex::new(Some(result)),
            cv: Condvar::new(),
        }
    }

    fn publish(&self, result: GenerationResult<T>) {
        let mut ready = self
            .ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if ready.is_none() {
            *ready = Some(result);
        }
        self.cv.notify_all();
    }

    fn peek(&self) -> Option<GenerationResult<T>> {
        self.ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn wait(&self) -> GenerationResult<T> {
        let mut ready = self
            .ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while ready.is_none() {
            ready = self
                .cv
                .wait(ready)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        ready.clone().expect("result was published before waking")
    }
}

struct Job<T> {
    key: GenerationKey,
    priority: RequestPriority,
    signal: Arc<JobSignal<T>>,
}

struct Queued<T> {
    priority: RequestPriority,
    seq: u64,
    job: Arc<Job<T>>,
}

impl<T> PartialEq for Queued<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.priority, self.seq) == (other.priority, other.seq)
    }
}

impl<T> Eq for Queued<T> {}

impl<T> PartialOrd for Queued<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Queued<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.priority, self.seq).cmp(&(other.priority, other.seq))
    }
}

struct InFlight<T> {
    job: Arc<Job<T>>,
    waiters: Vec<u64>,
    cancelled: bool,
}

struct State<T> {
    queue: BinaryHeap<Reverse<Queued<T>>>,
    in_flight: HashMap<GenerationKey, InFlight<T>>,
    completed: HashMap<GenerationKey, GenerationResult<T>>,
    completed_order: VecDeque<GenerationKey>,
    completed_capacity: usize,
    next_seq: u64,
    next_token: u64,
}

impl<T> State<T> {
    fn token(&mut self) -> CancellationToken {
        let id = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        CancellationToken {
            id,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    fn cache_put(&mut self, key: GenerationKey, result: GenerationResult<T>) {
        if self.completed_capacity == 0 {
            return;
        }
        if !self.completed.contains_key(&key) {
            self.completed_order.push_back(key);
            while self.completed_order.len() > self.completed_capacity {
                if let Some(old) = self.completed_order.pop_front() {
                    self.completed.remove(&old);
                }
            }
        }
        self.completed.insert(key, result);
    }

    fn drop_queued(&mut self, key: GenerationKey) {
        let queue = std::mem::take(&mut self.queue);
        self.queue = queue
            .into_iter()
            .filter(|Reverse(queued)| queued.job.key != key)
            .collect();
    }
}

struct Inner<T> {
    state: Mutex<State<T>>,
    work_cv: Condvar,
    space_cv: Condvar,
    generator: Box<dyn ChunkGenerator<T>>,
    capacity: usize,
    stopping: AtomicBool,
    generated: AtomicUsize,
}

fn lock<T>(inner: &Inner<T>) -> MutexGuard<'_, State<T>> {
    inner
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn worker<T: Send + Sync + 'static>(inner: Arc<Inner<T>>) {
    loop {
        let job = {
            let mut state = lock(&inner);
            loop {
                if inner.stopping.load(AtomicOrdering::Acquire) {
                    return;
                }
                if let Some(Reverse(queued)) = state.queue.pop() {
                    inner.space_cv.notify_all();
                    let active = state
                        .in_flight
                        .get(&queued.job.key)
                        .is_some_and(|entry| !entry.cancelled);
                    if active {
                        break Arc::clone(&queued.job);
                    }
                    continue;
                }
                state = inner
                    .work_cv
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        inner.generated.fetch_add(1, AtomicOrdering::Relaxed);
        let result = match inner.generator.generate(job.key) {
            Ok(value) => Ok(Arc::new(value)),
            Err(message) => Err(GenerationError::Failed(message)),
        };
        if result.is_ok() {
            let mut state = lock(&inner);
            state.cache_put(job.key, result.clone());
        }
        job.signal.publish(result);
    }
}

pub struct GenerationScheduler<T> {
    inner: Arc<Inner<T>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl<T: Send + Sync + 'static> GenerationScheduler<T> {
    pub fn new<G>(generator: G, workers: usize, capacity: usize) -> Self
    where
        G: ChunkGenerator<T>,
    {
        let capacity = capacity.max(1);
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                queue: BinaryHeap::new(),
                in_flight: HashMap::new(),
                completed: HashMap::new(),
                completed_order: VecDeque::new(),
                completed_capacity: capacity.saturating_mul(2),
                next_seq: 0,
                next_token: 0,
            }),
            work_cv: Condvar::new(),
            space_cv: Condvar::new(),
            generator: Box::new(generator),
            capacity,
            stopping: AtomicBool::new(false),
            generated: AtomicUsize::new(0),
        });
        let workers = (0..workers)
            .map(|_| {
                let inner = Arc::clone(&inner);
                thread::spawn(move || worker(inner))
            })
            .collect();
        Self {
            inner,
            workers: Mutex::new(workers),
        }
    }

    pub fn try_request(
        &self,
        key: GenerationKey,
        priority: RequestPriority,
    ) -> Result<GenerationRequest<T>, GenerationError> {
        if self.inner.stopping.load(AtomicOrdering::Acquire) {
            return Err(GenerationError::Stopped);
        }
        let mut state = lock(&self.inner);
        let token = state.token();
        if let Some(result) = state.completed.get(&key).cloned() {
            let job = Arc::new(Job {
                key,
                priority,
                signal: Arc::new(JobSignal::with_result(result)),
            });
            return Ok(GenerationRequest {
                inner: Arc::clone(&self.inner),
                job,
                token,
                registered: false,
                done: AtomicBool::new(false),
            });
        }
        if let Some(entry) = state.in_flight.get_mut(&key) {
            entry.waiters.push(token.id);
            return Ok(GenerationRequest {
                inner: Arc::clone(&self.inner),
                job: Arc::clone(&entry.job),
                token,
                registered: true,
                done: AtomicBool::new(false),
            });
        }
        if state.queue.len() >= self.inner.capacity {
            return Err(GenerationError::QueueFull);
        }
        let seq = state.next_seq;
        state.next_seq = state.next_seq.wrapping_add(1);
        let job = Arc::new(Job {
            key,
            priority,
            signal: Arc::new(JobSignal::new()),
        });
        state.in_flight.insert(
            key,
            InFlight {
                job: Arc::clone(&job),
                waiters: vec![token.id],
                cancelled: false,
            },
        );
        state.queue.push(Reverse(Queued {
            priority,
            seq,
            job: Arc::clone(&job),
        }));
        drop(state);
        self.inner.work_cv.notify_one();
        Ok(GenerationRequest {
            inner: Arc::clone(&self.inner),
            job,
            token,
            registered: true,
            done: AtomicBool::new(false),
        })
    }

    pub fn request(
        &self,
        key: GenerationKey,
        priority: RequestPriority,
    ) -> Result<GenerationRequest<T>, GenerationError> {
        loop {
            match self.try_request(key, priority) {
                Ok(request) => return Ok(request),
                Err(GenerationError::QueueFull) => {
                    let mut state = lock(&self.inner);
                    while state.queue.len() >= self.inner.capacity
                        && !self.inner.stopping.load(AtomicOrdering::Acquire)
                    {
                        state = self
                            .inner
                            .space_cv
                            .wait(state)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                    if self.inner.stopping.load(AtomicOrdering::Acquire) {
                        return Err(GenerationError::Stopped);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub fn queued_requests(&self) -> Vec<(GenerationKey, RequestPriority)> {
        let state = lock(&self.inner);
        let mut queued: Vec<_> = state
            .queue
            .iter()
            .map(|Reverse(entry)| (entry.job.key, entry.priority, entry.seq))
            .collect();
        queued.sort_unstable_by_key(|&(_, priority, seq)| (priority, seq));
        queued
            .into_iter()
            .map(|(key, priority, _)| (key, priority))
            .collect()
    }

    pub fn in_flight_requests(&self) -> Vec<GenerationKey> {
        let state = lock(&self.inner);
        let mut keys: Vec<_> = state.in_flight.keys().copied().collect();
        keys.sort_unstable();
        keys
    }

    pub fn generated_count(&self) -> usize {
        self.inner.generated.load(AtomicOrdering::Relaxed)
    }

    pub fn shutdown(&self) {
        self.inner.stopping.store(true, AtomicOrdering::Release);
        self.inner.work_cv.notify_all();
        self.inner.space_cv.notify_all();
        let handles = std::mem::take(
            &mut *self
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for worker in handles {
            let _ = worker.join();
        }
    }
}

impl<T> Drop for GenerationScheduler<T> {
    fn drop(&mut self) {
        self.inner.stopping.store(true, AtomicOrdering::Release);
        self.inner.work_cv.notify_all();
        self.inner.space_cv.notify_all();
    }
}

pub struct GenerationRequest<T> {
    inner: Arc<Inner<T>>,
    job: Arc<Job<T>>,
    token: CancellationToken,
    registered: bool,
    done: AtomicBool,
}

impl<T> GenerationRequest<T> {
    pub fn key(&self) -> GenerationKey {
        self.job.key
    }

    pub fn priority(&self) -> RequestPriority {
        self.job.priority
    }

    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    pub fn try_result(&self) -> Option<GenerationResult<T>> {
        self.job.signal.peek()
    }

    pub fn cancel(&self) {
        if self.done.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        self.token.cancel();
        self.cancel_registered();
    }

    pub fn wait(self) -> GenerationResult<T> {
        let result = if self.token.is_cancelled() {
            Err(GenerationError::Cancelled)
        } else {
            self.job.signal.wait()
        };
        self.done.store(true, AtomicOrdering::Release);
        if self.registered {
            let mut state = lock(&self.inner);
            if let Some(entry) = state.in_flight.get_mut(&self.job.key) {
                entry.waiters.retain(|id| *id != self.token.id());
                if entry.waiters.is_empty() {
                    state.in_flight.remove(&self.job.key);
                }
            }
        }
        result
    }

    fn cancel_registered(&self) {
        if !self.registered {
            return;
        }
        let mut state = lock(&self.inner);
        let mut cancelled_job = None;
        if let Some(entry) = state.in_flight.get_mut(&self.job.key) {
            entry.waiters.retain(|id| *id != self.token.id());
            if entry.waiters.is_empty() {
                entry.cancelled = true;
                cancelled_job = Some(Arc::clone(&entry.job));
            }
        }
        if let Some(job) = cancelled_job {
            state.in_flight.remove(&self.job.key);
            state.drop_queued(self.job.key);
            job.signal.publish(Err(GenerationError::Cancelled));
            self.inner.work_cv.notify_all();
            self.inner.space_cv.notify_all();
        }
    }
}

impl<T> Drop for GenerationRequest<T> {
    fn drop(&mut self) {
        if !self.done.swap(true, AtomicOrdering::AcqRel) && self.registered {
            self.token.cancel();
            self.cancel_registered();
        }
    }
}
