//! Route handlers.
//!
//! Every state change is a POST that redirects (303 See Other) back to a GET page, the classic
//! Post/Redirect/Get pattern, so plain HTML forms work without JavaScript. With JavaScript, htmx
//! makes the same requests in the background and swaps in the relevant part of the page.

use axum::extract::{Form, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use choosey_core::ids::{encode_base62, hash_secret, is_valid_id, verify_secret};
use choosey_core::model::{Bounds, CriterionKind, Decision, Direction, Feedback};
use choosey_core::validate::{
    self, InvalidInput, MAX_CONTACT, MAX_CRITERIA, MAX_FEEDBACK, MAX_NAME, MAX_NOTES, MAX_OPTIONS,
    MAX_UNIT,
};
use choosey_core::{rank_weights, templates};
use rand::TryRng;
use rand::rngs::SysRng;
use serde::Deserialize;

use crate::app::AppState;
use crate::error::AppError;
use crate::export;
use crate::views::{EditContext, EditPage, FeedbackPage, HomePage, Step, ViewPage, render};

type Result<T, E = AppError> = std::result::Result<T, E>;

/// Seconds since the Unix epoch.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// 128 bits from the operating system's CSPRNG, as a 22-character base62 string.
fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    SysRng.try_fill_bytes(&mut bytes).map_err(|e| {
        tracing::error!(error = %e, "OS random number generator failed");
        AppError::Internal
    })?;
    Ok(encode_base62(u128::from_le_bytes(bytes)))
}

fn edit_path(view_id: &str, secret: &str, step: Step) -> String {
    format!("/d/{view_id}/edit/{secret}{}", step.suffix())
}

/// Load a decision for the read-only view. Malformed ids are rejected before the store is asked.
async fn load_view(state: &AppState, view_id: &str) -> Result<Decision> {
    if !is_valid_id(view_id) {
        return Err(AppError::NotFound);
    }
    state.store.get(view_id).await?.ok_or(AppError::NotFound)
}

/// Load a decision for editing, checking the edit secret. A wrong secret gives exactly the same
/// result as an unknown decision.
async fn load_edit(state: &AppState, view_id: &str, secret: &str) -> Result<Decision> {
    if !is_valid_id(secret) {
        return Err(AppError::NotFound);
    }
    let d = load_view(state, view_id).await?;
    if verify_secret(secret, &d.edit_secret_hash) {
        Ok(d)
    } else {
        Err(AppError::NotFound)
    }
}

/// Query parameters accepted by the edit pages. Unknown parameters are ignored.
#[derive(Debug, Default, Deserialize)]
pub struct EditQuery {
    new: Option<String>,
    mode: Option<String>,
    option: Option<String>,
    /// Id of the criterion or option just added: show "Added …" and put the cursor back in the
    /// Add form's Name box, ready for the next one. An id rather than the name, so user text never
    /// goes into a URL; an unknown id is simply ignored.
    added: Option<String>,
    /// Ranks typed into the "Rank instead" form, one per criterion in criterion order, when the
    /// user pressed "Update suggestions". Not read by serde: the `r.{id}` field names are dynamic,
    /// so [`criteria_page`] parses them separately.
    #[serde(skip)]
    ranks: Option<Vec<u32>>,
}

fn edit_page(
    state: &AppState,
    d: &Decision,
    secret: &str,
    step: Step,
    q: &EditQuery,
    error: Option<String>,
) -> Response {
    let page = EditPage::build(
        d,
        EditContext {
            secret,
            public_base_url: &state.config.public_base_url,
            step,
            is_new: q.new.is_some() && step == Step::Criteria,
            rank_mode: q.mode.as_deref() == Some("rank"),
            ranks: q.ranks.clone(),
            option: q.option.as_deref(),
            added: q.added.as_deref(),
            error: error.clone(),
        },
    );
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    render(&page, status)
}

/// Load, change, save and redirect: the shape of every edit.
///
/// `change` validates its input and edits the decision in memory. If it returns an error, nothing
/// is saved and the step page is shown again with the message (status 422). Otherwise the
/// decision is written back with a conditional put, so two simultaneous edits can't silently
/// overwrite each other: the loser gets a "changed elsewhere" page.
///
/// On success the browser is sent back to `step` with `redirect_query` (empty, or starting `?`).
async fn mutate(
    state: &AppState,
    ids: (&str, &str),
    step: Step,
    redirect_query: &str,
    change: impl FnOnce(&mut Decision) -> Result<(), InvalidInput>,
) -> Result<Response> {
    let on_error = EditQuery {
        mode: redirect_query.contains("mode=rank").then(|| "rank".into()),
        ..Default::default()
    };
    mutate_with(state, ids, step, on_error, |d| {
        change(d)?;
        Ok((step, redirect_query.to_string()))
    })
    .await
}

/// Like [`mutate`], but `change` also decides where to go on success, as a step and a query
/// string, so it can use what it just did (the id of a new item, or an option id it checked
/// against the decision). Errors still re-show `step`, the page the form was on, with `on_error`
/// as its query, so the user can fix their input.
async fn mutate_with(
    state: &AppState,
    (view_id, secret): (&str, &str),
    step: Step,
    on_error: EditQuery,
    change: impl FnOnce(&mut Decision) -> Result<(Step, String), InvalidInput>,
) -> Result<Response> {
    let mut d = load_edit(state, view_id, secret).await?;
    let original = d.clone();
    let (redirect_step, redirect_query) = match change(&mut d) {
        Ok(to) => to,
        Err(e) => {
            return Ok(edit_page(
                state,
                &original,
                secret,
                step,
                &on_error,
                Some(e.0),
            ));
        }
    };
    let expected = d.bump_version(now());
    state.store.put(&d, expected).await?;
    let to = format!(
        "{}{redirect_query}",
        edit_path(view_id, secret, redirect_step)
    );
    Ok(Redirect::to(&to).into_response())
}

/// Where "Add" goes: back to the same step with `?added={id}` (see [`EditQuery::added`]), or on
/// to `next` when the "Next" button sent the form.
fn after_add(next: bool, step: Step, next_step: Step, id: &str) -> (Step, String) {
    if next {
        (next_step, String::new())
    } else {
        (step, format!("?added={id}"))
    }
}

/// Value of the `then` field sent by the page's "Next" button.
///
/// On the Criteria and Options pages, "Next" is a submit button for the "Add" form (linked with
/// the HTML `form` attribute) rather than a plain link. That way, a name typed into the Add box
/// but not yet added isn't silently thrown away when the user moves on: it's added first. If
/// the Add box is empty, "Next" just goes to the next step.
const THEN_NEXT: &str = "next";

/// Redirect to `step` after checking the edit secret, like every other edit route does.
async fn go_to(state: &AppState, (view_id, secret): (&str, &str), step: Step) -> Result<Response> {
    load_edit(state, view_id, secret).await?;
    Ok(Redirect::to(&edit_path(view_id, secret, step)).into_response())
}

fn invalid(msg: &str) -> InvalidInput {
    InvalidInput(msg.to_string())
}

// ---------------------------------------------------------------------------------------------
// Home and creation

#[derive(Debug, Deserialize)]
pub struct HomeQuery {
    deleted: Option<String>,
}

/// `GET /`
pub async fn home(Query(q): Query<HomeQuery>) -> Response {
    render(&HomePage::new(q.deleted.is_some(), None), StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    #[serde(default)]
    title: String,
    #[serde(default)]
    template: String,
}

/// `POST /d`: create a decision and go to its edit page.
pub async fn create(State(state): State<AppState>, Form(f): Form<CreateForm>) -> Result<Response> {
    let Some(template) = templates::find(&f.template) else {
        let page = HomePage::new(false, Some("Pick one of the starting points.".into()));
        return Ok(render(&page, StatusCode::UNPROCESSABLE_ENTITY));
    };
    let title = if f.title.trim().is_empty() {
        if template.id == "blank" {
            "My decision".to_string()
        } else {
            template.title.to_string()
        }
    } else {
        match validate::clean_line("Title", &f.title, MAX_NAME) {
            Ok(t) => t,
            Err(e) => {
                return Ok(render(
                    &HomePage::new(false, Some(e.0)),
                    StatusCode::UNPROCESSABLE_ENTITY,
                ));
            }
        }
    };

    let view_id = random_id()?;
    let secret = random_id()?;
    let mut d = Decision::new(view_id.clone(), hash_secret(&secret), title, now());
    template.apply(&mut d);
    state.store.create(&d).await?;
    tracing::info!(
        view = &view_id[..4],
        template = template.id,
        "decision created"
    );

    let to = format!("{}?new=1", edit_path(&view_id, &secret, Step::Criteria));
    Ok(Redirect::to(&to).into_response())
}

// ---------------------------------------------------------------------------------------------
// Read-only view and export

/// `GET /d/{view_id}`
pub async fn view(State(state): State<AppState>, Path(view_id): Path<String>) -> Result<Response> {
    let d = load_view(&state, &view_id).await?;
    Ok(render(&ViewPage::build(&d), StatusCode::OK))
}

/// `GET /d/{view_id}/export.csv`
pub async fn export_csv(
    State(state): State<AppState>,
    Path(view_id): Path<String>,
) -> Result<Response> {
    let d = load_view(&state, &view_id).await?;
    let body = export::to_csv(&d).map_err(|e| {
        tracing::error!(error = %e, "csv export failed");
        AppError::Internal
    })?;
    // A fixed file name: user text never goes into a header.
    let disposition = format!("attachment; filename=\"{}.csv\"", choosey_core::APP_NAME);
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    )
        .into_response())
}

// ---------------------------------------------------------------------------------------------
// Edit pages (GET)

type EditPath = Path<(String, String)>;

async fn show(
    state: AppState,
    (view_id, secret): (String, String),
    step: Step,
    q: EditQuery,
) -> Result<Response> {
    let d = load_edit(&state, &view_id, &secret).await?;
    Ok(edit_page(&state, &d, &secret, step, &q, None))
}

/// `GET /d/{view_id}/edit/{secret}` (also `/criteria`)
///
/// In rank mode, the "Update suggestions" button submits the rank form here as a GET, with
/// `r.{criterion_id}` fields, so the page can preview the weights those ranks (including ties)
/// would give before anything is saved.
pub async fn criteria_page(
    State(s): State<AppState>,
    Path((view_id, secret)): EditPath,
    Query(mut q): Query<EditQuery>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response> {
    let d = load_edit(&s, &view_id, &secret).await?;
    if q.mode.as_deref() == Some("rank") && pairs.iter().any(|(k, _)| k.starts_with("r.")) {
        // Only a preview, so bad numbers are ignored rather than reported: the page then
        // shows the suggestion for the current order.
        q.ranks = parse_ranks(&d, &pairs).ok();
    }
    Ok(edit_page(&s, &d, &secret, Step::Criteria, &q, None))
}

/// `GET /d/{view_id}/edit/{secret}/options`
pub async fn options_page(
    State(s): State<AppState>,
    Path(p): EditPath,
    Query(q): Query<EditQuery>,
) -> Result<Response> {
    show(s, p, Step::Options, q).await
}

/// `GET /d/{view_id}/edit/{secret}/score`
pub async fn score_page(
    State(s): State<AppState>,
    Path(p): EditPath,
    Query(q): Query<EditQuery>,
) -> Result<Response> {
    show(s, p, Step::Score, q).await
}

/// `GET /d/{view_id}/edit/{secret}/results`
pub async fn results_page(
    State(s): State<AppState>,
    Path(p): EditPath,
    Query(q): Query<EditQuery>,
) -> Result<Response> {
    show(s, p, Step::Results, q).await
}

// ---------------------------------------------------------------------------------------------
// Decision title and deletion

#[derive(Debug, Deserialize)]
pub struct TitleForm {
    #[serde(default)]
    title: String,
}

/// `POST .../title`
pub async fn rename(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(f): Form<TitleForm>,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "", |d| {
        d.title = validate::clean_line("Decision name", &f.title, MAX_NAME)?;
        Ok(())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    #[serde(default)]
    confirm: String,
}

/// `POST .../delete`: requires the confirmation checkbox.
pub async fn delete_decision(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(f): Form<DeleteForm>,
) -> Result<Response> {
    let d = load_edit(&s, &v, &k).await?;
    if f.confirm != "yes" {
        let msg = "Tick the box to confirm you want to delete this decision.".to_string();
        return Ok(edit_page(
            &s,
            &d,
            &k,
            Step::Results,
            &EditQuery::default(),
            Some(msg),
        ));
    }
    s.store.delete(&v).await?;
    tracing::info!(view = &v[..4], "decision deleted");
    Ok(Redirect::to("/?deleted=1").into_response())
}

// ---------------------------------------------------------------------------------------------
// Criteria

#[derive(Debug, Deserialize)]
pub struct CriterionForm {
    /// [`THEN_NEXT`] when sent by the "Next" button; empty for "Add".
    #[serde(default)]
    then: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    worst: String,
    #[serde(default)]
    best: String,
}

impl CriterionForm {
    fn parse_kind(&self) -> Result<CriterionKind, InvalidInput> {
        let direction = match self.kind.as_str() {
            "rated" => return Ok(CriterionKind::Rated),
            "higher" => Direction::HigherBetter,
            "lower" => Direction::LowerBetter,
            _ => return Err(invalid("Choose how this criterion is scored.")),
        };
        let unit = validate::clean_optional_line("Unit", &self.unit, MAX_UNIT)?;
        let worst = validate::parse_number("Worst", &self.worst)?;
        let best = validate::parse_number("Best", &self.best)?;
        let bounds = match (worst, best) {
            (None, None) => None,
            (Some(worst), Some(best)) => {
                if worst == best {
                    return Err(invalid("Worst and best must be different."));
                }
                match direction {
                    Direction::HigherBetter if best < worst => {
                        return Err(invalid(
                            "For higher-is-better, the best value should be the larger number.",
                        ));
                    }
                    Direction::LowerBetter if best > worst => {
                        return Err(invalid(
                            "For lower-is-better, the best value should be the smaller number.",
                        ));
                    }
                    _ => Some(Bounds { worst, best }),
                }
            }
            _ => return Err(invalid("Set both worst and best, or leave both empty.")),
        };
        Ok(CriterionKind::Numeric {
            direction,
            unit,
            bounds,
        })
    }
}

/// `POST .../criteria`: add a criterion (weight starts at 5).
pub async fn add_criterion(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(f): Form<CriterionForm>,
) -> Result<Response> {
    let next = f.then == THEN_NEXT;
    let typed_nothing = [&f.name, &f.unit, &f.worst, &f.best]
        .iter()
        .all(|field| field.trim().is_empty());
    if next && typed_nothing {
        return go_to(&s, (&v, &k), Step::Options).await;
    }
    mutate_with(&s, (&v, &k), Step::Criteria, EditQuery::default(), |d| {
        if d.criteria.len() >= MAX_CRITERIA {
            return Err(invalid("You've reached the maximum number of criteria."));
        }
        let name = validate::clean_line("Name", &f.name, MAX_NAME)?;
        let kind = f.parse_kind()?;
        let id = d.add_criterion(name, kind, 5);
        Ok(after_add(next, Step::Criteria, Step::Options, &id))
    })
    .await
}

type ItemPath = Path<(String, String, String)>;

/// `POST .../criteria/{cid}`: rename or change the kind. Switching between rated and numeric
/// clears that criterion's scores, since the old values mean something else.
pub async fn update_criterion(
    State(s): State<AppState>,
    Path((v, k, cid)): ItemPath,
    Form(f): Form<CriterionForm>,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "", |d| {
        let name = validate::clean_line("Name", &f.name, MAX_NAME)?;
        let kind = f.parse_kind()?;
        let c = d
            .criterion_mut(&cid)
            .ok_or_else(|| invalid("That criterion no longer exists."))?;
        let was_rated = matches!(c.kind, CriterionKind::Rated);
        let now_rated = matches!(kind, CriterionKind::Rated);
        c.name = name;
        c.kind = kind;
        if was_rated != now_rated {
            d.clear_criterion_scores(&cid);
        }
        Ok(())
    })
    .await
}

/// `POST .../criteria/{cid}/delete`
pub async fn delete_criterion(
    State(s): State<AppState>,
    Path((v, k, cid)): ItemPath,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "", |d| {
        d.remove_criterion(&cid);
        Ok(())
    })
    .await
}

/// `POST .../criteria/{cid}/up`
pub async fn move_up(State(s): State<AppState>, Path((v, k, cid)): ItemPath) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "?mode=rank", |d| {
        d.move_criterion(&cid, true);
        Ok(())
    })
    .await
}

/// `POST .../criteria/{cid}/down`
pub async fn move_down(State(s): State<AppState>, Path((v, k, cid)): ItemPath) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "?mode=rank", |d| {
        d.move_criterion(&cid, false);
        Ok(())
    })
    .await
}

/// Form fields as raw pairs, for forms with one field per criterion/option.
type Pairs = Form<Vec<(String, String)>>;

/// `POST .../weights`: fields `w.{criterion_id}` = 0..10.
pub async fn save_weights(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(pairs): Pairs,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "", |d| {
        for (key, value) in &pairs {
            if let Some(cid) = key.strip_prefix("w.") {
                let Some(c) = d.criterion_mut(cid) else {
                    continue;
                };
                c.weight = validate::parse_weight(&c.name.clone(), value)?;
            }
        }
        Ok(())
    })
    .await
}

/// Read `r.{criterion_id}` = rank fields into one rank per criterion, in criterion order
/// (1 = most important, equal numbers tie). A criterion missing from the fields keeps its current
/// position as its rank; fields for unknown criteria are ignored.
fn parse_ranks(d: &Decision, pairs: &[(String, String)]) -> Result<Vec<u32>, InvalidInput> {
    let mut ranks: Vec<u32> = (1..=d.criteria.len() as u32).collect();
    for (key, value) in pairs {
        if let Some(cid) = key.strip_prefix("r.")
            && let Some(i) = d.criteria.iter().position(|c| c.id == cid)
        {
            ranks[i] = value
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|r| (1..=1000).contains(r))
                .ok_or_else(|| invalid("Ranks must be whole numbers, 1 for the most important."))?;
        }
    }
    Ok(ranks)
}

/// `POST .../rank`: fields `r.{criterion_id}` = rank (1 = most important, equal numbers tie).
/// Sets every weight from the rank-sum suggestions (see `choosey_core::rank_weights`).
pub async fn apply_rank(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(pairs): Pairs,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Criteria, "", |d| {
        let ranks = parse_ranks(d, &pairs)?;
        for (c, w) in d
            .criteria
            .iter_mut()
            .zip(rank_weights::suggested_weights(&ranks))
        {
            c.weight = w;
        }
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Options

#[derive(Debug, Deserialize)]
pub struct OptionForm {
    /// [`THEN_NEXT`] when sent by the "Next" button; empty for "Add".
    #[serde(default)]
    then: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    notes: String,
}

/// `POST .../options`
pub async fn add_option(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(f): Form<OptionForm>,
) -> Result<Response> {
    let next = f.then == THEN_NEXT;
    if next && f.name.trim().is_empty() && f.notes.trim().is_empty() {
        return go_to(&s, (&v, &k), Step::Score).await;
    }
    mutate_with(&s, (&v, &k), Step::Options, EditQuery::default(), |d| {
        if d.options.len() >= MAX_OPTIONS {
            return Err(invalid("You've reached the maximum number of options."));
        }
        let name = validate::clean_line("Name", &f.name, MAX_NAME)?;
        let notes = validate::clean_text("Notes", &f.notes, MAX_NOTES)?;
        let id = d.add_option(name, notes);
        Ok(after_add(next, Step::Options, Step::Score, &id))
    })
    .await
}

/// `POST .../options/{oid}`
pub async fn update_option(
    State(s): State<AppState>,
    Path((v, k, oid)): ItemPath,
    Form(f): Form<OptionForm>,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Options, "", |d| {
        let name = validate::clean_line("Name", &f.name, MAX_NAME)?;
        let notes = validate::clean_text("Notes", &f.notes, MAX_NOTES)?;
        let o = d
            .option_mut(&oid)
            .ok_or_else(|| invalid("That option no longer exists."))?;
        o.name = name;
        o.notes = notes;
        Ok(())
    })
    .await
}

/// `POST .../options/{oid}/delete`
pub async fn delete_option(
    State(s): State<AppState>,
    Path((v, k, oid)): ItemPath,
) -> Result<Response> {
    mutate(&s, (&v, &k), Step::Options, "", |d| {
        d.remove_option(&oid);
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Scores

/// `POST .../scores`: fields `s.{option_id}.{criterion_id}` = value (empty clears it), plus
/// `option` = the option shown on small screens, so the redirect returns to it.
///
/// On small screens the Score step shows one option at a time. Its "← Previous" and "Next →"
/// buttons submit this same form with `go` = the option to show next, so scores typed without
/// JavaScript are saved before moving (they used to be plain links, which dropped them). `go` is
/// checked against the decision's options; an unknown id falls back to the first option.
pub async fn save_scores(
    State(s): State<AppState>,
    Path((v, k)): EditPath,
    Form(pairs): Pairs,
) -> Result<Response> {
    let field = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
    };
    // Only echo back an option id that looks like one of ours (`o` + digits).
    let option = field("option")
        .filter(|v| {
            v.len() <= 8 && v.starts_with('o') && v[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .map(str::to_string);
    let go = field("go").map(str::to_string);
    // If a score is invalid, show the error on the option the user was looking at.
    let on_error = EditQuery {
        option: option.clone(),
        ..Default::default()
    };

    mutate_with(&s, (&v, &k), Step::Score, on_error, |d| {
        for (key, value) in &pairs {
            let Some((oid, cid)) = key.strip_prefix("s.").and_then(|rest| rest.split_once('.'))
            else {
                continue;
            };
            // Ignore cells for things deleted since the form was rendered.
            let Some(c) = d.criterion(cid) else { continue };
            if !d.options.iter().any(|o| o.id == oid) {
                continue;
            }
            let label = c.name.clone();
            let parsed = match c.kind {
                CriterionKind::Rated => validate::parse_rating(&label, value)?,
                CriterionKind::Numeric { .. } => validate::parse_number(&label, value)?,
            };
            let cells = d.owner_scores_mut().entry(oid.to_string()).or_default();
            match parsed {
                Some(x) => {
                    cells.insert(cid.to_string(), x);
                }
                None => {
                    cells.remove(cid);
                }
            }
        }
        // Previous/Next: the requested option if it's one of ours, else the first. Otherwise
        // ("Save scores", or htmx's auto-save) stay on the option that was shown.
        let show = match go {
            Some(go) => d
                .options
                .iter()
                .find(|o| o.id == go)
                .or(d.options.first())
                .map(|o| o.id.clone()),
            None => option,
        };
        let query = show.map(|o| format!("?option={o}")).unwrap_or_default();
        Ok((Step::Score, query))
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Feedback

#[derive(Debug, Deserialize)]
pub struct FeedbackQuery {
    sent: Option<String>,
}

/// `GET /feedback`
pub async fn feedback_page(Query(q): Query<FeedbackQuery>) -> Response {
    let page = FeedbackPage::new(q.sent.is_some(), None, String::new(), String::new());
    render(&page, StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct FeedbackForm {
    #[serde(default)]
    message: String,
    #[serde(default)]
    contact: String,
    /// Honeypot: hidden from people (see `.hp` in app.css) but bots tend to fill in every field.
    #[serde(default)]
    website: String,
}

/// `POST /feedback`: save the message, then tell the owner (best effort).
///
/// The message is deliberately not linked to any decision, and the page sends no referrer, so
/// feedback can never carry someone's edit link.
pub async fn send_feedback(
    State(s): State<AppState>,
    Form(f): Form<FeedbackForm>,
) -> Result<Response> {
    let thanks = || Redirect::to("/feedback?sent=1").into_response();
    if !f.website.is_empty() {
        // Looks like a bot. Pretend it worked so it doesn't try another way.
        tracing::info!("feedback honeypot triggered; dropped");
        return Ok(thanks());
    }

    let cleaned =
        validate::clean_text("Your feedback", &f.message, MAX_FEEDBACK).and_then(|message| {
            if message.is_empty() {
                return Err(invalid("Please write some feedback before sending."));
            }
            let contact = validate::clean_optional_line("Contact", &f.contact, MAX_CONTACT)?;
            Ok((message, contact))
        });
    let (message, contact) = match cleaned {
        Ok(v) => v,
        Err(e) => {
            // Show the form again with what they typed, so nothing is lost.
            let page = FeedbackPage::new(false, Some(e.0), f.message, f.contact);
            return Ok(render(&page, StatusCode::UNPROCESSABLE_ENTITY));
        }
    };

    let feedback = Feedback {
        id: random_id()?,
        created_at: now(),
        message,
        contact,
    };
    s.store.add_feedback(&feedback).await?;
    // Never log the message itself: it's the user's words and could contain anything.
    tracing::info!(id_prefix = &feedback.id[..4], "feedback saved");
    s.notifier.feedback(&feedback).await;
    Ok(thanks())
}
