use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

pub type TaskId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn region_key(self, grid_size: u32) -> RegionKey {
        let grid = grid_size as i32;
        RegionKey {
            x: self.x.div_euclid(grid),
            z: self.z.div_euclid(grid),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionKey {
    pub x: i32,
    pub z: i32,
}

struct Task {
    id: TaskId,
    delay: u32,
    period: u32,
    callback: Box<dyn FnMut() + Send>,
    cancelled: bool,
}

struct Region {
    tasks: Vec<Task>,
    ticks: u64,
}

impl Region {
    fn new() -> Self {
        Self {
            tasks: Vec::new(),
            ticks: 0,
        }
    }
}

/// A small regionized task scheduler in the spirit of Folia.
///
/// Chunks are partitioned into square regions. Tasks scheduled at a chunk
/// position only run when that region is ticked, which is the core isolation
/// property Folia uses to run regions independently. This implementation is
/// deliberately single-threaded; the scheduler owns the queues and the host
/// decides how regions are dispatched to worker threads.
pub struct RegionScheduler {
    grid_size: u32,
    next_id: TaskId,
    global: Region,
    regions: BTreeMap<RegionKey, Region>,
}

impl RegionScheduler {
    pub fn new(grid_size: u32) -> Self {
        assert!(
            (1..=1024).contains(&grid_size),
            "region grid size must be between 1 and 1024"
        );
        Self {
            grid_size,
            next_id: 1,
            global: Region::new(),
            regions: BTreeMap::new(),
        }
    }

    pub fn grid_size(&self) -> u32 {
        self.grid_size
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    pub fn task_count(&self) -> usize {
        self.global.tasks.len()
            + self
                .regions
                .values()
                .map(|region| region.tasks.len())
                .sum::<usize>()
    }

    pub fn region_ticks(&self, key: RegionKey) -> Option<u64> {
        self.regions.get(&key).map(|region| region.ticks)
    }

    pub fn global_ticks(&self) -> u64 {
        self.global.ticks
    }

    pub fn schedule_at(
        &mut self,
        chunk: ChunkPos,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        let key = chunk.region_key(self.grid_size);
        let region = self.regions.entry(key).or_insert_with(Region::new);
        push_task(
            &mut region.tasks,
            &mut self.next_id,
            delay,
            period,
            callback,
        )
    }

    pub fn schedule_global(
        &mut self,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        push_task(
            &mut self.global.tasks,
            &mut self.next_id,
            delay,
            period,
            callback,
        )
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        let mut found = false;
        for task in &mut self.global.tasks {
            if task.id == id {
                task.cancelled = true;
                found = true;
            }
        }
        for region in self.regions.values_mut() {
            for task in &mut region.tasks {
                if task.id == id {
                    task.cancelled = true;
                    found = true;
                }
            }
        }
        found
    }

    pub fn cancel_region(&mut self, key: RegionKey) {
        if let Some(region) = self.regions.get_mut(&key) {
            for task in &mut region.tasks {
                task.cancelled = true;
            }
        }
    }

    /// Runs the global queue and every region once.
    pub fn tick(&mut self) -> usize {
        let mut executed = tick_region(&mut self.global);
        let keys: Vec<RegionKey> = self.regions.keys().copied().collect();
        for key in keys {
            if let Some(region) = self.regions.get_mut(&key) {
                executed += tick_region(region);
            }
        }
        executed
    }

    /// Runs the global queue once, then dispatches regions across worker
    /// threads. Each region has exclusive access to its own task queue, which
    /// is the isolation property Folia relies on to parallelize regions.
    pub fn tick_parallel(&mut self, workers: usize) -> usize {
        let workers = workers.max(1);
        if workers == 1 || self.regions.is_empty() {
            return self.tick();
        }

        let global = tick_region(&mut self.global);
        let mut entries: Vec<(RegionKey, Region)> =
            std::mem::take(&mut self.regions).into_iter().collect();
        let chunk_size = entries.len().div_ceil(workers);
        let executed = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for chunk in entries.chunks_mut(chunk_size) {
                let executed = &executed;
                scope.spawn(move || {
                    let mut local = 0;
                    for (_, region) in chunk.iter_mut() {
                        local += tick_region(region);
                    }
                    executed.fetch_add(local, Ordering::Relaxed);
                });
            }
        });

        self.regions = entries.into_iter().collect();
        global + executed.load(Ordering::Relaxed)
    }

    /// Runs only the queue belonging to one region.
    pub fn tick_region(&mut self, key: RegionKey) -> usize {
        match self.regions.get_mut(&key) {
            Some(region) => tick_region(region),
            None => 0,
        }
    }

    pub fn tick_global(&mut self) -> usize {
        tick_region(&mut self.global)
    }
}

fn push_task(
    tasks: &mut Vec<Task>,
    next_id: &mut TaskId,
    delay: u32,
    period: u32,
    callback: impl FnMut() + Send + 'static,
) -> TaskId {
    let id = *next_id;
    *next_id += 1;
    tasks.push(Task {
        id,
        delay,
        period,
        callback: Box::new(callback),
        cancelled: false,
    });
    id
}

fn tick_region(region: &mut Region) -> usize {
    region.ticks += 1;
    let mut executed = 0;
    let mut index = 0;
    while index < region.tasks.len() {
        if region.tasks[index].cancelled {
            region.tasks.remove(index);
            continue;
        }
        if region.tasks[index].delay > 0 {
            region.tasks[index].delay -= 1;
            index += 1;
            continue;
        }

        let period = region.tasks[index].period;
        (region.tasks[index].callback)();
        executed += 1;

        if period > 0 {
            // `delay` counts down at the start of each tick. Subtracting one
            // here makes periodic tasks run every `period` ticks instead of
            // every `period + 1` ticks.
            region.tasks[index].delay = period.saturating_sub(1);
            index += 1;
        } else {
            region.tasks.remove(index);
        }
    }
    executed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn chunk_to_region_mapping_handles_negative_coordinates() {
        let scheduler = RegionScheduler::new(8);
        assert_eq!(
            ChunkPos::new(-1, -1).region_key(scheduler.grid_size()),
            RegionKey { x: -1, z: -1 }
        );
        assert_eq!(
            ChunkPos::new(0, 0).region_key(scheduler.grid_size()),
            RegionKey { x: 0, z: 0 }
        );
        assert_eq!(
            ChunkPos::new(7, 7).region_key(scheduler.grid_size()),
            RegionKey { x: 0, z: 0 }
        );
        assert_eq!(
            ChunkPos::new(8, 8).region_key(scheduler.grid_size()),
            RegionKey { x: 1, z: 1 }
        );
    }

    #[test]
    fn tasks_only_run_when_their_region_ticks() {
        let mut scheduler = RegionScheduler::new(8);
        let left = Arc::new(Mutex::new(0));
        let right = Arc::new(Mutex::new(0));

        {
            let left = Arc::clone(&left);
            scheduler.schedule_at(ChunkPos::new(1, 1), 0, 0, move || {
                *left.lock().unwrap() += 1;
            });
        }
        {
            let right = Arc::clone(&right);
            scheduler.schedule_at(ChunkPos::new(9, 9), 0, 0, move || {
                *right.lock().unwrap() += 1;
            });
        }

        assert_eq!(scheduler.region_count(), 2);
        assert_eq!(scheduler.tick_region(RegionKey { x: 0, z: 0 }), 1);
        assert_eq!(*left.lock().unwrap(), 1);
        assert_eq!(*right.lock().unwrap(), 0);

        assert_eq!(scheduler.tick_region(RegionKey { x: 1, z: 1 }), 1);
        assert_eq!(*left.lock().unwrap(), 1);
        assert_eq!(*right.lock().unwrap(), 1);
    }

    #[test]
    fn delay_and_period_behave_like_bukkit_ticks() {
        let mut scheduler = RegionScheduler::new(4);
        let count = Arc::new(Mutex::new(0));
        {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(0, 0), 1, 2, move || {
                *count.lock().unwrap() += 1;
            });
        }
        let key = RegionKey { x: 0, z: 0 };

        scheduler.tick_region(key); // delay 1 -> 0
        assert_eq!(*count.lock().unwrap(), 0);
        scheduler.tick_region(key); // run, then delay = period - 1 = 1
        assert_eq!(*count.lock().unwrap(), 1);
        scheduler.tick_region(key); // delay 1 -> 0
        assert_eq!(*count.lock().unwrap(), 1);
        scheduler.tick_region(key); // run again, exactly `period` ticks later
        assert_eq!(*count.lock().unwrap(), 2);
    }

    #[test]
    fn cancellation_removes_tasks() {
        let mut scheduler = RegionScheduler::new(8);
        let count = Arc::new(Mutex::new(0));
        let id = {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(0, 0), 0, 1, move || {
                *count.lock().unwrap() += 1;
            })
        };
        assert!(scheduler.cancel(id));
        scheduler.tick();
        assert_eq!(*count.lock().unwrap(), 0);
        assert_eq!(scheduler.task_count(), 0);
    }

    #[test]
    fn global_queue_ticks_with_every_scheduler_tick() {
        let mut scheduler = RegionScheduler::new(8);
        let count = Arc::new(Mutex::new(0));
        {
            let count = Arc::clone(&count);
            scheduler.schedule_global(0, 0, move || {
                *count.lock().unwrap() += 1;
            });
        }
        assert_eq!(scheduler.tick(), 1);
        assert_eq!(*count.lock().unwrap(), 1);
        assert_eq!(scheduler.global_ticks(), 1);
    }

    #[test]
    fn region_tick_counters_are_independent() {
        let mut scheduler = RegionScheduler::new(8);
        scheduler.schedule_at(ChunkPos::new(0, 0), 0, 0, || {});
        scheduler.schedule_at(ChunkPos::new(9, 0), 0, 0, || {});

        scheduler.tick_region(RegionKey { x: 0, z: 0 });
        scheduler.tick_region(RegionKey { x: 0, z: 0 });
        scheduler.tick_region(RegionKey { x: 1, z: 0 });

        assert_eq!(scheduler.region_ticks(RegionKey { x: 0, z: 0 }), Some(2));
        assert_eq!(scheduler.region_ticks(RegionKey { x: 1, z: 0 }), Some(1));
    }

    #[test]
    fn parallel_tick_runs_every_region_exactly_once() {
        let mut scheduler = RegionScheduler::new(4);
        let count = Arc::new(AtomicUsize::new(0));
        for index in 0..16 {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(index * 4, 0), 0, 0, move || {
                count.fetch_add(1, Ordering::Relaxed);
            });
        }

        let executed = scheduler.tick_parallel(4);
        assert_eq!(executed, 16);
        assert_eq!(count.load(Ordering::Relaxed), 16);
        assert_eq!(scheduler.region_count(), 16);
        assert_eq!(scheduler.task_count(), 0);
    }
}
