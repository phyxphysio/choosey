//! Errors that end a request with an error page.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use choosey_store::StoreError;

use crate::views::{ErrorPage, render};

#[derive(Debug)]
pub enum AppError {
    /// Unknown decision *or* wrong edit secret. Both look identical to the client, so the response
    /// can't be used to discover which decisions exist.
    NotFound,
    /// Optimistic-concurrency conflict: someone else saved in between.
    Conflict,
    /// Anything unexpected. Details go to the log, not to the client.
    Internal,
}

impl From<StoreError> for AppError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Conflict => AppError::Conflict,
            other => {
                // StoreError messages never contain ids or secrets.
                tracing::error!(error = %other, "store error");
                AppError::Internal
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, heading, message) = match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "Not found",
                "There's nothing here. Check the link is complete; it may also have been deleted.",
            ),
            AppError::Conflict => (
                StatusCode::CONFLICT,
                "Changed elsewhere",
                "This decision was changed in another tab or by someone else at the same moment. \
                 Go back, reload the page and try again.",
            ),
            AppError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                "Sorry, that didn't work. Please try again in a moment.",
            ),
        };
        render(&ErrorPage::new(heading, message), status)
    }
}
