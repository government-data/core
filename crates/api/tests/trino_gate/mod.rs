//! The queue's jobs that use Trino and Trino's queries, as fakes for the
//! tests of its coordinator's hold (0066, point 9).
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use application::{TrinoJobs, TrinoQueries};
use async_trait::async_trait;

/// `running` jobs that use Trino; whether they are held, and how many times
/// a hold was taken.
#[derive(Default)]
pub(crate) struct Jobs {
    pub(crate) running: AtomicU64,
    pub(crate) held: AtomicBool,
    pub(crate) holds: AtomicU64,
}

#[async_trait]
impl TrinoJobs for Jobs {
    async fn hold(&self, _machine: &str, _ttl: Duration) -> Result<u64, String> {
        self.held.store(true, Ordering::SeqCst);
        self.holds.fetch_add(1, Ordering::SeqCst);
        Ok(self.running.load(Ordering::SeqCst))
    }

    async fn release(&self) -> Result<bool, String> {
        Ok(self.held.swap(false, Ordering::SeqCst))
    }
}

/// Trino running `active` queries, or not answering while `down`.
#[derive(Default)]
pub(crate) struct Queries {
    pub(crate) active: AtomicU64,
    pub(crate) down: AtomicBool,
}

#[async_trait]
impl TrinoQueries for Queries {
    async fn active(&self) -> Result<u64, String> {
        if self.down.load(Ordering::SeqCst) {
            return Err("connection refused".into());
        }
        Ok(self.active.load(Ordering::SeqCst))
    }
}
