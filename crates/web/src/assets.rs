//! Static assets compiled into the binary.
//!
//! Files are embedded with `include_bytes!`, so the Lambda zip is a single executable. Each file is
//! served under a name that changes whenever its content changes (htmx carries its version; our
//! CSS and JS carry a hash of their bytes), which makes the one-day cache on `/static/*` safe.

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

/// Our own small script (copy buttons, select-on-focus). Progressive enhancement only.
const JS: &[u8] = include_bytes!("../static/app.js");

/// The public file names, computed once.
pub struct Assets {
    pub css: String,
    pub js: String,
    pub htmx: &'static str,
}

pub static ASSETS: LazyLock<Assets> = LazyLock::new(|| Assets {
    css: hashed_name("app", CSS, "css"),
    js: hashed_name("app", JS, "js"),
    htmx: HTMX_NAME,
});

/// `app.0123456789.css`: the stem, the first 10 hex digits of the content's SHA-256, the extension.
fn hashed_name(stem: &str, content: &[u8], ext: &str) -> String {
    format!("{stem}.{}.{ext}", hex_prefix(&Sha256::digest(content), 10))
}

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
    } else if name == assets.js {
        (JS, "text/javascript; charset=utf-8")
    } else if name == assets.htmx {
        (HTMX, "text/javascript; charset=utf-8")
    } else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}
