//! Static assets compiled into the binary.
//!
//! Files are embedded with `include_bytes!`, so the Lambda zip is a single executable. Each file is
//! served under a name that changes whenever its content changes (htmx carries its version; our
//! CSS carries a hash of its bytes), which makes the one-day cache on `/static/*` safe.

use std::sync::LazyLock;

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// Vendored htmx. Version 2.0.11 from the npm package `htmx.org` (dist/htmx.min.js).
/// SHA-256: d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717
/// To upgrade: replace the file, update the name and hash here and in README.md.
const HTMX_NAME: &str = "htmx-2.0.11.min.js";
const HTMX: &[u8] = include_bytes!("../static/htmx-2.0.11.min.js");

const CSS: &[u8] = include_bytes!("../static/app.css");

/// The public file names, computed once.
pub struct Assets {
    pub css: String,
    pub htmx: &'static str,
}

pub static ASSETS: LazyLock<Assets> = LazyLock::new(|| {
    let hash = hex_prefix(&Sha256::digest(CSS), 10);
    Assets {
        css: format!("app.{hash}.css"),
        htmx: HTMX_NAME,
    }
});

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .chars()
        .take(len)
        .collect()
}

/// `GET /static/{name}`
pub async fn serve(Path(name): Path<String>) -> Response {
    let assets = &*ASSETS;
    let (body, content_type) = if name == assets.css {
        (CSS, "text/css; charset=utf-8")
    } else if name == assets.htmx {
        (HTMX, "text/javascript; charset=utf-8")
    } else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}
