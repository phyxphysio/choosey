//! Cross-cutting request/response handling: origin protection, CSRF defence, security and cache
//! headers, and request logging that never records secrets.

use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::app::AppState;

/// Header CloudFront adds to every origin request (see `ORIGIN_VERIFY_SECRET`).
pub const ORIGIN_VERIFY_HEADER: &str = "x-origin-verify";

/// Content Security Policy: only our own scripts and styles, nothing inline, no framing.
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'";

/// Reject requests that didn't come through CloudFront.
///
/// The Lambda Function URL has auth type `NONE` (CloudFront's signed-origin feature would require
/// clients to hash POST bodies, which plain HTML forms can't do), so instead CloudFront adds a
/// secret header and we check it here. The comparison hashes both sides first so it is
/// constant-time regardless of length. No detail is given to the caller.
pub async fn verify_origin_secret(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.config.origin_verify_secret {
        let provided = req
            .headers()
            .get(ORIGIN_VERIFY_HEADER)
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        let ok: bool = Sha256::digest(provided)
            .ct_eq(&Sha256::digest(expected.as_bytes()))
            .into();
        if !ok {
            return (StatusCode::FORBIDDEN, "Forbidden").into_response();
        }
    }
    next.run(req).await
}

/// Defence against cross-site request forgery.
///
/// Choosey has no cookies or other ambient credentials: the edit secret lives in the URL, so a
/// forged cross-site request can't use the victim's access unless the attacker already knows the
/// secret, in which case they could just make the request themselves. Classic CSRF therefore
/// doesn't apply. As defence in depth we still (a) only change state on POST (the router has no
/// other mutating methods) and (b) reject POSTs whose `Origin` header is present and isn't our own
/// public origin. A missing `Origin` (old browsers, curl) is allowed.
pub async fn check_post_origin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if req.method() == Method::POST
        && let Some(origin) = req.headers().get(header::ORIGIN)
        && !origin
            .to_str()
            .is_ok_and(|o| o.eq_ignore_ascii_case(&state.config.public_origin))
    {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(req).await
}

/// Add security and caching headers to every response.
pub async fn response_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let is_htmx = req.headers().contains_key("hx-request");
    let mut res = next.run(req).await;
    let status = res.status();
    let h = res.headers_mut();

    set(h, header::CONTENT_SECURITY_POLICY, CSP);
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    // No referrer anywhere, so secret edit links never leak to other sites.
    set(h, header::REFERRER_POLICY, "no-referrer");
    set(
        h,
        HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    );
    set(h, header::X_FRAME_OPTIONS, "DENY");
    if state.config.hsts {
        set(
            h,
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=63072000; includeSubDomains; preload",
        );
    }
    if path.starts_with("/d/") || path == "/d" {
        // Decisions are private by obscurity of their URL: keep them out of search engines.
        set(h, HeaderName::from_static("x-robots-tag"), "noindex");
    }
    // Static assets have content-hashed or versioned names, so they can be cached; everything
    // else is per-decision and must not be.
    let cache = if path.starts_with("/static/") && status.is_success() {
        "public, max-age=86400"
    } else {
        "no-store"
    };
    set(h, header::CACHE_CONTROL, cache);

    // htmx doesn't swap error responses into the element that asked for a fragment (it would
    // find nothing to select). Point it at the page's <main> instead, so validation and
    // "not found" messages are visible.
    if is_htmx && (status.is_client_error() || status.is_server_error()) {
        set(h, HeaderName::from_static("hx-retarget"), "#main");
        set(h, HeaderName::from_static("hx-reselect"), "#main");
        set(h, HeaderName::from_static("hx-reswap"), "outerHTML");
    }
    res
}

fn set(headers: &mut HeaderMap, name: HeaderName, value: &'static str) {
    headers.insert(name, HeaderValue::from_static(value));
}

/// Log one line per request: method, route template, status and latency.
///
/// We deliberately don't use tower-http's TraceLayer, which logs the full URI: our URIs contain
/// edit secrets. The route template comes from axum's `MatchedPath`, so it holds placeholders
/// like `{secret}` rather than values; we further rewrite it so the log reads `<redacted>`.
/// Unmatched paths are logged as `<unmatched>`, never as the raw path.
pub async fn log_request(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| redact_route(p.as_str()))
        .unwrap_or_else(|| "<unmatched>".to_string());
    let res = next.run(req).await;
    tracing::info!(
        %method,
        route,
        status = res.status().as_u16(),
        latency_ms = start.elapsed().as_millis() as u64,
        "request"
    );
    res
}

/// `/d/{view_id}/edit/{secret}/options` -> `/d/:id/edit/<redacted>/options`.
fn redact_route(template: &str) -> String {
    template
        .replace("{view_id}", ":id")
        .replace("{secret}", "<redacted>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_redacted() {
        assert_eq!(
            redact_route("/d/{view_id}/edit/{secret}/criteria/{cid}"),
            "/d/:id/edit/<redacted>/criteria/{cid}"
        );
    }
}
