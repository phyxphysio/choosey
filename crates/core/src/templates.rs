//! Built-in decision templates: static lists of suggested criteria.

use crate::model::{CriterionKind, Decision, Direction};

/// A suggested criterion in a template.
#[derive(Debug, Clone, Copy)]
pub struct TemplateCriterion {
    pub name: &'static str,
    pub kind: TemplateKind,
    pub weight: u8,
}

/// The kind of a template criterion, in a `const`-friendly form.
#[derive(Debug, Clone, Copy)]
pub enum TemplateKind {
    Rated,
    Numeric {
        direction: Direction,
        unit: &'static str,
    },
}

/// A starting point for a new decision.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub criteria: &'static [TemplateCriterion],
}

const fn rated(name: &'static str, weight: u8) -> TemplateCriterion {
    TemplateCriterion {
        name,
        kind: TemplateKind::Rated,
        weight,
    }
}

const fn lower(name: &'static str, unit: &'static str, weight: u8) -> TemplateCriterion {
    TemplateCriterion {
        name,
        kind: TemplateKind::Numeric {
            direction: Direction::LowerBetter,
            unit,
        },
        weight,
    }
}

const fn higher(name: &'static str, unit: &'static str, weight: u8) -> TemplateCriterion {
    TemplateCriterion {
        name,
        kind: TemplateKind::Numeric {
            direction: Direction::HigherBetter,
            unit,
        },
        weight,
    }
}

/// All templates, in the order they are offered.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "home",
        title: "Rent or buy a home",
        description: "Compare flats or houses on price, commute, light and more.",
        criteria: &[
            lower("Price", "per month", 10),
            lower("Commute", "minutes", 8),
            rated("Sun", 5),
            rated("Warmth", 5),
            higher("Bedrooms", "rooms", 6),
            rated("Kitchen", 5),
            rated("Storage", 4),
            rated("Parking", 3),
            rated("Neighbourhood", 7),
        ],
    },
    Template {
        id: "retire",
        title: "Where to retire",
        description: "Weigh up towns on cost, family, healthcare and lifestyle.",
        criteria: &[
            lower("Cost of living", "per month", 9),
            rated("Near family", 8),
            rated("Healthcare access", 8),
            rated("Climate", 6),
            rated("Community", 6),
            rated("Walkability", 5),
            rated("Activities", 5),
        ],
    },
    Template {
        id: "job",
        title: "Job offer",
        description: "Compare offers on pay, growth, team and flexibility.",
        criteria: &[
            higher("Pay", "per year", 9),
            rated("Growth", 7),
            lower("Commute", "minutes", 6),
            rated("Team", 7),
            rated("Flexibility", 6),
            rated("Stability", 6),
        ],
    },
    Template {
        id: "blank",
        title: "Blank",
        description: "Start from scratch with your own criteria.",
        criteria: &[],
    },
];

/// Look up a template by id.
pub fn find(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

impl Template {
    /// Add this template's criteria to a decision.
    pub fn apply(&self, decision: &mut Decision) {
        decision.template_id = Some(self.id.to_string());
        for c in self.criteria {
            let kind = match c.kind {
                TemplateKind::Rated => CriterionKind::Rated,
                TemplateKind::Numeric { direction, unit } => CriterionKind::Numeric {
                    direction,
                    unit: unit.to_string(),
                    bounds: None,
                },
            };
            decision.add_criterion(c.name.to_string(), kind, c.weight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid() {
        for t in TEMPLATES {
            assert!(find(t.id).is_some());
            assert!(t.criteria.len() <= crate::validate::MAX_CRITERIA);
            for c in t.criteria {
                assert!((1..=10).contains(&c.weight), "{}", c.name);
            }
        }
        let mut d = Decision::new("v".into(), "h".into(), "t".into(), 0);
        find("home").unwrap().apply(&mut d);
        assert_eq!(d.criteria.len(), 9);
        assert_eq!(d.template_id.as_deref(), Some("home"));
    }
}
