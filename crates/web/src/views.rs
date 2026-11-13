//! Page templates (askama) and the view models that feed them.
//!
//! Templates live in `crates/web/templates`. Askama compiles them into Rust at build time and
//! HTML-escapes every `{{ value }}` by default; we never use the `|safe` filter on user data.
//! View models hold display-ready strings so the templates stay simple.

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use choosey_core::model::{CriterionKind, Decision, Direction};
use choosey_core::scoring::{self, ScoreError};
use choosey_core::templates::TEMPLATES;
use choosey_core::validate::{MAX_CRITERIA, MAX_NAME, MAX_NOTES, MAX_OPTIONS, MAX_UNIT};

use crate::assets::{ASSETS, Assets};

/// Render a template to a response, or a bare 500 if rendering fails (which would be a bug).
pub fn render(template: &impl Template, status: StatusCode) -> Response {
    match template.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// "Choosey", from the configured app name.
pub fn display_name() -> String {
    let mut chars = choosey_core::APP_NAME.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Input limits shown in the forms (and enforced on the server).
pub struct Limits {
    pub name: usize,
    pub notes: usize,
    pub unit: usize,
    pub criteria: usize,
    pub options: usize,
}

const LIMITS: Limits = Limits {
    name: MAX_NAME,
    notes: MAX_NOTES,
    unit: MAX_UNIT,
    criteria: MAX_CRITERIA,
    options: MAX_OPTIONS,
};

// ---------------------------------------------------------------------------------------------
// Home and error pages

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePage {
    pub app: String,
    pub assets: &'static Assets,
    pub templates: Vec<TemplateCard>,
    pub deleted: bool,
    pub error: Option<String>,
    pub limits: &'static Limits,
}

pub struct TemplateCard {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub count: usize,
}

impl HomePage {
    pub fn new(deleted: bool, error: Option<String>) -> Self {
        Self {
            app: display_name(),
            assets: &ASSETS,
            templates: TEMPLATES
                .iter()
                .map(|t| TemplateCard {
                    id: t.id,
                    title: t.title,
                    description: t.description,
                    count: t.criteria.len(),
                })
                .collect(),
            deleted,
            error,
            limits: &LIMITS,
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    pub app: String,
    pub assets: &'static Assets,
    pub heading: &'static str,
    pub message: &'static str,
}

impl ErrorPage {
    pub fn new(heading: &'static str, message: &'static str) -> Self {
        Self {
            app: display_name(),
            assets: &ASSETS,
            heading,
            message,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Results (shared by the edit page and the read-only view)

pub struct ResultsView {
    /// Shown instead of the ranking when results can't be computed.
    pub message: Option<String>,
    pub rows: Vec<ResultRow>,
    pub legend: Vec<LegendItem>,
    pub any_incomplete: bool,
}

pub struct ResultRow {
    pub rank: usize,
    pub name: String,
    pub total: String,
    pub incomplete: bool,
    pub coverage: String,
    pub segments: Vec<Segment>,
    /// Text alternative for the bar.
    pub summary: String,
}

/// One coloured piece of a breakdown bar.
///
/// The bar is an inline SVG whose `<rect>` elements get their position and size from the `x` and
/// `width` *attributes* (SVG geometry, in a 0-100 viewBox), not from `style="..."`. That keeps the
/// page compatible with our strict Content Security Policy (`style-src 'self'`, no inline styles)
/// while still showing exact proportions with no JavaScript.
pub struct Segment {
    pub x: String,
    pub width: String,
    pub class: String,
    pub label: String,
}

pub struct LegendItem {
    pub class: String,
    pub name: String,
}

/// Number of distinct segment colours defined in app.css (`.seg-0` .. `.seg-7`).
const PALETTE: usize = 8;

fn seg_class(index: usize) -> String {
    format!("seg-{}", index % PALETTE)
}

/// Format a number for display: one decimal place, without a trailing ".0".
pub fn fmt1(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

impl ResultsView {
    pub fn build(d: &Decision) -> Self {
        let empty = |message: &str| Self {
            message: Some(message.to_string()),
            rows: Vec::new(),
            legend: Vec::new(),
            any_incomplete: false,
        };
        if d.options.is_empty() {
            return empty("Add some options to see results.");
        }
        let results = match scoring::compute(&d.criteria, &d.options, d.owner_scores()) {
            Ok(r) => r,
            Err(ScoreError::NoCriteria) => return empty("Add some criteria to see results."),
            Err(ScoreError::AllWeightsZero) => {
                return empty(
                    "Every criterion has weight 0, so nothing counts. Give at least one criterion a weight.",
                );
            }
        };

        let legend = d
            .criteria
            .iter()
            .enumerate()
            .filter(|(_, c)| c.weight > 0)
            .map(|(i, c)| LegendItem {
                class: seg_class(i),
                name: c.name.clone(),
            })
            .collect();

        let rows: Vec<ResultRow> = results
            .ranked
            .iter()
            .enumerate()
            .map(|(rank, r)| {
                let name = d
                    .options
                    .iter()
                    .find(|o| o.id == r.option_id)
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                let mut x = 0.0;
                let mut segments = Vec::new();
                let mut parts = Vec::new();
                for (i, (c, cr)) in d.criteria.iter().zip(&r.per_criterion).enumerate() {
                    if cr.contribution > 0.0 {
                        let label = format!("{}: {} points", c.name, fmt1(cr.contribution));
                        segments.push(Segment {
                            x: format!("{x:.3}"),
                            width: format!("{:.3}", cr.contribution),
                            class: seg_class(i),
                            label: label.clone(),
                        });
                        parts.push(label);
                        x += cr.contribution;
                    }
                }
                ResultRow {
                    rank: rank + 1,
                    total: r.total.map(fmt1).unwrap_or_else(|| "–".to_string()),
                    incomplete: r.is_incomplete(),
                    coverage: format!("{:.0}%", r.coverage * 100.0),
                    summary: if parts.is_empty() {
                        format!("{name}: no points yet")
                    } else {
                        format!("{name}: {}", parts.join(", "))
                    },
                    name,
                    segments,
                }
            })
            .collect();

        Self {
            message: None,
            any_incomplete: rows.iter().any(|r| r.incomplete),
            rows,
            legend,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Edit page

/// The steps of the edit page, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Criteria,
    Options,
    Score,
    Results,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Criteria, Step::Options, Step::Score, Step::Results];

    /// Path suffix after `/d/{view_id}/edit/{secret}`.
    pub fn suffix(self) -> &'static str {
        match self {
            Step::Criteria => "",
            Step::Options => "/options",
            Step::Score => "/score",
            Step::Results => "/results",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Step::Criteria => "Criteria",
            Step::Options => "Options",
            Step::Score => "Score",
            Step::Results => "Results",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Step::Criteria => "criteria",
            Step::Options => "options",
            Step::Score => "score",
            Step::Results => "results",
        }
    }
}

pub struct StepLink {
    pub n: usize,
    pub label: &'static str,
    pub href: String,
    pub current: bool,
}

pub struct CriterionRow {
    pub id: String,
    pub position: usize,
    pub name: String,
    /// "rated", "higher" or "lower" (the value of the kind `<select>`).
    pub kind: String,
    pub kind_label: String,
    pub unit: String,
    pub worst: String,
    pub best: String,
    pub weight: u8,
    /// Numeric without bounds: show the min-max warning.
    pub minmax_hint: bool,
    pub is_first: bool,
    pub is_last: bool,
}

pub struct OptionRow {
    pub id: String,
    pub name: String,
    pub notes: String,
}

pub struct ScoreCard {
    pub option_name: String,
    pub current: bool,
    pub cells: Vec<ScoreCell>,
}

pub struct ScoreCell {
    /// Form field name, `s.{option}.{criterion}`.
    pub field: String,
    /// Element id, `s-{option}-{criterion}`.
    pub input_id: String,
    pub criterion_name: String,
    pub rated: bool,
    pub hint: String,
    pub value: String,
}

/// Previous/next links for the one-option-at-a-time score view on small screens.
pub struct ScoreNav {
    pub index: usize,
    pub count: usize,
    pub prev: Option<String>,
    pub next: Option<String>,
}

#[derive(Template)]
#[template(path = "edit.html")]
pub struct EditPage {
    pub app: String,
    pub assets: &'static Assets,
    pub limits: &'static Limits,
    pub title: String,
    /// `/d/{view_id}/edit/{secret}`
    pub base: String,
    pub step: String,
    pub steps: Vec<StepLink>,
    pub next_step: Option<StepLink>,
    pub error: Option<String>,
    pub is_new: bool,
    pub edit_url: String,
    pub view_url: String,
    pub export_href: String,
    pub rank_mode: bool,
    pub criteria: Vec<CriterionRow>,
    pub options: Vec<OptionRow>,
    pub can_add_criterion: bool,
    pub can_add_option: bool,
    pub score_cards: Vec<ScoreCard>,
    pub score_nav: Option<ScoreNav>,
    pub current_option: String,
    pub results: ResultsView,
}

/// Inputs to [`EditPage::build`] that come from the request rather than the decision.
pub struct EditContext<'a> {
    pub secret: &'a str,
    pub public_base_url: &'a str,
    pub step: Step,
    pub error: Option<String>,
    pub is_new: bool,
    pub rank_mode: bool,
    pub option: Option<&'a str>,
}

fn kind_fields(kind: &CriterionKind) -> (&'static str, String, String, String, String, bool) {
    match kind {
        CriterionKind::Rated => (
            "rated",
            "Rated 0-10".into(),
            String::new(),
            String::new(),
            String::new(),
            false,
        ),
        CriterionKind::Numeric {
            direction,
            unit,
            bounds,
        } => {
            let (key, label) = match direction {
                Direction::HigherBetter => ("higher", "Number, higher is better"),
                Direction::LowerBetter => ("lower", "Number, lower is better"),
            };
            let (worst, best) = bounds
                .map(|b| (b.worst.to_string(), b.best.to_string()))
                .unwrap_or_default();
            let label = match bounds {
                Some(b) => format!("{label} ({} worst, {} best)", b.worst, b.best),
                None => label.to_string(),
            };
            (key, label, unit.clone(), worst, best, bounds.is_none())
        }
    }
}

impl EditPage {
    pub fn build(d: &Decision, ctx: EditContext<'_>) -> Self {
        let base = format!("/d/{}/edit/{}", d.view_id, ctx.secret);
        let step_link = |i: usize, s: Step| StepLink {
            n: i + 1,
            label: s.label(),
            href: format!("{base}{}", s.suffix()),
            current: s == ctx.step,
        };
        let steps: Vec<StepLink> = Step::ALL
            .iter()
            .enumerate()
            .map(|(i, &s)| step_link(i, s))
            .collect();
        let next_step = Step::ALL
            .iter()
            .position(|&s| s == ctx.step)
            .and_then(|i| Step::ALL.get(i + 1).map(|&s| step_link(i + 1, s)));

        let n = d.criteria.len();
        let criteria = d
            .criteria
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (kind, kind_label, unit, worst, best, minmax_hint) = kind_fields(&c.kind);
                CriterionRow {
                    id: c.id.clone(),
                    position: i + 1,
                    name: c.name.clone(),
                    kind: kind.to_string(),
                    kind_label,
                    unit,
                    worst,
                    best,
                    weight: c.weight,
                    minmax_hint,
                    is_first: i == 0,
                    is_last: i + 1 == n,
                }
            })
            .collect();

        let options = d
            .options
            .iter()
            .map(|o| OptionRow {
                id: o.id.clone(),
                name: o.name.clone(),
                notes: o.notes.clone(),
            })
            .collect();

        // Which option is shown on small screens: the requested one if it exists, else the first.
        let current_index = ctx
            .option
            .and_then(|id| d.options.iter().position(|o| o.id == id))
            .unwrap_or(0);
        let current_option = d
            .options
            .get(current_index)
            .map(|o| o.id.clone())
            .unwrap_or_default();
        let score_href = |i: usize| format!("{base}/score?option={}", d.options[i].id);
        let score_nav = (!d.options.is_empty()).then(|| ScoreNav {
            index: current_index + 1,
            count: d.options.len(),
            prev: current_index.checked_sub(1).map(score_href),
            next: (current_index + 1 < d.options.len()).then(|| score_href(current_index + 1)),
        });

        let sheet = d.owner_scores();
        let score_cards = d
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| ScoreCard {
                option_name: o.name.clone(),
                current: i == current_index,
                cells: d
                    .criteria
                    .iter()
                    .map(|c| {
                        let (rated, hint) = match &c.kind {
                            CriterionKind::Rated => (true, "0 to 10".to_string()),
                            CriterionKind::Numeric {
                                direction, unit, ..
                            } => {
                                let dir = match direction {
                                    Direction::HigherBetter => "higher is better",
                                    Direction::LowerBetter => "lower is better",
                                };
                                let hint = if unit.is_empty() {
                                    dir.to_string()
                                } else {
                                    format!("{unit}, {dir}")
                                };
                                (false, hint)
                            }
                        };
                        let value = sheet
                            .get(&o.id)
                            .and_then(|m| m.get(&c.id))
                            .map(|x| x.to_string())
                            .unwrap_or_default();
                        ScoreCell {
                            field: format!("s.{}.{}", o.id, c.id),
                            input_id: format!("s-{}-{}", o.id, c.id),
                            criterion_name: c.name.clone(),
                            rated,
                            hint,
                            value,
                        }
                    })
                    .collect(),
            })
            .collect();

        Self {
            app: display_name(),
            assets: &ASSETS,
            limits: &LIMITS,
            title: d.title.clone(),
            edit_url: format!("{}{base}", ctx.public_base_url),
            view_url: format!("{}/d/{}", ctx.public_base_url, d.view_id),
            export_href: format!("/d/{}/export.csv", d.view_id),
            step: ctx.step.key().to_string(),
            steps,
            next_step,
            error: ctx.error,
            is_new: ctx.is_new,
            rank_mode: ctx.rank_mode,
            criteria,
            options,
            can_add_criterion: d.criteria.len() < MAX_CRITERIA,
            can_add_option: d.options.len() < MAX_OPTIONS,
            score_cards,
            score_nav,
            current_option,
            results: ResultsView::build(d),
            base,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Read-only view

#[derive(Template)]
#[template(path = "view.html")]
pub struct ViewPage {
    pub app: String,
    pub assets: &'static Assets,
    pub title: String,
    pub export_href: String,
    pub results: ResultsView,
    pub headers: Vec<String>,
    pub rows: Vec<TableRow>,
}

pub struct TableRow {
    pub name: String,
    pub notes: String,
    /// One per criterion: (raw value with unit, normalised 0-10 score), or empty strings.
    pub cells: Vec<(String, String)>,
}

impl ViewPage {
    pub fn build(d: &Decision) -> Self {
        let sheet = d.owner_scores();
        let normalised: Vec<_> = d
            .criteria
            .iter()
            .map(|c| scoring::normalise_criterion(c, &d.options, sheet))
            .collect();
        let headers = d
            .criteria
            .iter()
            .map(|c| match &c.kind {
                CriterionKind::Numeric { unit, .. } if !unit.is_empty() => {
                    format!("{} ({unit}), weight {}", c.name, c.weight)
                }
                _ => format!("{}, weight {}", c.name, c.weight),
            })
            .collect();
        let rows = d
            .options
            .iter()
            .map(|o| TableRow {
                name: o.name.clone(),
                notes: o.notes.clone(),
                cells: d
                    .criteria
                    .iter()
                    .zip(&normalised)
                    .map(|(c, norm)| {
                        let raw = sheet
                            .get(&o.id)
                            .and_then(|m| m.get(&c.id))
                            .map(|x| x.to_string())
                            .unwrap_or_default();
                        let score = norm.get(&o.id).map(|&s| fmt1(s)).unwrap_or_default();
                        (raw, score)
                    })
                    .collect(),
            })
            .collect();
        Self {
            app: display_name(),
            assets: &ASSETS,
            title: d.title.clone(),
            export_href: format!("/d/{}/export.csv", d.view_id),
            results: ResultsView::build(d),
            headers,
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt1(72.0), "72");
        assert_eq!(fmt1(72.46), "72.5");
        assert_eq!(fmt1(0.04), "0");
        assert_eq!(display_name(), "Choosey");
    }
}
