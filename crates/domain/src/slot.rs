//! A value kept in memory and shared between clones: what the server applied
//! (its mail, directory and OAuth providers, the settings it read). One
//! installation is one organisation (decision 0076), so there is one value,
//! not one per tenant.

use std::sync::{Arc, PoisonError, RwLock};

/// One value in memory, shared by every clone; empty until set.
pub struct Slot<T>(Arc<RwLock<Option<T>>>);

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T> std::fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("set", &self.contains())
            .finish()
    }
}

impl<T> Slot<T> {
    /// Whether it was set.
    pub fn contains(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Replaces it.
    pub fn set(&self, value: T) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(value);
    }

    /// Changes it in place, starting from `T::default()` if it was empty.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: Default,
    {
        let mut value = self.0.write().unwrap_or_else(PoisonError::into_inner);
        f(value.get_or_insert_with(T::default))
    }
}

impl<T: Clone> Slot<T> {
    /// A copy of it, if it was set.
    pub fn get(&self) -> Option<T> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_value() {
        let slot: Slot<u32> = Slot::default();
        let other = slot.clone();
        assert_eq!((slot.get(), slot.contains()), (None, false));
        other.set(3);
        assert_eq!(slot.get(), Some(3));
        assert_eq!(
            slot.update(|v| {
                *v += 1;
                *v
            }),
            4
        );
        let empty: Slot<u32> = Slot::default();
        assert_eq!(empty.update(|v| *v), 0);
        assert!(empty.contains());
    }
}
