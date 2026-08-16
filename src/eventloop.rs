// ============================================================
//  eventloop.rs  —  Clock, timers and animation frames
// ============================================================
//
//  The scheduling half of the browser's event loop: *what* is due and *in
//  which order*, with no opinion about what a callback is. The queues are
//  generic over the callback type, so the ordering rules can be tested on
//  their own before any JavaScript is involved.
//
//  Time comes from a [`Clock`]. Production uses [`RealClock`] (monotonic,
//  immune to the system clock moving); tests use [`ManualClock`] and advance
//  it by hand, so no test ever sleeps.
//
//  Ordering rules, which the tests pin down:
//   • timers fire when `now >= deadline`, earliest deadline first
//   • equal deadlines fire in registration order (FIFO)
//   • a timer scheduled *during* a turn never runs in that same turn —
//     nested `setTimeout(…, 0)` becomes a task for the next turn
//   • an interval that falls behind does not "catch up" by firing repeatedly;
//     it is rescheduled from now, so a slow frame cannot spiral
//   • a turn runs at most `budget` callbacks; the rest wait for the next turn

use std::cell::Cell;
use std::time::Instant;

/// Monotonic milliseconds since the loop started.
pub trait Clock {
    fn now_ms(&self) -> f64;

    /// Move a manual clock forward. Real clocks ignore this.
    fn advance_ms(&self, _delta_ms: f64) {}
}

/// Wall-clock time from a monotonic instant captured at startup.
pub struct RealClock {
    start: Instant,
}

impl RealClock {
    pub fn new() -> RealClock {
        RealClock {
            start: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        RealClock::new()
    }
}

impl Clock for RealClock {
    fn now_ms(&self) -> f64 {
        // `Instant` is monotonic, so this never goes backwards even if the
        // system clock is adjusted.
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// A clock that only moves when a test says so.
#[derive(Default)]
pub struct ManualClock {
    now: Cell<f64>,
}

impl ManualClock {
    pub fn new() -> ManualClock {
        ManualClock {
            now: Cell::new(0.0),
        }
    }

    pub fn starting_at(now_ms: f64) -> ManualClock {
        ManualClock {
            now: Cell::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: f64) {
        self.now.set(now_ms);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> f64 {
        self.now.get()
    }

    fn advance_ms(&self, delta_ms: f64) {
        // Clamped so a manual clock is monotonic too.
        self.now.set(self.now.get() + delta_ms.max(0.0));
    }
}

/// Identifies a timer or an animation-frame request.
pub type TaskId = u32;

/// How many microtasks one checkpoint will run before yielding to the loop.
///
/// A microtask may queue another one, and the checkpoint keeps draining until
/// the queue is empty — so a callback that re-queues itself forever would
/// never return. The budget bounds that: leftovers stay queued for the next
/// checkpoint, which keeps the browser responsive instead of wedged.
pub const MAX_MICROTASKS_PER_CHECKPOINT: usize = 1024;

/// The microtask queue: strict FIFO, drained to empty at each checkpoint.
///
/// This is deliberately separate from the timer queues. Microtasks have no
/// deadline and no id — they run at the next checkpoint, in the order they
/// were queued, and anything queued *while* draining runs in the same
/// checkpoint rather than waiting for the next task.
#[derive(Debug)]
pub struct MicrotaskQueue<T> {
    queue: std::collections::VecDeque<T>,
    budget: usize,
}

impl<T> Default for MicrotaskQueue<T> {
    fn default() -> Self {
        MicrotaskQueue::new()
    }
}

impl<T> MicrotaskQueue<T> {
    pub fn new() -> MicrotaskQueue<T> {
        MicrotaskQueue {
            queue: std::collections::VecDeque::new(),
            budget: MAX_MICROTASKS_PER_CHECKPOINT,
        }
    }

    /// Change how many microtasks a single checkpoint may run.
    pub fn with_budget(mut self, budget: usize) -> MicrotaskQueue<T> {
        self.budget = budget.max(1);
        self
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Queue a microtask to run at the next checkpoint.
    pub fn enqueue(&mut self, task: T) {
        self.queue.push_back(task);
    }

    /// Take the next microtask, oldest first.
    pub fn pop(&mut self) -> Option<T> {
        self.queue.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Drop everything queued — what navigating away from a page does.
    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    Timeout,
    Interval,
}

/// How many callbacks one turn of the loop will run before yielding.
pub const DEFAULT_TASK_BUDGET: usize = 64;

/// Shortest interval a repeating timer may use, in milliseconds.
///
/// HTML clamps nested timers to 4ms for the same reason: a 0ms interval would
/// otherwise consume the whole loop.
pub const MIN_INTERVAL_MS: f64 = 4.0;

#[derive(Debug, Clone)]
struct Timer<T> {
    id: TaskId,
    kind: TimerKind,
    deadline_ms: f64,
    interval_ms: f64,
    /// Registration order, used to break deadline ties and to keep a timer
    /// scheduled during a turn out of that turn.
    sequence: u64,
    callback: T,
}

/// The timer and animation-frame queues for one document.
#[derive(Debug)]
pub struct Scheduler<T> {
    timers: Vec<Timer<T>>,
    frames: Vec<(TaskId, T)>,
    next_id: TaskId,
    sequence: u64,
    budget: usize,
}

impl<T> Default for Scheduler<T> {
    fn default() -> Self {
        Scheduler::new()
    }
}

impl<T> Scheduler<T> {
    pub fn new() -> Scheduler<T> {
        Scheduler {
            timers: Vec::new(),
            frames: Vec::new(),
            next_id: 1,
            sequence: 0,
            budget: DEFAULT_TASK_BUDGET,
        }
    }

    /// Change how many callbacks a single turn may run.
    pub fn with_budget(mut self, budget: usize) -> Scheduler<T> {
        self.budget = budget.max(1);
        self
    }

    fn take_id(&mut self) -> TaskId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Schedule a one-shot timer. A missing or negative delay means "as soon
    /// as possible", which is the next turn rather than this one.
    pub fn set_timeout(&mut self, callback: T, delay_ms: f64, now_ms: f64) -> TaskId {
        self.add_timer(TimerKind::Timeout, callback, delay_ms, now_ms)
    }

    /// Schedule a repeating timer.
    pub fn set_interval(&mut self, callback: T, delay_ms: f64, now_ms: f64) -> TaskId {
        self.add_timer(TimerKind::Interval, callback, delay_ms, now_ms)
    }

    fn add_timer(&mut self, kind: TimerKind, callback: T, delay_ms: f64, now_ms: f64) -> TaskId {
        let delay = if delay_ms.is_finite() {
            delay_ms.max(0.0)
        } else {
            0.0
        };
        // Repeating timers are clamped so they always make progress.
        let interval = match kind {
            TimerKind::Interval => delay.max(MIN_INTERVAL_MS),
            TimerKind::Timeout => delay,
        };
        let id = self.take_id();
        self.sequence += 1;
        self.timers.push(Timer {
            id,
            kind,
            deadline_ms: now_ms + interval,
            interval_ms: interval,
            sequence: self.sequence,
            callback,
        });
        id
    }

    /// Cancel a timer, dropping its callback. Returns true if it existed.
    pub fn clear_timer(&mut self, id: TaskId) -> bool {
        let before = self.timers.len();
        self.timers.retain(|timer| timer.id != id);
        before != self.timers.len()
    }

    /// Ask for a callback before the next frame is painted.
    pub fn request_animation_frame(&mut self, callback: T) -> TaskId {
        let id = self.take_id();
        self.frames.push((id, callback));
        id
    }

    /// Cancel a pending animation-frame request.
    pub fn cancel_animation_frame(&mut self, id: TaskId) -> bool {
        let before = self.frames.len();
        self.frames.retain(|(frame_id, _)| *frame_id != id);
        before != self.frames.len()
    }

    /// Remove and return the timer callbacks due at `now_ms`, in firing order.
    ///
    /// Timers registered while this turn's callbacks run are held back for the
    /// next turn, so a nested `setTimeout(…, 0)` cannot starve the loop.
    pub fn take_due_timers(&mut self, now_ms: f64) -> Vec<(TaskId, T)>
    where
        T: Clone,
    {
        // Anything registered from here on belongs to a later turn.
        let cutoff = self.sequence;

        let mut due: Vec<(f64, u64, usize)> = self
            .timers
            .iter()
            .enumerate()
            .filter(|(_, timer)| timer.sequence <= cutoff && timer.deadline_ms <= now_ms)
            .map(|(index, timer)| (timer.deadline_ms, timer.sequence, index))
            .collect();
        due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        due.truncate(self.budget);

        let mut ready = Vec::with_capacity(due.len());
        let mut finished: Vec<TaskId> = Vec::new();
        for (_, _, index) in due {
            let timer = &mut self.timers[index];
            ready.push((timer.id, timer.callback.clone()));
            match timer.kind {
                TimerKind::Timeout => finished.push(timer.id),
                TimerKind::Interval => {
                    // Reschedule from now: a late interval fires once, not
                    // once per missed period. Its registration sequence is
                    // kept, so an interval still sorts by when it was created
                    // rather than when it last fired.
                    timer.deadline_ms = now_ms + timer.interval_ms;
                }
            }
        }
        self.timers.retain(|timer| !finished.contains(&timer.id));
        ready
    }

    /// Remove and return the animation-frame callbacks for this frame.
    ///
    /// Callbacks registered while these run belong to the *next* frame, which
    /// is what makes a self-rescheduling `requestAnimationFrame` loop advance
    /// one step per frame.
    pub fn take_animation_frames(&mut self) -> Vec<(TaskId, T)> {
        std::mem::take(&mut self.frames)
    }

    /// When the next timer is due, if any.
    pub fn next_deadline_ms(&self) -> Option<f64> {
        self.timers
            .iter()
            .map(|timer| timer.deadline_ms)
            .min_by(f64::total_cmp)
    }

    /// True when a frame callback is pending or a timer will ever fire.
    pub fn has_pending_work(&self) -> bool {
        !self.timers.is_empty() || !self.frames.is_empty()
    }

    /// True when something is due right now or a frame is pending.
    pub fn has_work_at(&self, now_ms: f64) -> bool {
        !self.frames.is_empty() || self.timers.iter().any(|timer| timer.deadline_ms <= now_ms)
    }

    pub fn timer_count(&self) -> usize {
        self.timers.len()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Drop every pending task — what navigating away from a page does.
    pub fn clear_all(&mut self) {
        self.timers.clear();
        self.frames.clear();
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(taken: Vec<(TaskId, &'static str)>) -> Vec<&'static str> {
        taken.into_iter().map(|(_, name)| name).collect()
    }

    // ── Clock ─────────────────────────────────────────────────────────────

    #[test]
    fn manual_clock_only_moves_when_advanced() {
        let clock = ManualClock::new();
        assert_eq!(clock.now_ms(), 0.0);
        clock.advance_ms(100.0);
        assert_eq!(clock.now_ms(), 100.0);
        clock.advance_ms(0.5);
        assert_eq!(clock.now_ms(), 100.5);
    }

    #[test]
    fn manual_clock_never_goes_backwards() {
        let clock = ManualClock::starting_at(50.0);
        clock.advance_ms(-100.0);
        assert_eq!(clock.now_ms(), 50.0, "negative advances are ignored");
    }

    #[test]
    fn real_clock_is_monotonic() {
        let clock = RealClock::new();
        let first = clock.now_ms();
        let second = clock.now_ms();
        assert!(second >= first);
        assert!(first >= 0.0);
        // Advancing a real clock is a no-op rather than an error.
        clock.advance_ms(1000.0);
        assert!(clock.now_ms() < first + 1000.0);
    }

    // ── Timeouts ──────────────────────────────────────────────────────────

    #[test]
    fn a_timeout_fires_only_once_its_deadline_passes() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_timeout("late", 100.0, 0.0);

        assert!(scheduler.take_due_timers(99.0).is_empty());
        assert_eq!(ids(scheduler.take_due_timers(100.0)), vec!["late"]);
        // One-shot: it does not fire again.
        assert!(scheduler.take_due_timers(500.0).is_empty());
        assert_eq!(scheduler.timer_count(), 0);
    }

    #[test]
    fn earlier_deadlines_fire_first_and_ties_keep_registration_order() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_timeout("a", 100.0, 0.0);
        scheduler.set_timeout("b", 50.0, 0.0);
        scheduler.set_timeout("c", 100.0, 0.0);

        assert_eq!(ids(scheduler.take_due_timers(100.0)), vec!["b", "a", "c"]);
    }

    #[test]
    fn missing_and_negative_delays_are_clamped_to_zero() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_timeout("zero", 0.0, 0.0);
        scheduler.set_timeout("negative", -500.0, 0.0);
        scheduler.set_timeout("nan", f64::NAN, 0.0);

        assert_eq!(
            ids(scheduler.take_due_timers(0.0)),
            vec!["zero", "negative", "nan"]
        );
    }

    #[test]
    fn clearing_a_timer_drops_it_and_its_callback() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        let id = scheduler.set_timeout("gone", 10.0, 0.0);
        scheduler.set_timeout("kept", 10.0, 0.0);

        assert!(scheduler.clear_timer(id));
        assert!(!scheduler.clear_timer(id), "clearing twice is harmless");
        assert_eq!(scheduler.timer_count(), 1);
        assert_eq!(ids(scheduler.take_due_timers(10.0)), vec!["kept"]);
    }

    #[test]
    fn a_timer_scheduled_during_a_turn_waits_for_the_next_one() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_timeout("outer", 0.0, 0.0);

        let first = scheduler.take_due_timers(0.0);
        assert_eq!(ids(first), vec!["outer"]);

        // The callback would have run here and scheduled a nested timer.
        scheduler.set_timeout("nested", 0.0, 0.0);
        // Same instant, but it is a new task: it runs on the next turn.
        assert_eq!(ids(scheduler.take_due_timers(0.0)), vec!["nested"]);
    }

    // ── Intervals ─────────────────────────────────────────────────────────

    #[test]
    fn an_interval_repeats() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("tick", 100.0, 0.0);

        assert!(scheduler.take_due_timers(99.0).is_empty());
        assert_eq!(ids(scheduler.take_due_timers(100.0)), vec!["tick"]);
        assert!(scheduler.take_due_timers(150.0).is_empty());
        assert_eq!(ids(scheduler.take_due_timers(200.0)), vec!["tick"]);
        assert_eq!(scheduler.timer_count(), 1, "it stays scheduled");
    }

    #[test]
    fn a_late_interval_fires_once_rather_than_catching_up() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("tick", 10.0, 0.0);

        // A second of stall: a catch-up loop would fire this 100 times.
        assert_eq!(ids(scheduler.take_due_timers(1000.0)), vec!["tick"]);
        assert!(scheduler.take_due_timers(1005.0).is_empty());
        assert_eq!(ids(scheduler.take_due_timers(1010.0)), vec!["tick"]);
    }

    #[test]
    fn a_zero_interval_is_clamped_so_it_cannot_spin() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("spin", 0.0, 0.0);

        assert_eq!(ids(scheduler.take_due_timers(0.0)).len(), 0);
        // It only becomes due after the minimum interval.
        assert_eq!(
            ids(scheduler.take_due_timers(MIN_INTERVAL_MS)),
            vec!["spin"]
        );
    }

    #[test]
    fn a_repeating_interval_keeps_its_place_in_registration_order() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("interval", 100.0, 0.0);
        scheduler.set_timeout("timeout", 200.0, 0.0);

        assert_eq!(ids(scheduler.take_due_timers(100.0)), vec!["interval"]);
        // Both are due at 200: the interval was registered first, so it stays
        // ahead even though it has fired since.
        assert_eq!(
            ids(scheduler.take_due_timers(200.0)),
            vec!["interval", "timeout"]
        );
    }

    #[test]
    fn intervals_and_timeouts_interleave_by_deadline() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("every-50", 50.0, 0.0);
        scheduler.set_timeout("once-75", 75.0, 0.0);

        assert_eq!(ids(scheduler.take_due_timers(50.0)), vec!["every-50"]);
        assert_eq!(
            ids(scheduler.take_due_timers(100.0)),
            vec!["once-75", "every-50"]
        );
    }

    // ── Budget ────────────────────────────────────────────────────────────

    #[test]
    fn a_turn_runs_at_most_its_budget_and_keeps_the_rest() {
        let mut scheduler: Scheduler<&str> = Scheduler::new().with_budget(2);
        for name in ["a", "b", "c", "d"] {
            scheduler.set_timeout(name, 0.0, 0.0);
        }

        assert_eq!(ids(scheduler.take_due_timers(0.0)), vec!["a", "b"]);
        assert_eq!(ids(scheduler.take_due_timers(0.0)), vec!["c", "d"]);
        assert!(scheduler.take_due_timers(0.0).is_empty());
    }

    // ── Animation frames ──────────────────────────────────────────────────

    #[test]
    fn animation_frames_run_once_in_registration_order() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.request_animation_frame("first");
        scheduler.request_animation_frame("second");

        assert_eq!(
            ids(scheduler.take_animation_frames()),
            vec!["first", "second"]
        );
        // Single-shot: the next frame has nothing to run.
        assert!(scheduler.take_animation_frames().is_empty());
    }

    #[test]
    fn a_frame_requested_during_a_frame_waits_for_the_next() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.request_animation_frame("frame-1");

        let running = scheduler.take_animation_frames();
        assert_eq!(ids(running), vec!["frame-1"]);
        // The callback re-registered itself while running.
        scheduler.request_animation_frame("frame-2");
        assert_eq!(ids(scheduler.take_animation_frames()), vec!["frame-2"]);
    }

    #[test]
    fn animation_frames_can_be_cancelled() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        let id = scheduler.request_animation_frame("gone");
        scheduler.request_animation_frame("kept");

        assert!(scheduler.cancel_animation_frame(id));
        assert!(!scheduler.cancel_animation_frame(id));
        assert_eq!(ids(scheduler.take_animation_frames()), vec!["kept"]);
    }

    // ── Queue state ───────────────────────────────────────────────────────

    #[test]
    fn reports_the_next_deadline_and_pending_work() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        assert_eq!(scheduler.next_deadline_ms(), None);
        assert!(!scheduler.has_pending_work());

        scheduler.set_timeout("late", 200.0, 0.0);
        scheduler.set_timeout("soon", 50.0, 0.0);
        assert_eq!(scheduler.next_deadline_ms(), Some(50.0));
        assert!(scheduler.has_pending_work());
        assert!(!scheduler.has_work_at(10.0));
        assert!(scheduler.has_work_at(50.0));

        scheduler.request_animation_frame("frame");
        assert!(scheduler.has_work_at(0.0), "a pending frame is always work");
    }

    #[test]
    fn clearing_everything_empties_both_queues() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        scheduler.set_interval("tick", 10.0, 0.0);
        scheduler.request_animation_frame("frame");

        scheduler.clear_all();
        assert_eq!(scheduler.timer_count(), 0);
        assert_eq!(scheduler.frame_count(), 0);
        assert!(!scheduler.has_pending_work());
    }

    // ── Microtasks ────────────────────────────────────────────────────────

    #[test]
    fn microtasks_come_out_in_the_order_they_went_in() {
        let mut queue: MicrotaskQueue<&str> = MicrotaskQueue::new();
        queue.enqueue("a");
        queue.enqueue("b");
        queue.enqueue("c");

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop(), Some("a"));
        assert_eq!(queue.pop(), Some("b"));
        assert_eq!(queue.pop(), Some("c"));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_microtask_queued_while_draining_joins_the_same_checkpoint() {
        let mut queue: MicrotaskQueue<i32> = MicrotaskQueue::new();
        queue.enqueue(1);

        // Draining: each task queues one more, up to three.
        let mut ran = Vec::new();
        while let Some(task) = queue.pop() {
            ran.push(task);
            if task < 3 {
                queue.enqueue(task + 1);
            }
        }
        assert_eq!(
            ran,
            vec![1, 2, 3],
            "nested microtasks drain in the same pass"
        );
    }

    #[test]
    fn clearing_the_queue_drops_pending_microtasks() {
        let mut queue: MicrotaskQueue<&str> = MicrotaskQueue::new();
        queue.enqueue("gone");
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn the_checkpoint_budget_is_configurable() {
        let queue: MicrotaskQueue<&str> = MicrotaskQueue::new().with_budget(8);
        assert_eq!(queue.budget(), 8);
        // A zero budget would wedge the loop, so it is clamped.
        let clamped: MicrotaskQueue<&str> = MicrotaskQueue::new().with_budget(0);
        assert_eq!(clamped.budget(), 1);
    }

    #[test]
    fn dropping_the_queue_releases_its_tasks() {
        use std::rc::Rc;
        let callback = Rc::new("closure");
        let mut queue: MicrotaskQueue<Rc<&str>> = MicrotaskQueue::new();
        queue.enqueue(callback.clone());
        queue.enqueue(callback.clone());
        assert_eq!(Rc::strong_count(&callback), 3);

        drop(queue);
        assert_eq!(Rc::strong_count(&callback), 1);
    }

    // ── Callback ownership ────────────────────────────────────────────────

    #[test]
    fn cancelling_a_task_releases_its_callback() {
        use std::rc::Rc;

        let callback = Rc::new("closure state");
        let mut scheduler: Scheduler<Rc<&str>> = Scheduler::new();

        let timer = scheduler.set_timeout(callback.clone(), 10.0, 0.0);
        let interval = scheduler.set_interval(callback.clone(), 10.0, 0.0);
        let frame = scheduler.request_animation_frame(callback.clone());
        assert_eq!(Rc::strong_count(&callback), 4);

        scheduler.clear_timer(timer);
        scheduler.clear_timer(interval);
        scheduler.cancel_animation_frame(frame);
        assert_eq!(
            Rc::strong_count(&callback),
            1,
            "a cancelled task must not keep its callback alive"
        );
    }

    #[test]
    fn a_fired_timeout_releases_its_callback() {
        use std::rc::Rc;

        let callback = Rc::new("once");
        let mut scheduler: Scheduler<Rc<&str>> = Scheduler::new();
        scheduler.set_timeout(callback.clone(), 0.0, 0.0);

        let taken = scheduler.take_due_timers(0.0);
        assert_eq!(taken.len(), 1);
        drop(taken);
        assert_eq!(
            Rc::strong_count(&callback),
            1,
            "a one-shot timer is dropped after it fires"
        );
    }

    #[test]
    fn dropping_the_scheduler_releases_everything() {
        use std::rc::Rc;

        let callback = Rc::new("page state");
        let mut scheduler: Scheduler<Rc<&str>> = Scheduler::new();
        scheduler.set_interval(callback.clone(), 10.0, 0.0);
        scheduler.request_animation_frame(callback.clone());
        assert_eq!(Rc::strong_count(&callback), 3);

        // What navigating away from a page does.
        drop(scheduler);
        assert_eq!(Rc::strong_count(&callback), 1);
    }

    #[test]
    fn ids_are_unique_across_timers_and_frames() {
        let mut scheduler: Scheduler<&str> = Scheduler::new();
        let a = scheduler.set_timeout("a", 0.0, 0.0);
        let b = scheduler.set_interval("b", 10.0, 0.0);
        let c = scheduler.request_animation_frame("c");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
