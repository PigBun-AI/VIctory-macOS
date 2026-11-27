//! The thread behind `ID3D12Fence::SetEventOnCompletion`.
//!
//! One thread per fence, started by the first wait that is not already met,
//! takes the waits in value order: it blocks in the back on the lowest value
//! (the back's own wait wakes the moment the GPU signals), then runs that
//! wait's action and every other one that value met. A thread of its own
//! for each wait (what this replaced) costs a thread start in the path of
//! every frame's wait. A wait that comes in for a lower value than the one
//! the thread is blocked on (values normally rise) is not left behind it:
//! it gets a thread of its own for that one wait.
//!
//! The thread leaves after [`IDLE`] without a wait.
//!
//! Pure Rust: the action (`SetEvent`) is the shell's.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::hresult::Hr;

/// How long the thread waits for another wait before it ends.
pub const IDLE: Duration = Duration::from_secs(1);

/// What is waited on: the back's fence.
pub trait Source: Send + Sync + 'static {
    /// Blocks until `value` is reached; `Ok(false)` on a time out.
    fn wait(&self, value: u64, timeout_ms: u64) -> Result<bool, Hr>;
}

impl Source for crate::device::Fence {
    fn wait(&self, value: u64, timeout_ms: u64) -> Result<bool, Hr> {
        crate::device::Fence::wait(self, value, timeout_ms)
    }
}

type Action = Box<dyn FnOnce() + Send>;

struct Item {
    value: u64,
    seq: u64,
    action: Action,
}

impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        (self.value, self.seq) == (o.value, o.seq)
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.value, self.seq).cmp(&(o.value, o.seq))
    }
}

#[derive(Default)]
struct State {
    waits: BinaryHeap<Reverse<Item>>,
    /// The value the thread is blocked on.
    blocked_on: Option<u64>,
    running: bool,
    seq: u64,
}

struct Shared<S: Source> {
    source: Arc<S>,
    state: Mutex<State>,
    wake: Condvar,
}

/// The waits of one fence.
pub struct Waiter<S: Source> {
    shared: Arc<Shared<S>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<S: Source> Waiter<S> {
    pub fn new(source: Arc<S>) -> Self {
        Waiter {
            shared: Arc::new(Shared {
                source,
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
        }
    }

    /// Runs `action` once the fence reaches `value`.
    pub fn when_reached(&self, value: u64, action: impl FnOnce() + Send + 'static) {
        let action: Action = Box::new(action);
        let mut st = lock(&self.shared.state);
        if st.blocked_on.is_some_and(|b| value < b) {
            // Behind a higher value it would wait for that too.
            drop(st);
            let source = self.shared.source.clone();
            std::thread::spawn(move || {
                if source.wait(value, u64::MAX).unwrap_or(false) {
                    action();
                }
            });
            return;
        }
        st.seq += 1;
        let seq = st.seq;
        st.waits.push(Reverse(Item { value, seq, action }));
        if st.running {
            drop(st);
            self.shared.wake.notify_one();
            return;
        }
        st.running = true;
        drop(st);
        let shared = self.shared.clone();
        std::thread::spawn(move || shared.run());
    }
}

impl<S: Source> Shared<S> {
    fn run(&self) {
        let mut st = lock(&self.state);
        loop {
            let Some(Reverse(first)) = st.waits.pop() else {
                let (g, timeout) = self
                    .wake
                    .wait_timeout(st, IDLE)
                    .unwrap_or_else(|e| e.into_inner());
                st = g;
                if timeout.timed_out() && st.waits.is_empty() {
                    st.running = false;
                    return;
                }
                continue;
            };
            st.blocked_on = Some(first.value);
            drop(st);
            let reached = self.source.wait(first.value, u64::MAX).unwrap_or(false);
            st = lock(&self.state);
            st.blocked_on = None;
            let mut due = vec![first];
            while st
                .waits
                .peek()
                .is_some_and(|Reverse(i)| i.value <= due[0].value)
            {
                due.push(st.waits.pop().expect("peeked").0);
            }
            drop(st);
            if reached {
                for item in due {
                    (item.action)();
                }
            }
            st = lock(&self.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;

    /// A fence that moves when the test says.
    struct Fake {
        value: Mutex<u64>,
        moved: Condvar,
        threads: AtomicU64,
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake {
                value: Mutex::new(0),
                moved: Condvar::new(),
                threads: AtomicU64::new(0),
            })
        }
        fn set(&self, v: u64) {
            *lock(&self.value) = v;
            self.moved.notify_all();
        }
    }

    impl Source for Fake {
        fn wait(&self, value: u64, _timeout_ms: u64) -> Result<bool, Hr> {
            self.threads.fetch_add(1, Ordering::SeqCst);
            let mut v = lock(&self.value);
            while *v < value {
                v = self.moved.wait(v).unwrap();
            }
            Ok(true)
        }
    }

    fn got(rx: &mpsc::Receiver<u64>) -> Option<u64> {
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn waits_run_in_value_order_as_the_fence_moves() {
        let f = Fake::new();
        let w = Waiter::new(f.clone());
        let (tx, rx) = mpsc::channel();
        for v in [1, 2, 4] {
            let tx = tx.clone();
            w.when_reached(v, move || tx.send(v).unwrap());
        }
        f.set(2);
        assert_eq!((got(&rx), got(&rx)), (Some(1), Some(2)));
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        f.set(9);
        assert_eq!(got(&rx), Some(4));
    }

    #[test]
    fn a_lower_value_behind_a_higher_one_is_not_held_up() {
        let f = Fake::new();
        let w = Waiter::new(f.clone());
        let (tx, rx) = mpsc::channel();
        let t = tx.clone();
        w.when_reached(10, move || t.send(10).unwrap());
        // The thread is on its way to block on 10.
        while lock(&w.shared.state).blocked_on.is_none() {
            std::thread::yield_now();
        }
        w.when_reached(3, move || tx.send(3).unwrap());
        f.set(3);
        assert_eq!(got(&rx), Some(3), "not behind 10");
        f.set(10);
        assert_eq!(got(&rx), Some(10));
    }

    #[test]
    fn one_thread_serves_the_waits_one_after_another() {
        let f = Fake::new();
        let w = Waiter::new(f.clone());
        let (tx, rx) = mpsc::channel();
        for v in 1..=50u64 {
            let tx = tx.clone();
            w.when_reached(v, move || tx.send(v).unwrap());
            f.set(v);
            assert_eq!(got(&rx), Some(v));
        }
        // 50 waits, each blocked on by the one thread (none by a thread of its own).
        assert_eq!(f.threads.load(Ordering::SeqCst), 50);
        assert!(lock(&w.shared.state).running);
    }
}
