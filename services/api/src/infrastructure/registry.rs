//! Per-game serialization.
//!
//! Within a process, every mutation of a game (move, join, resign) acquires
//! the game's lock first, so operations are strictly ordered. Across
//! processes the database row lock (`SELECT ... FOR UPDATE`) plus optimistic
//! `version` checks provide the same guarantee — this registry is an
//! optimization and a first line of defence, not the source of truth.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct GameRegistry {
    locks: Arc<RwLock<HashMap<Uuid, Arc<Mutex<()>>>>>,
}

impl GameRegistry {
    /// Acquire the exclusive lock guarding `game_id`.
    pub async fn lock_for(&self, game_id: Uuid) -> OwnedMutexGuard<()> {
        let existing = {
            let map = self.locks.read().unwrap_or_else(|e| e.into_inner());
            map.get(&game_id).cloned()
        };
        let mutex = match existing {
            Some(m) => m,
            None => {
                let mut map = self.locks.write().unwrap_or_else(|e| e.into_inner());
                map.entry(game_id).or_default().clone()
            }
        };
        mutex.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lock_is_exclusive() {
        let registry = GameRegistry::default();
        let game = Uuid::new_v4();
        let guard = registry.lock_for(game).await;
        let handle = tokio::spawn({
            let registry = registry.clone();
            async move {
                let _g = registry.lock_for(game).await;
                // Only reached after the first guard drops.
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!handle.is_finished(), "second lock must wait");
        drop(guard);
        handle.await.expect("lock task");
    }
}
