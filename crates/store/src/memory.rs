//! In-memory store for local development and tests. Data is lost when the process exits.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use choosey_core::model::Decision;

use crate::{Store, StoreError};

/// A `HashMap` behind a mutex. The lock is never held across an `.await`, so a plain
/// `std::sync::Mutex` is the right tool (and simpler than an async mutex).
#[derive(Default)]
pub struct MemoryStore {
    decisions: Mutex<HashMap<String, Decision>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Decision>> {
        // A poisoned lock means another thread panicked mid-update. The map itself is still
        // usable, so carry on rather than taking the whole server down.
        self.decisions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl Store for MemoryStore {
    async fn create(&self, decision: &Decision) -> Result<(), StoreError> {
        let mut map = self.lock();
        if map.contains_key(&decision.view_id) {
            return Err(StoreError::AlreadyExists);
        }
        map.insert(decision.view_id.clone(), decision.clone());
        Ok(())
    }

    async fn get(&self, view_id: &str) -> Result<Option<Decision>, StoreError> {
        Ok(self.lock().get(view_id).cloned())
    }

    async fn put(&self, decision: &Decision, expected_version: u64) -> Result<(), StoreError> {
        let mut map = self.lock();
        match map.get_mut(&decision.view_id) {
            Some(stored) if stored.version == expected_version => {
                *stored = decision.clone();
                Ok(())
            }
            _ => Err(StoreError::Conflict),
        }
    }

    async fn delete(&self, view_id: &str) -> Result<(), StoreError> {
        self.lock().remove(view_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn conformance() {
        crate::conformance::check(&MemoryStore::new(), "mem").await;
    }
}
