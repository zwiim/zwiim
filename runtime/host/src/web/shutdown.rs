use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// Shutdown coordinator for the embedded web server. Combines a sticky
/// `AtomicBool` with a `Notify` so that:
///
/// - Currently awaiting tasks wake up promptly (`Notify`)
/// - Tasks that *start* awaiting after shutdown was triggered see the
///   flag and exit immediately, instead of waiting forever for a signal
///   that already fired (`AtomicBool`)
///
/// Without the sticky flag, a WebSocket handshake that completes during
/// the narrow window between `notify_waiters()` and the listener being
/// torn down would create a fresh `notified()` future after the wakeup
/// already happened — leaking an orphan task into the runtime.
pub struct ShutdownSignal {
    flag: AtomicBool,
    notify: Notify,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    /// Set the sticky flag and wake all currently waiting `wait()` futures.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Clear the flag for the next server cycle. Must be called before
    /// any task subscribes to `wait()` for the new cycle, otherwise that
    /// task would see the leftover signal from the previous cycle.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::Release);
    }

    pub fn is_triggered(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// Resolves when shutdown is triggered. Returns immediately if the
    /// flag is already set. Race-safe across `trigger()` happening at
    /// any point: the `notified()` future is created *before* the second
    /// flag check, so a `trigger()` interleaved between the two checks
    /// still wakes us via the future.
    pub async fn wait(&self) {
        if self.is_triggered() {
            return;
        }
        let notified = self.notify.notified();
        if self.is_triggered() {
            return;
        }
        notified.await;
    }
}
