//! Dolt's restart to take a renewed certificate (decision 0066, point 11):
//! Dolt reads its certificate only when it starts, so it is restarted, and
//! only when free. The API is its only client, so the API is where the
//! first of the two checks is: every operation of the catalog holds an
//! [`Entry`] for as long as it runs, and a hold is taken only when none is
//! in flight; from then on new operations wait (up to the gate's wait) for
//! the release, or for the hold to expire by itself.
//!
//! One gate per process: with one API per installation (0065) it is the
//! only one. Several API replicas would need it shared (the database).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

/// More operations than ever run at once: a hold takes them all.
const ALL: u32 = 1 << 20;

/// What a refused operation says: Dolt did not come back within the wait.
pub const RESTARTING: &str = "the rules' store is restarting; try again in a moment";

/// An operation in flight; the hold waits for none.
#[derive(Debug)]
pub(super) struct Entry {
    _permit: OwnedSemaphorePermit,
}

/// The gate of one process.
#[derive(Debug)]
pub(super) struct Gate {
    permits: Arc<Semaphore>,
    held: Arc<Mutex<Option<Held>>>,
    next: AtomicU64,
    wait: Duration,
}

#[derive(Debug)]
struct Held {
    id: u64,
    _all: OwnedSemaphorePermit,
}

impl Gate {
    /// New operations wait up to `wait` while it is held.
    pub(super) fn new(wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(ALL as usize)),
            held: Arc::new(Mutex::new(None)),
            next: AtomicU64::new(1),
            wait,
        }
    }

    /// Enter for one operation, waiting while it is held; refused with
    /// [`RESTARTING`] after the wait.
    pub(super) async fn enter(&self) -> Result<Entry, &'static str> {
        let permit = self.permits.clone().acquire_owned();
        match tokio::time::timeout(self.wait, permit).await {
            Ok(Ok(permit)) => Ok(Entry { _permit: permit }),
            _ => Err(RESTARTING),
        }
    }

    /// Holds it for `ttl` if no operation is in flight (a hold in force is
    /// extended); otherwise how many are.
    pub(super) fn hold(&self, ttl: Duration) -> Result<(), u32> {
        let mut held = lock(&self.held);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        match held.as_mut() {
            Some(h) => h.id = id,
            None => match self.permits.clone().try_acquire_many_owned(ALL) {
                Ok(all) => *held = Some(Held { id, _all: all }),
                Err(TryAcquireError::NoPermits) => {
                    let free = u32::try_from(self.permits.available_permits()).unwrap_or(ALL);
                    return Err(ALL.saturating_sub(free).max(1));
                }
                Err(TryAcquireError::Closed) => return Err(0),
            },
        }
        drop(held);
        let slot = Arc::clone(&self.held);
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let mut held = lock(&slot);
            if held.as_ref().is_some_and(|h| h.id == id) {
                tracing::warn!("Dolt's hold expired without a release");
                *held = None;
            }
        });
        Ok(())
    }

    /// Ends the hold: whether one was in force.
    pub(super) fn release(&self) -> bool {
        lock(&self.held).take().is_some()
    }
}

fn lock(held: &Mutex<Option<Held>>) -> MutexGuard<'_, Option<Held>> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(50);
    const TTL: Duration = Duration::from_secs(300);

    #[tokio::test]
    async fn an_operation_in_flight_keeps_it_from_being_held() {
        let gate = Gate::new(SHORT);
        let a = gate.enter().await.unwrap();
        let b = gate.enter().await.unwrap();
        assert_eq!(gate.hold(TTL), Err(2), "two in flight");
        drop(a);
        assert_eq!(gate.hold(TTL), Err(1));
        drop(b);
        assert_eq!(gate.hold(TTL), Ok(()), "none in flight: held");
        assert_eq!(gate.hold(TTL), Ok(()), "asked again: still held");
        assert!(gate.release());
        assert!(!gate.release(), "nothing in force any more");
    }

    #[tokio::test]
    async fn while_held_operations_wait_then_are_refused() {
        let gate = Arc::new(Gate::new(Duration::from_millis(300)));
        gate.hold(TTL).unwrap();
        assert_eq!(
            Gate::new(SHORT).enter().await.map(|_| ()),
            Ok(()),
            "another gate is not held"
        );
        let refused = Gate::enter(&gate).await;
        assert_eq!(refused.map(|_| ()), Err(RESTARTING), "held past the wait");

        // Released while one waits: it goes on.
        let waiting = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.enter().await.map(|_| ()) })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(gate.release());
        assert_eq!(waiting.await.unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn a_hold_expires_by_itself() {
        let ms = Duration::from_millis;
        let gate = Gate::new(ms(10));
        gate.hold(ms(300)).unwrap();
        tokio::time::sleep(ms(200)).await;
        // Asked again: extended from now, the first timer no longer ends it.
        gate.hold(ms(300)).unwrap();
        tokio::time::sleep(ms(150)).await;
        assert!(gate.enter().await.is_err(), "still held at 350 ms");
        tokio::time::sleep(ms(250)).await;
        assert!(gate.enter().await.is_ok(), "expired at 500 ms");
        assert!(!gate.release(), "nothing left to release");
    }
}
