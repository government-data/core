//! Each account's Actor, in memory (ADR 0013): building it costs a few queries
//! (the account's policies and its groups') and it is used on every request. It is kept per
//! (tenant, account, role) and reloaded when what someone can do changes in
//! that tenant (Postgres's `permissions` notice, or a change made in this very
//! process), or after a while, in case a notice was lost.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use iam::Actor;

/// How long an Actor lives without a change notice, in case one was lost.
pub const TTL: Duration = Duration::from_secs(60);

/// How many accounts are kept at most; beyond that it starts over.
const MAX: usize = 10_000;

type Key = (String, String, String);

struct Entry {
    actor: Actor,
    generation: u64,
    at: Instant,
}

/// The Actors in memory and each tenant's permission generation.
pub struct ActorCache {
    ttl: Duration,
    entries: Mutex<HashMap<Key, Entry>>,
    generations: Mutex<HashMap<String, u64>>,
    /// The generation of every tenant (bumped by a notice without tenant).
    global: Mutex<u64>,
}

impl Default for ActorCache {
    fn default() -> Self {
        Self::new(TTL)
    }
}

impl std::fmt::Debug for ActorCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorCache")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl ActorCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            global: Mutex::new(0),
        }
    }

    /// A tenant's permission generation: it goes up with every change. It is
    /// read before loading an Actor, so a stale one is not stored if it
    /// changed while loading.
    pub fn generation(&self, tenant: &str) -> u64 {
        let global = *self.global.lock().unwrap_or_else(|e| e.into_inner());
        let own = self
            .generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .copied()
            .unwrap_or(0);
        global + own
    }

    /// The stored Actor, if still valid.
    pub fn get(&self, tenant: &str, account: &str, role: &str) -> Option<Actor> {
        let generation = self.generation(tenant);
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(&(tenant.to_string(), account.to_string(), role.to_string()))
            .filter(|e| e.generation == generation && e.at.elapsed() < self.ttl)
            .map(|e| e.actor.clone())
    }

    /// Store an Actor loaded at generation `generation`.
    pub fn put(&self, tenant: &str, account: &str, role: &str, actor: Actor, generation: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX {
            entries.clear();
        }
        entries.insert(
            (tenant.to_string(), account.to_string(), role.to_string()),
            Entry {
                actor,
                generation,
                at: Instant::now(),
            },
        );
    }

    /// What someone can do changed in `tenant` (or in all of them, with
    /// `None`): the stored Actors are no longer valid.
    pub fn invalidate(&self, tenant: Option<&str>) {
        match tenant {
            Some(t) => {
                *self
                    .generations
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(t.to_string())
                    .or_insert(0) += 1;
            }
            None => *self.global.lock().unwrap_or_else(|e| e.into_inner()) += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str) -> Actor {
        Actor::account(id, false, Default::default())
    }

    #[test]
    fn stored_and_invalidated_per_tenant() {
        let c = ActorCache::default();
        let g = c.generation("t1");
        c.put("t1", "ana", "auditor", actor("ana"), g);
        c.put("t2", "luis", "auditor", actor("luis"), c.generation("t2"));
        assert!(c.get("t1", "ana", "auditor").is_some());
        // Another role is another Actor.
        assert!(c.get("t1", "ana", "admin").is_none());
        c.invalidate(Some("t1"));
        assert!(c.get("t1", "ana", "auditor").is_none());
        assert!(c.get("t2", "luis", "auditor").is_some());
        c.invalidate(None);
        assert!(c.get("t2", "luis", "auditor").is_none());
    }

    #[test]
    fn one_loaded_before_a_change_is_not_valid() {
        let c = ActorCache::default();
        let before = c.generation("t1");
        c.invalidate(Some("t1"));
        c.put("t1", "ana", "x", actor("ana"), before);
        assert!(c.get("t1", "ana", "x").is_none());
    }

    #[test]
    fn expires_by_time() {
        let c = ActorCache::new(Duration::from_millis(1));
        c.put("t1", "ana", "x", actor("ana"), c.generation("t1"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(c.get("t1", "ana", "x").is_none());
    }
}
