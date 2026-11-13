//! The axum application: shared state, routes and middleware.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use choosey_store::Store;
use tower_http::limit::RequestBodyLimitLayer;

use crate::config::Config;
use crate::error::AppError;
use crate::{assets, handlers as h, middleware as mw};

/// Largest request body we accept. The biggest form (a full score grid of 50 options x 30
/// criteria) is well under this.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// State shared by every request. Cloning is cheap: it's two `Arc`s.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub config: Arc<Config>,
}

/// Build the router with all middleware.
pub fn router(state: AppState) -> Router {
    // Everything under one decision's edit link.
    let edit = Router::new()
        .route("/", get(h::criteria_page))
        .route("/criteria", get(h::criteria_page).post(h::add_criterion))
        .route("/criteria/{cid}", post(h::update_criterion))
        .route("/criteria/{cid}/delete", post(h::delete_criterion))
        .route("/criteria/{cid}/up", post(h::move_up))
        .route("/criteria/{cid}/down", post(h::move_down))
        .route("/weights", post(h::save_weights))
        .route("/rank", post(h::apply_rank))
        .route("/options", get(h::options_page).post(h::add_option))
        .route("/options/{oid}", post(h::update_option))
        .route("/options/{oid}/delete", post(h::delete_option))
        .route("/score", get(h::score_page))
        .route("/scores", post(h::save_scores))
        .route("/results", get(h::results_page))
        .route("/title", post(h::rename))
        .route("/delete", post(h::delete_decision));

    Router::new()
        .route("/", get(h::home))
        .route("/d", post(h::create))
        .route("/d/{view_id}", get(h::view))
        .route("/d/{view_id}/export.csv", get(h::export_csv))
        // `/d/{id}/edit/{secret}` itself (the "/" route above) plus the steps and actions.
        .nest("/d/{view_id}/edit/{secret}", edit)
        .route("/static/{name}", get(assets::serve))
        .fallback(|| async { AppError::NotFound })
        // Layers: the last one added runs first. Reading bottom-up: headers are added to every
        // response (even rejections), requests are logged, the CloudFront secret is checked, the
        // Origin is checked for POSTs, and bodies are size-limited.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(from_fn_with_state(state.clone(), mw::check_post_origin))
        .layer(from_fn_with_state(state.clone(), mw::verify_origin_secret))
        .layer(from_fn(mw::log_request))
        .layer(from_fn_with_state(state.clone(), mw::response_headers))
        .with_state(state)
}
