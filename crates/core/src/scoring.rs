//! The scoring model: normalise raw values to 0-10, combine them with weights, and rank.
//!
//! Everything here is a pure function of the criteria, options and one participant's scores.
//! See PLAN.md section 2 for the maths.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::model::{
    Bounds, Criterion, CriterionId, CriterionKind, Direction, OptionId, OptionItem, ScoreSheet,
};

/// Why results can't be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScoreError {
    #[error("there are no criteria yet")]
    NoCriteria,
    #[error("every criterion has weight 0, so nothing counts")]
    AllWeightsZero,
}

/// The result for one option.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionResult {
    pub option_id: OptionId,
    /// 0-100, counting only the criteria this option has been scored on. `None` if it hasn't been
    /// scored on any criterion that has a non-zero weight.
    pub total: Option<f64>,
    /// Share of the total weight (0.0-1.0) that this option has scores for. Below 1.0 means the
    /// total is based on incomplete information and the UI must say so.
    pub coverage: f64,
    /// One entry per criterion, in criterion order.
    pub per_criterion: Vec<CriterionResult>,
}

impl OptionResult {
    /// True when some weighted criterion has no score for this option.
    pub fn is_incomplete(&self) -> bool {
        self.coverage < 1.0
    }
}

/// How one criterion contributed to one option's result.
#[derive(Debug, Clone, PartialEq)]
pub struct CriterionResult {
    pub criterion_id: CriterionId,
    /// Normalised score 0-10, or `None` if not scored.
    pub score: Option<f64>,
    /// Points (out of 100) this criterion adds to a bar where 100 = a perfect score on every
    /// weighted criterion: `w * s / Σ_all(w * 10) * 100`. Unscored criteria contribute 0 here, which
    /// is why the bar must be shown alongside the coverage badge.
    pub contribution: f64,
}

/// Ranked results, best first.
#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    pub ranked: Vec<OptionResult>,
}

/// Normalise one raw value with fixed bounds: `10 * clamp((x - worst) / (best - worst), 0, 1)`.
/// Works for both directions because for lower-is-better `worst > best`.
pub fn normalise_bounded(x: f64, bounds: Bounds) -> f64 {
    let span = bounds.best - bounds.worst;
    if span == 0.0 || !span.is_finite() {
        // Degenerate bounds can't separate options; treat everyone as best (input validation
        // rejects these, so this is just a guard against division by zero).
        return 10.0;
    }
    10.0 * ((x - bounds.worst) / span).clamp(0.0, 1.0)
}

/// Normalise every option's value for one criterion. Returns option id -> score (0-10) for the
/// options that have a finite value; options without a value are absent.
pub fn normalise_criterion(
    criterion: &Criterion,
    options: &[OptionItem],
    scores: &ScoreSheet,
) -> BTreeMap<OptionId, f64> {
    // Collect (option, raw value) for the options that have a usable value.
    let values: Vec<(&OptionId, f64)> = options
        .iter()
        .filter_map(|o| {
            let x = *scores.get(&o.id)?.get(&criterion.id)?;
            x.is_finite().then_some((&o.id, x))
        })
        .collect();

    match &criterion.kind {
        // Ratings are already 0-10. Clamp in case stored data predates validation.
        CriterionKind::Rated => values
            .into_iter()
            .map(|(id, x)| (id.clone(), x.clamp(0.0, 10.0)))
            .collect(),
        CriterionKind::Numeric {
            bounds: Some(b), ..
        } => values
            .into_iter()
            .map(|(id, x)| (id.clone(), normalise_bounded(x, *b)))
            .collect(),
        CriterionKind::Numeric {
            direction,
            bounds: None,
            ..
        } => {
            // Min-max across the options that have a value.
            let min = values.iter().map(|&(_, x)| x).fold(f64::INFINITY, f64::min);
            let max = values
                .iter()
                .map(|&(_, x)| x)
                .fold(f64::NEG_INFINITY, f64::max);
            let span = max - min;
            values
                .into_iter()
                .map(|(id, x)| {
                    // If every value is the same (this includes a single value), the criterion
                    // can't separate the options, so nobody is penalised.
                    let s = if span <= 0.0 {
                        10.0
                    } else {
                        match direction {
                            Direction::HigherBetter => 10.0 * (x - min) / span,
                            Direction::LowerBetter => 10.0 * (max - x) / span,
                        }
                    };
                    (id.clone(), s)
                })
                .collect()
        }
    }
}

/// Score and rank every option.
pub fn compute(
    criteria: &[Criterion],
    options: &[OptionItem],
    scores: &ScoreSheet,
) -> Result<Results, ScoreError> {
    if criteria.is_empty() {
        return Err(ScoreError::NoCriteria);
    }
    let total_weight: u32 = criteria.iter().map(|c| u32::from(c.weight)).sum();
    if total_weight == 0 {
        return Err(ScoreError::AllWeightsZero);
    }
    let total_weight = f64::from(total_weight);

    // Normalised scores per criterion, computed once.
    let normalised: Vec<BTreeMap<OptionId, f64>> = criteria
        .iter()
        .map(|c| normalise_criterion(c, options, scores))
        .collect();

    let mut ranked: Vec<OptionResult> = options
        .iter()
        .map(|option| {
            let mut earned = 0.0; // Σ w * s over scored criteria
            let mut scored_weight = 0.0; // Σ w over scored criteria
            let per_criterion = criteria
                .iter()
                .zip(&normalised)
                .map(|(c, norm)| {
                    let score = norm.get(&option.id).copied();
                    let w = f64::from(c.weight);
                    let contribution = match score {
                        Some(s) => {
                            earned += w * s;
                            scored_weight += w;
                            w * s / (total_weight * 10.0) * 100.0
                        }
                        None => 0.0,
                    };
                    CriterionResult {
                        criterion_id: c.id.clone(),
                        score,
                        contribution,
                    }
                })
                .collect();
            OptionResult {
                option_id: option.id.clone(),
                total: (scored_weight > 0.0).then(|| 100.0 * earned / (scored_weight * 10.0)),
                coverage: scored_weight / total_weight,
                per_criterion,
            }
        })
        .collect();

    // Tie-breaker: the score on the highest-weighted criterion (the first one if several share it).
    let top_index = criteria
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| a.weight.cmp(&b.weight).then(ib.cmp(ia)))
        .map(|(i, _)| i)
        .unwrap_or(0);

    // `sort_by` is stable, so options that tie on everything keep their original order.
    ranked.sort_by(|a, b| {
        cmp_desc(a.total, b.total)
            .then_with(|| b.coverage.total_cmp(&a.coverage))
            .then_with(|| {
                cmp_desc(
                    a.per_criterion[top_index].score,
                    b.per_criterion[top_index].score,
                )
            })
    });

    Ok(Results { ranked })
}

/// Compare optional numbers so that higher values come first and `None` comes last.
fn cmp_desc(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.total_cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Decision, OWNER};

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn numeric(direction: Direction, bounds: Option<Bounds>) -> CriterionKind {
        CriterionKind::Numeric {
            direction,
            unit: String::new(),
            bounds,
        }
    }

    /// A decision with the given criteria (name, kind, weight) and option names.
    fn decision(criteria: &[(&str, CriterionKind, u8)], options: &[&str]) -> Decision {
        let mut d = Decision::new("v".into(), "h".into(), "t".into(), 0);
        for (name, kind, w) in criteria {
            d.add_criterion(name.to_string(), kind.clone(), *w);
        }
        for name in options {
            d.add_option(name.to_string(), String::new());
        }
        d
    }

    fn set(d: &mut Decision, option: usize, criterion: usize, x: f64) {
        let o = d.options[option].id.clone();
        let c = d.criteria[criterion].id.clone();
        d.owner_scores_mut().entry(o).or_default().insert(c, x);
    }

    fn results(d: &Decision) -> Results {
        compute(&d.criteria, &d.options, d.owner_scores()).unwrap()
    }

    fn by_name<'a>(d: &Decision, r: &'a Results, name: &str) -> &'a OptionResult {
        let id = &d.options.iter().find(|o| o.name == name).unwrap().id;
        r.ranked.iter().find(|x| &x.option_id == id).unwrap()
    }

    #[test]
    fn hand_calculated_example() {
        // Weights 5 and 3. A rated 8 and 4; B rated 6 and 10.
        // A: 100 * (5*8 + 3*4) / (8*10) = 100 * 52/80 = 65
        // B: 100 * (5*6 + 3*10) / 80 = 100 * 60/80 = 75
        let mut d = decision(
            &[
                ("x", CriterionKind::Rated, 5),
                ("y", CriterionKind::Rated, 3),
            ],
            &["A", "B"],
        );
        set(&mut d, 0, 0, 8.0);
        set(&mut d, 0, 1, 4.0);
        set(&mut d, 1, 0, 6.0);
        set(&mut d, 1, 1, 10.0);
        let r = results(&d);
        assert_eq!(r.ranked[0].option_id, d.options[1].id, "B wins");
        approx(r.ranked[0].total.unwrap(), 75.0);
        approx(r.ranked[1].total.unwrap(), 65.0);
        // Contributions for B: 5*6/80*100 = 37.5 and 3*10/80*100 = 37.5; they sum to the total.
        approx(r.ranked[0].per_criterion[0].contribution, 37.5);
        approx(r.ranked[0].per_criterion[1].contribution, 37.5);
        approx(r.ranked[0].coverage, 1.0);
    }

    #[test]
    fn min_max_higher_is_better() {
        let mut d = decision(
            &[("pay", numeric(Direction::HigherBetter, None), 1)],
            &["A", "B", "C"],
        );
        set(&mut d, 0, 0, 100.0);
        set(&mut d, 1, 0, 150.0);
        set(&mut d, 2, 0, 200.0);
        let n = normalise_criterion(&d.criteria[0], &d.options, d.owner_scores());
        approx(n[&d.options[0].id], 0.0);
        approx(n[&d.options[1].id], 5.0);
        approx(n[&d.options[2].id], 10.0);
    }

    #[test]
    fn min_max_lower_is_better() {
        let mut d = decision(
            &[("price", numeric(Direction::LowerBetter, None), 1)],
            &["A", "B", "C"],
        );
        set(&mut d, 0, 0, 100.0);
        set(&mut d, 1, 0, 175.0);
        set(&mut d, 2, 0, 200.0);
        let n = normalise_criterion(&d.criteria[0], &d.options, d.owner_scores());
        approx(n[&d.options[0].id], 10.0);
        approx(n[&d.options[1].id], 2.5);
        approx(n[&d.options[2].id], 0.0);
    }

    #[test]
    fn bounds_clamp_both_directions() {
        // Higher is better, 0 (worst) .. 50 (best).
        let b = Bounds {
            worst: 0.0,
            best: 50.0,
        };
        approx(normalise_bounded(25.0, b), 5.0);
        approx(normalise_bounded(-10.0, b), 0.0);
        approx(normalise_bounded(80.0, b), 10.0);
        // Lower is better: commute 60 min (worst) .. 10 min (best).
        let b = Bounds {
            worst: 60.0,
            best: 10.0,
        };
        approx(normalise_bounded(35.0, b), 5.0);
        approx(normalise_bounded(5.0, b), 10.0);
        approx(normalise_bounded(90.0, b), 0.0);
    }

    #[test]
    fn bounded_criterion_uses_bounds_not_min_max() {
        let b = Bounds {
            worst: 60.0,
            best: 10.0,
        };
        let mut d = decision(
            &[("commute", numeric(Direction::LowerBetter, Some(b)), 1)],
            &["A", "B"],
        );
        set(&mut d, 0, 0, 20.0);
        set(&mut d, 1, 0, 30.0);
        let n = normalise_criterion(&d.criteria[0], &d.options, d.owner_scores());
        approx(n[&d.options[0].id], 8.0);
        approx(n[&d.options[1].id], 6.0);
    }

    #[test]
    fn single_option_gets_full_marks_on_min_max() {
        let mut d = decision(
            &[("pay", numeric(Direction::HigherBetter, None), 4)],
            &["A"],
        );
        set(&mut d, 0, 0, 12345.0);
        let r = results(&d);
        approx(r.ranked[0].total.unwrap(), 100.0);
    }

    #[test]
    fn only_one_option_with_a_value_gets_full_marks() {
        let mut d = decision(
            &[("pay", numeric(Direction::HigherBetter, None), 4)],
            &["A", "B"],
        );
        set(&mut d, 1, 0, 50.0);
        let n = normalise_criterion(&d.criteria[0], &d.options, d.owner_scores());
        assert_eq!(n.len(), 1);
        approx(n[&d.options[1].id], 10.0);
    }

    #[test]
    fn all_values_the_same_gives_everyone_ten() {
        let mut d = decision(
            &[("price", numeric(Direction::LowerBetter, None), 2)],
            &["A", "B", "C"],
        );
        for o in 0..3 {
            set(&mut d, o, 0, 400.0);
        }
        let n = normalise_criterion(&d.criteria[0], &d.options, d.owner_scores());
        assert!(n.values().all(|&s| s == 10.0));
    }

    #[test]
    fn missing_scores_are_excluded_and_reported_as_coverage() {
        // Weights 6 and 4. A has both (10, 0); B only the first (5).
        // A: 100 * (60 + 0) / 100 = 60, coverage 1.0
        // B: 100 * 30 / 60 = 50, coverage 0.6; it is not treated as scoring 0 on the second.
        let mut d = decision(
            &[
                ("x", CriterionKind::Rated, 6),
                ("y", CriterionKind::Rated, 4),
            ],
            &["A", "B", "C"],
        );
        set(&mut d, 0, 0, 10.0);
        set(&mut d, 0, 1, 0.0);
        set(&mut d, 1, 0, 5.0);
        let r = results(&d);
        let a = by_name(&d, &r, "A");
        let b = by_name(&d, &r, "B");
        let c = by_name(&d, &r, "C");
        approx(a.total.unwrap(), 60.0);
        approx(a.coverage, 1.0);
        assert!(!a.is_incomplete());
        approx(b.total.unwrap(), 50.0);
        approx(b.coverage, 0.6);
        assert!(b.is_incomplete());
        assert_eq!(b.per_criterion[1].score, None);
        approx(b.per_criterion[1].contribution, 0.0);
        // C has no scores at all: no total, zero coverage, ranked last.
        assert_eq!(c.total, None);
        approx(c.coverage, 0.0);
        assert_eq!(r.ranked.last().unwrap().option_id, d.options[2].id);
    }

    #[test]
    fn zero_weight_criteria_are_ignored() {
        let mut d = decision(
            &[
                ("x", CriterionKind::Rated, 5),
                ("ignored", CriterionKind::Rated, 0),
            ],
            &["A"],
        );
        set(&mut d, 0, 0, 7.0);
        let r = results(&d);
        approx(r.ranked[0].total.unwrap(), 70.0);
        approx(r.ranked[0].coverage, 1.0);
    }

    #[test]
    fn all_weights_zero_is_an_error() {
        let mut d = decision(
            &[
                ("x", CriterionKind::Rated, 0),
                ("y", CriterionKind::Rated, 0),
            ],
            &["A"],
        );
        set(&mut d, 0, 0, 7.0);
        assert_eq!(
            compute(&d.criteria, &d.options, d.owner_scores()),
            Err(ScoreError::AllWeightsZero)
        );
    }

    #[test]
    fn no_criteria_is_an_error() {
        let d = decision(&[], &["A"]);
        assert_eq!(
            compute(&d.criteria, &d.options, d.owner_scores()),
            Err(ScoreError::NoCriteria)
        );
    }

    fn names(d: &Decision, r: &Results) -> Vec<String> {
        r.ranked
            .iter()
            .map(|x| {
                d.options
                    .iter()
                    .find(|o| o.id == x.option_id)
                    .unwrap()
                    .name
                    .clone()
            })
            .collect()
    }

    #[test]
    fn ties_break_on_coverage() {
        // A and B both total 50, but B is missing a score, so A (complete) ranks first.
        let mut d = decision(
            &[
                ("x", CriterionKind::Rated, 5),
                ("y", CriterionKind::Rated, 5),
            ],
            &["B", "A"],
        );
        set(&mut d, 1, 0, 5.0); // A: x=5, y=5 -> 50, coverage 1
        set(&mut d, 1, 1, 5.0);
        set(&mut d, 0, 0, 5.0); // B: x=5 only -> 50, coverage 0.5
        let r = results(&d);
        approx(r.ranked[0].total.unwrap(), r.ranked[1].total.unwrap());
        assert_eq!(names(&d, &r), ["A", "B"]);
    }

    #[test]
    fn ties_break_on_highest_weighted_criterion() {
        // "big" has the highest weight. A: 4*10 + 6*5 = 70. B: 4*2.5 + 6*10 = 70.
        // Equal totals and coverage, so B wins because it scores higher on "big".
        let mut d = decision(
            &[
                ("small", CriterionKind::Rated, 4),
                ("big", CriterionKind::Rated, 6),
            ],
            &["A", "B"],
        );
        set(&mut d, 0, 0, 10.0);
        set(&mut d, 0, 1, 5.0);
        set(&mut d, 1, 0, 2.5);
        set(&mut d, 1, 1, 10.0);
        let r = results(&d);
        approx(r.ranked[0].total.unwrap(), 70.0);
        approx(r.ranked[1].total.unwrap(), 70.0);
        assert_eq!(names(&d, &r), ["B", "A"]);
    }

    #[test]
    fn non_finite_values_are_treated_as_missing() {
        let mut d = decision(&[("x", CriterionKind::Rated, 1)], &["A"]);
        set(&mut d, 0, 0, f64::NAN);
        let r = results(&d);
        assert_eq!(r.ranked[0].total, None);
        assert_eq!(d.scores[OWNER].len(), 1);
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        /// Random decisions: 1-6 criteria of mixed kinds, 1-6 options, some scores missing.
        fn arb_decision() -> impl Strategy<Value = Decision> {
            let kind = prop_oneof![
                Just(CriterionKind::Rated),
                Just(numeric(Direction::HigherBetter, None)),
                Just(numeric(Direction::LowerBetter, None)),
                (-1000.0..1000.0f64, 1.0..500.0f64).prop_map(|(worst, d)| numeric(
                    Direction::HigherBetter,
                    Some(Bounds {
                        worst,
                        best: worst + d
                    })
                )),
            ];
            let criteria = prop::collection::vec((kind, 0u8..=10), 1..6);
            (criteria, 1usize..6).prop_flat_map(|(criteria, n_options)| {
                let n_criteria = criteria.len();
                let cells = prop::collection::vec(
                    prop::option::of(-2000.0..2000.0f64),
                    n_criteria * n_options,
                );
                (Just(criteria), Just(n_options), cells).prop_map(
                    move |(criteria, n_options, cells)| {
                        let mut d = Decision::new("v".into(), "h".into(), "t".into(), 0);
                        for (kind, w) in &criteria {
                            d.add_criterion("c".into(), kind.clone(), *w);
                        }
                        for _ in 0..n_options {
                            d.add_option("o".into(), String::new());
                        }
                        for (i, cell) in cells.iter().enumerate() {
                            if let Some(x) = cell {
                                let (o, c) = (i / n_criteria, i % n_criteria);
                                // Ratings are validated to 0-10 at input; mirror that here.
                                let x = match d.criteria[c].kind {
                                    CriterionKind::Rated => (x.abs() % 10.0).round(),
                                    _ => *x,
                                };
                                set(&mut d, o, c, x);
                            }
                        }
                        d
                    },
                )
            })
        }

        proptest! {
            #[test]
            fn totals_are_between_0_and_100(d in arb_decision()) {
                if let Ok(r) = compute(&d.criteria, &d.options, d.owner_scores()) {
                    for o in &r.ranked {
                        if let Some(t) = o.total {
                            prop_assert!((0.0..=100.0 + 1e-9).contains(&t), "total {t}");
                        }
                        prop_assert!((0.0..=1.0 + 1e-12).contains(&o.coverage));
                        let bar: f64 = o.per_criterion.iter().map(|c| c.contribution).sum();
                        prop_assert!((0.0..=100.0 + 1e-9).contains(&bar), "bar {bar}");
                    }
                }
            }

            #[test]
            fn scaling_weights_changes_nothing(d in arb_decision(), k in 2u8..=5) {
                // Keep scaled weights within u8 by using the scaled copy's weights directly; the
                // 0-10 limit is a UI rule, not something the maths depends on.
                let mut scaled = d.clone();
                for c in &mut scaled.criteria {
                    c.weight = c.weight.saturating_mul(k);
                }
                let a = compute(&d.criteria, &d.options, d.owner_scores());
                let b = compute(&scaled.criteria, &scaled.options, scaled.owner_scores());
                match (a, b) {
                    (Ok(a), Ok(b)) => {
                        // Compare per option rather than by rank: exact ties may legitimately
                        // reorder by a rounding error.
                        for x in &a.ranked {
                            let y = b.ranked.iter().find(|y| y.option_id == x.option_id).unwrap();
                            match (x.total, y.total) {
                                (Some(p), Some(q)) => prop_assert!((p - q).abs() < 1e-9),
                                (p, q) => prop_assert_eq!(p, q),
                            }
                            prop_assert!((x.coverage - y.coverage).abs() < 1e-12);
                        }
                    }
                    (a, b) => prop_assert_eq!(a.is_err(), b.is_err()),
                }
            }
        }
    }
}
