// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the item-agent cap (automation-engine spec 7.1.8, 15.1).
//!
//! At most 2 item agents run at once [est]; interactive sessions get priority
//! only when an item would start, never by stopping a running item. This is
//! the gate the run loop asks before starting an item session, separate from
//! the lane's one-tab rule (which is a database index).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// The default number of concurrent item agents (spec 7.1.8 [est]).
pub const DEFAULT_ITEM_AGENTS: usize = 2;

/// A counting gate for item sessions. Callers `acquire` before starting an
/// item agent and `release` when its session ends; the returned guard does
/// the release on drop.
#[derive(Debug)]
pub struct ItemAgentGate {
    limit: usize,
    live: Arc<AtomicUsize>,
    freed: Arc<Notify>,
}

impl ItemAgentGate {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            live: Arc::new(AtomicUsize::new(0)),
            freed: Arc::new(Notify::new()),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Whether a new item agent may start now (no queuing).
    pub fn has_slot(&self) -> bool {
        self.live() < self.limit
    }

    /// Take a slot, waiting until one is free. The returned guard releases
    /// it on drop.
    pub async fn acquire(&self) -> ItemAgentSlot {
        loop {
            let current = self.live.load(Ordering::SeqCst);
            if current < self.limit {
                // Compare-and-swap so two waiters cannot both take the last
                // slot; a losing racer loops and waits.
                if self
                    .live
                    .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return ItemAgentSlot {
                        live: self.live.clone(),
                        freed: self.freed.clone(),
                    };
                }
                continue;
            }
            // No slot: wait to be woken when someone releases.
            self.freed.notified().await;
        }
    }

    /// Take a slot only if one is free right now ("Try on one item" path
    /// under the cap). `None` when the cap is full.
    pub fn try_acquire(&self) -> Option<ItemAgentSlot> {
        let mut current = self.live.load(Ordering::SeqCst);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.live.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(ItemAgentSlot {
                        live: self.live.clone(),
                        freed: self.freed.clone(),
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }
}

/// A held item-agent slot; dropping it frees the slot and wakes a waiter.
#[derive(Debug)]
pub struct ItemAgentSlot {
    live: Arc<AtomicUsize>,
    freed: Arc<Notify>,
}

impl Drop for ItemAgentSlot {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        self.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_holds_at_most_the_limit_and_frees_on_drop() {
        let gate = ItemAgentGate::new(2);
        let a = gate.acquire_blocking_for_test();
        let b = gate.acquire_blocking_for_test();
        assert_eq!(gate.live(), 2);
        assert!(!gate.has_slot());
        assert!(gate.try_acquire().is_none());
        drop(a);
        assert_eq!(gate.live(), 1);
        assert!(gate.has_slot());
        let c = gate.try_acquire().expect("a slot is free");
        assert_eq!(gate.live(), 2);
        drop(b);
        drop(c);
        assert_eq!(gate.live(), 0);
    }

    #[tokio::test]
    async fn acquire_waits_until_a_slot_frees() {
        let gate = Arc::new(ItemAgentGate::new(1));
        let held = gate.acquire().await;
        assert_eq!(gate.live(), 1);
        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let slot = gate.acquire().await;
                // Keep it until the end so the count is observed held.
                assert_eq!(gate.live(), 1);
                drop(slot);
            })
        };
        // Give the waiter a moment to block, then free the slot.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(held);
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("the waiter woke")
            .expect("task ok");
        assert_eq!(gate.live(), 0);
    }

    impl ItemAgentGate {
        /// Synchronous acquire for plain `#[test]`s (single-threaded).
        fn acquire_blocking_for_test(&self) -> ItemAgentSlot {
            let mut current = self.live.load(Ordering::SeqCst);
            loop {
                assert!(current < self.limit, "the gate would block here");
                if self
                    .live
                    .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return ItemAgentSlot {
                        live: self.live.clone(),
                        freed: self.freed.clone(),
                    };
                }
                current = self.live.load(Ordering::SeqCst);
            }
        }
    }
}
