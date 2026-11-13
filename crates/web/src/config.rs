//! Runtime configuration, read once from environment variables at startup.
//!
//! | Variable               | Meaning                                                        |
//! |------------------------|----------------------------------------------------------------|
//! | `STORE`                | `memory` or `dynamo`. Default: `memory` locally, `dynamo` in Lambda. |
//! | `TABLE_NAME`           | DynamoDB table (required when `STORE=dynamo`).                 |
//! | `PUBLIC_BASE_URL`      | e.g. `https://choosey.app`. Used for absolute links and the Origin check. Default `http://127.0.0.1:3000`. |
//! | `ORIGIN_VERIFY_SECRET` | Shared secret CloudFront sends in `x-origin-verify`. Required in Lambda. |
//! | `RUST_LOG`             | Log filter, default `info`.                                    |

use axum::http::Uri;

/// Default base URL for local development.
pub const LOCAL_BASE_URL: &str = "http://127.0.0.1:3000";

/// Which storage backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreKind {
    Memory,
    Dynamo { table_name: String },
}

/// Everything the app needs to know about its environment.
#[derive(Clone)]
pub struct Config {
    pub store: StoreKind,
    /// Base URL without a trailing slash, e.g. `https://choosey.app`.
    pub public_base_url: String,
    /// `scheme://host[:port]` of [`Self::public_base_url`], compared with the `Origin` header.
    pub public_origin: String,
    /// When set, every request must carry `x-origin-verify: <secret>`.
    pub origin_verify_secret: Option<String>,
    /// Send `Strict-Transport-Security` (only when served over https).
    pub hsts: bool,
}

// Hand-written so the secret can never end up in a log line via `{:?}`.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("store", &self.store)
            .field("public_base_url", &self.public_base_url)
            .field(
                "origin_verify_secret",
                &self.origin_verify_secret.as_ref().map(|_| "<set>"),
            )
            .field("hsts", &self.hsts)
            .finish()
    }
}

impl Config {
    /// Read the configuration from the process environment.
    pub fn from_env(in_lambda: bool) -> Result<Self, String> {
        Self::from_lookup(in_lambda, |k| std::env::var(k).ok())
    }

    /// Build a config from any key lookup (the environment, or a map in tests).
    pub fn from_lookup(
        in_lambda: bool,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        // Treat empty values as unset.
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());

        let store = match get("STORE").as_deref() {
            Some("memory") => StoreKind::Memory,
            Some("dynamo") => StoreKind::Dynamo {
                table_name: get("TABLE_NAME").ok_or("STORE=dynamo needs TABLE_NAME")?,
            },
            None if !in_lambda => StoreKind::Memory,
            None => StoreKind::Dynamo {
                table_name: get("TABLE_NAME").ok_or("TABLE_NAME is required in Lambda")?,
            },
            Some(other) => return Err(format!("unknown STORE {other:?}; use memory or dynamo")),
        };

        let base = get("PUBLIC_BASE_URL").unwrap_or_else(|| LOCAL_BASE_URL.to_string());
        let base = base.trim().trim_end_matches('/').to_string();
        let uri: Uri = base
            .parse()
            .map_err(|_| format!("PUBLIC_BASE_URL {base:?} is not a valid URL"))?;
        let scheme = uri.scheme_str().unwrap_or_default();
        let authority = uri.authority().map(|a| a.as_str()).unwrap_or_default();
        if !matches!(scheme, "http" | "https") || authority.is_empty() {
            return Err(format!(
                "PUBLIC_BASE_URL {base:?} must look like https://example.com"
            ));
        }
        let public_origin = format!("{scheme}://{authority}").to_ascii_lowercase();

        let origin_verify_secret = get("ORIGIN_VERIFY_SECRET");
        if in_lambda && origin_verify_secret.is_none() {
            // Fail closed: without the secret the Function URL would be open to anyone who finds it.
            return Err("ORIGIN_VERIFY_SECRET must be set when running in Lambda".into());
        }

        Ok(Self {
            store,
            hsts: scheme == "https",
            public_base_url: base,
            public_origin,
            origin_verify_secret,
        })
    }

    /// Defaults for tests: memory store, local URL, no origin secret.
    pub fn for_tests() -> Self {
        Self::from_lookup(false, |_| None).expect("default config is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(lambda: bool, vars: &[(&str, &str)]) -> Result<Config, String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(lambda, |k| map.get(k).cloned())
    }

    #[test]
    fn local_defaults() {
        let c = cfg(false, &[]).unwrap();
        assert_eq!(c.store, StoreKind::Memory);
        assert_eq!(c.public_base_url, LOCAL_BASE_URL);
        assert_eq!(c.public_origin, LOCAL_BASE_URL);
        assert!(!c.hsts);
        assert!(c.origin_verify_secret.is_none());
    }

    #[test]
    fn lambda_fails_closed_without_origin_secret() {
        let vars = [("STORE", "dynamo"), ("TABLE_NAME", "t")];
        assert!(cfg(true, &vars).is_err());
        assert!(cfg(true, &[("ORIGIN_VERIFY_SECRET", ""), vars[0], vars[1]]).is_err());
        let c = cfg(true, &[("ORIGIN_VERIFY_SECRET", "s"), vars[0], vars[1]]).unwrap();
        assert_eq!(
            c.store,
            StoreKind::Dynamo {
                table_name: "t".into()
            }
        );
        assert!(!format!("{c:?}").contains("\"s\""));
    }

    #[test]
    fn base_url_parsing() {
        let c = cfg(false, &[("PUBLIC_BASE_URL", "https://Choosey.app/")]).unwrap();
        assert_eq!(c.public_base_url, "https://Choosey.app");
        assert_eq!(c.public_origin, "https://choosey.app");
        assert!(c.hsts);
        assert!(cfg(false, &[("PUBLIC_BASE_URL", "choosey.app")]).is_err());
        assert!(cfg(false, &[("PUBLIC_BASE_URL", "ftp://x")]).is_err());
        assert!(cfg(false, &[("STORE", "dynamo")]).is_err());
        assert!(cfg(false, &[("STORE", "postgres")]).is_err());
    }
}
