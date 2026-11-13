//! Choosey domain types and scoring. Pure functions only: no async, no I/O.
//!
//! - [`model`]: the stored data (decisions, criteria, options, raw scores).
//! - [`scoring`]: normalisation, weighted totals, coverage and ranking.
//! - [`roc`]: suggested weights from a ranking (Rank Order Centroid).
//! - [`templates`]: built-in starting points.
//! - [`ids`]: base62 ids and edit-secret hashing.
//! - [`validate`]: input limits and cleaning.

pub mod ids;
pub mod model;
pub mod roc;
pub mod scoring;
pub mod templates;
pub mod validate;

/// The app's name, used for page titles, file names and resource names. Kept in one place so it
/// is easy to rename.
pub const APP_NAME: &str = "choosey";
