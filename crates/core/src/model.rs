//! Domain types: a decision with its criteria, options and raw scores.
//!
//! These types are what gets stored (serialised with serde), so changing them means thinking about
//! existing data. Bump [`SCHEMA_VERSION`] when the stored shape changes incompatibly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Version of the stored shape of [`Decision`].
pub const SCHEMA_VERSION: u32 = 1;

/// The only participant in the MVP. Scores are keyed by participant so that several people can
/// score the same decision later without changing the data model.
pub const OWNER: &str = "owner";

/// Identifier of a criterion within one decision, e.g. `"c3"`.
pub type CriterionId = String;
/// Identifier of an option within one decision, e.g. `"o2"`.
pub type OptionId = String;
/// Identifier of a participant, e.g. `"owner"`.
pub type ParticipantId = String;

/// What the user typed for one option on one criterion: a 0-10 rating for [`CriterionKind::Rated`]
/// or a real-world value (price, minutes...) for [`CriterionKind::Numeric`]. Raw values are stored,
/// never normalised scores, so changing bounds or weights recalculates everything.
pub type RawValue = f64;

/// Scores for one participant: option -> criterion -> raw value. A missing entry means "not scored".
pub type ScoreSheet = BTreeMap<OptionId, BTreeMap<CriterionId, RawValue>>;

/// Whether bigger raw values are better (pay) or worse (price, commute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    HigherBetter,
    LowerBetter,
}

/// Fixed scaling bounds for a numeric criterion. `worst` maps to 0 and `best` maps to 10; values
/// outside are clamped. For lower-is-better criteria `worst` is simply the larger number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub worst: f64,
    pub best: f64,
}

/// How a criterion is scored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CriterionKind {
    /// The user rates each option from 0 to 10 (half steps allowed).
    Rated,
    /// The user enters a raw value which is scaled to 0-10, either with fixed bounds or min-max
    /// across the options.
    Numeric {
        direction: Direction,
        unit: String,
        bounds: Option<Bounds>,
    },
}

/// Something that matters to the decision, e.g. "Commute".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    pub id: CriterionId,
    pub name: String,
    pub kind: CriterionKind,
    /// Importance from 0 to 10. 0 means "ignore this criterion".
    pub weight: u8,
}

/// One of the things being chosen between, e.g. a flat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionItem {
    pub id: OptionId,
    pub name: String,
    #[serde(default)]
    pub notes: String,
}

/// Someone who scores options. The MVP always has exactly one, [`OWNER`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub id: ParticipantId,
    pub name: String,
}

/// A whole decision. Stored as one item.
///
/// Criteria and options are kept in display order: their position is their index in the `Vec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub schema_version: u32,
    /// Public identifier used in URLs (128-bit random, base62).
    pub view_id: String,
    /// Incremented on every successful write, for optimistic concurrency.
    pub version: u64,
    pub title: String,
    pub template_id: Option<String>,
    /// Unix timestamps in seconds.
    pub created_at: i64,
    pub updated_at: i64,
    /// Hex SHA-256 of the edit secret. The secret itself is never stored.
    pub edit_secret_hash: String,
    pub criteria: Vec<Criterion>,
    pub options: Vec<OptionItem>,
    /// participant -> option -> criterion -> raw value.
    pub scores: BTreeMap<ParticipantId, ScoreSheet>,
    pub participants: Vec<Participant>,
    /// Counter used to mint short, unique criterion/option ids within this decision.
    pub next_id: u32,
}

impl Decision {
    /// A new, empty decision owned by a single participant.
    pub fn new(view_id: String, edit_secret_hash: String, title: String, now: i64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            view_id,
            version: 0,
            title,
            template_id: None,
            created_at: now,
            updated_at: now,
            edit_secret_hash,
            criteria: Vec::new(),
            options: Vec::new(),
            scores: BTreeMap::from([(OWNER.to_string(), ScoreSheet::new())]),
            participants: vec![Participant {
                id: OWNER.to_string(),
                name: "Owner".to_string(),
            }],
            next_id: 1,
        }
    }

    /// Mint a new id with the given prefix (`"c"` for criteria, `"o"` for options).
    fn mint_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Append a criterion and return its id.
    pub fn add_criterion(&mut self, name: String, kind: CriterionKind, weight: u8) -> CriterionId {
        let id = self.mint_id("c");
        self.criteria.push(Criterion {
            id: id.clone(),
            name,
            kind,
            weight: weight.min(10),
        });
        id
    }

    /// Append an option and return its id.
    pub fn add_option(&mut self, name: String, notes: String) -> OptionId {
        let id = self.mint_id("o");
        self.options.push(OptionItem {
            id: id.clone(),
            name,
            notes,
        });
        id
    }

    pub fn criterion(&self, id: &str) -> Option<&Criterion> {
        self.criteria.iter().find(|c| c.id == id)
    }

    pub fn criterion_mut(&mut self, id: &str) -> Option<&mut Criterion> {
        self.criteria.iter_mut().find(|c| c.id == id)
    }

    pub fn option_mut(&mut self, id: &str) -> Option<&mut OptionItem> {
        self.options.iter_mut().find(|o| o.id == id)
    }

    /// Remove a criterion and every score given for it. Returns false if it didn't exist.
    pub fn remove_criterion(&mut self, id: &str) -> bool {
        let before = self.criteria.len();
        self.criteria.retain(|c| c.id != id);
        self.clear_criterion_scores(id);
        self.criteria.len() != before
    }

    /// Forget every score for one criterion (used when it is deleted or its kind changes, since the
    /// old raw values no longer mean anything).
    pub fn clear_criterion_scores(&mut self, id: &str) {
        for sheet in self.scores.values_mut() {
            for per_option in sheet.values_mut() {
                per_option.remove(id);
            }
        }
    }

    /// Remove an option and its scores. Returns false if it didn't exist.
    pub fn remove_option(&mut self, id: &str) -> bool {
        let before = self.options.len();
        self.options.retain(|o| o.id != id);
        for sheet in self.scores.values_mut() {
            sheet.remove(id);
        }
        self.options.len() != before
    }

    /// Move a criterion one place up or down in display order (no-op at either end).
    pub fn move_criterion(&mut self, id: &str, up: bool) {
        if let Some(i) = self.criteria.iter().position(|c| c.id == id) {
            let j = if up { i.checked_sub(1) } else { Some(i + 1) };
            if let Some(j) = j.filter(|&j| j < self.criteria.len()) {
                self.criteria.swap(i, j);
            }
        }
    }

    /// The owner's score sheet (empty if none yet).
    pub fn owner_scores(&self) -> &ScoreSheet {
        static EMPTY: ScoreSheet = ScoreSheet::new();
        self.scores.get(OWNER).unwrap_or(&EMPTY)
    }

    /// Mutable access to the owner's score sheet, creating it if needed.
    pub fn owner_scores_mut(&mut self) -> &mut ScoreSheet {
        self.scores.entry(OWNER.to_string()).or_default()
    }

    /// Prepare for a conditional write: bump the version and timestamp, and return the version
    /// that the stored copy is expected to have (the one we read).
    pub fn bump_version(&mut self, now: i64) -> u64 {
        let expected = self.version;
        self.version += 1;
        self.updated_at = now;
        expected
    }
}
