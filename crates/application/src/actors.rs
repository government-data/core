//! Each account's Actor, in memory (ADR 0013): building it costs a few queries
//! (the account's policies and its groups') and it is used on every request. It is kept per
//! (account, role) and reloaded when what someone can do changes (Postgres's
//! `permissions` notice, or a change made in this very process), or after a
//! while, in case a notice was lost.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use iam::Actor;

/// How long an Actor lives without a change notice, in case one was lost.
pub const TTL: Duration = Duration::from_secs(60);

/// How many accounts are kept at most; beyond that it starts over.
const MAX: usize = 10_000;

type Key = (String, String);

struct Entry {
    actor: Actor,
    generation: u64,
    at: Instant,
}

/// The Actors in memory and the permissions' generation.
pub struct ActorCache {
    ttl: Duration,
    entries: Mutex<HashMap<Key, Entry>>,
    generation: Mutex<u64>,
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
            generation: Mutex::new(0),
        }
    }

    /// The permissions' generation: it goes up with every change. It is read
    /// before loading an Actor, so a stale one is not stored if it changed
    /// while loading.
    pub fn generation(&self) -> u64 {
        *self.generation.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The stored Actor, if still valid.
    pub fn get(&self, account: &str, role: &str) -> Option<Actor> {
        let generation = self.generation();
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(&(account.to_string(), role.to_string()))
            .filter(|e| e.generation == generation && e.at.elapsed() < self.ttl)
            .map(|e| e.actor.clone())
    }

    /// Store an Actor loaded at generation `generation`.
    pub fn put(&self, account: &str, role: &str, actor: Actor, generation: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX {
            entries.clear();
        }
        entries.insert(
            (account.to_string(), role.to_string()),
            Entry {
                actor,
                generation,
                at: Instant::now(),
            },
        );
    }

    /// What someone can do changed: the stored Actors are no longer valid.
    pub fn invalidate(&self) {
        *self.generation.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str) -> Actor {
        Actor::account(id, false, Default::default())
    }

    #[test]
    fn stored_and_invalidated() {
        let c = ActorCache::default();
        c.put("ana", "auditor", actor("ana"), c.generation());
        assert!(c.get("ana", "auditor").is_some());
        // Another role is another Actor.
        assert!(c.get("ana", "admin").is_none());
        c.invalidate();
        assert!(c.get("ana", "auditor").is_none());
    }

    #[test]
    fn one_loaded_before_a_change_is_not_valid() {
        let c = ActorCache::default();
        let before = c.generation();
        c.invalidate();
        c.put("ana", "x", actor("ana"), before);
        assert!(c.get("ana", "x").is_none());
    }

    #[test]
    fn expires_by_time() {
        let c = ActorCache::new(Duration::from_millis(1));
        c.put("ana", "x", actor("ana"), c.generation());
        std::thread::sleep(Duration::from_millis(5));
        assert!(c.get("ana", "x").is_none());
    }
}
