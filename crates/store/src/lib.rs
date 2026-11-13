//! Persistence for Choosey decisions.
//!
//! [`Store`] is the interface the web app uses. [`MemoryStore`] keeps everything in a `HashMap`
//! (local development and tests).

mod memory;

pub use memory::MemoryStore;

use async_trait::async_trait;
use choosey_core::model::Decision;

/// Errors a store can return. Messages never include ids or secrets, so they are safe to log.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// `create` found an existing decision with the same id (practically impossible with
    /// 128-bit random ids, but checked anyway).
    #[error("a decision with this id already exists")]
    AlreadyExists,
    /// `put` found a different version than expected (or the decision was deleted): someone else
    /// changed it since we read it.
    #[error("the decision was changed by someone else")]
    Conflict,
    /// The backing service failed.
    #[error("storage backend error: {0}")]
    Backend(String),
}

/// Storage for decisions, keyed by `view_id`.
///
/// `#[async_trait]` lets us use `async fn` here and still store the store as `Arc<dyn Store>`,
/// so the backend can be chosen at runtime.
#[async_trait]
pub trait Store: Send + Sync {
    /// Save a new decision. Fails with [`StoreError::AlreadyExists`] if the id is taken.
    async fn create(&self, decision: &Decision) -> Result<(), StoreError>;

    /// Load a decision, or `None` if there is no such decision.
    async fn get(&self, view_id: &str) -> Result<Option<Decision>, StoreError>;

    /// Replace a decision only if the stored copy still has `expected_version` (optimistic
    /// concurrency). The caller bumps `decision.version` first, see
    /// [`Decision::bump_version`](choosey_core::model::Decision::bump_version).
    async fn put(&self, decision: &Decision, expected_version: u64) -> Result<(), StoreError>;

    /// Delete a decision. Deleting something that doesn't exist is not an error.
    async fn delete(&self, view_id: &str) -> Result<(), StoreError>;
}

/// Behaviour every [`Store`] implementation must have. Used by the tests of each implementation.
#[doc(hidden)]
pub mod conformance {
    use super::*;

    fn sample(view_id: &str) -> Decision {
        let mut d = Decision::new(view_id.into(), "hash".into(), "Which flat?".into(), 1);
        let c = d.add_criterion(
            "Price".into(),
            choosey_core::model::CriterionKind::Numeric {
                direction: choosey_core::model::Direction::LowerBetter,
                unit: "per month".into(),
                bounds: Some(choosey_core::model::Bounds {
                    worst: 3000.0,
                    best: 1000.0,
                }),
            },
            7,
        );
        let o = d.add_option("Elm Street".into(), "near the park".into());
        d.owner_scores_mut().entry(o).or_default().insert(c, 1850.5);
        d
    }

    /// Run the shared checks against `store`. `prefix` keeps ids unique across runs.
    pub async fn check(store: &dyn Store, prefix: &str) {
        let id = format!("{prefix}conformance");
        let mut d = sample(&id);

        // Missing decisions are `None`, not an error.
        assert!(
            store
                .get(&format!("{prefix}missing"))
                .await
                .unwrap()
                .is_none()
        );

        // Round trip.
        store.create(&d).await.unwrap();
        assert_eq!(store.get(&id).await.unwrap().as_ref(), Some(&d));

        // Creating the same id twice fails.
        assert!(matches!(
            store.create(&d).await,
            Err(StoreError::AlreadyExists)
        ));

        // A put with the right expected version succeeds...
        let stale = d.clone();
        d.title = "Which flat, really?".into();
        let expected = d.bump_version(2);
        store.put(&d, expected).await.unwrap();
        assert_eq!(store.get(&id).await.unwrap().unwrap().title, d.title);

        // ...and a put based on an old copy is rejected.
        let mut stale = stale;
        let expected = stale.bump_version(3);
        assert!(matches!(
            store.put(&stale, expected).await,
            Err(StoreError::Conflict)
        ));
        assert_eq!(store.get(&id).await.unwrap().unwrap().version, 1);

        // Delete, then it's gone; deleting again is fine; put on a deleted decision conflicts.
        store.delete(&id).await.unwrap();
        assert!(store.get(&id).await.unwrap().is_none());
        store.delete(&id).await.unwrap();
        let expected = d.bump_version(4);
        assert!(matches!(
            store.put(&d, expected).await,
            Err(StoreError::Conflict)
        ));
    }
}
