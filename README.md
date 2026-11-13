# Choosey

A weighted decision-matrix web app to help get past analysis paralysis. List what matters
(criteria), say how much each one matters (weights), score your options, and get a ranked result
with a breakdown of what drove each score.

- Start blank or from a template (rent/buy a home, where to retire, job offer).
- Set weights with sliders, or rank the criteria and get suggested weights (Rank Order Centroid).
- Score as a 0-10 rating, or enter real numbers (price, minutes) that are scaled automatically,
  either between fixed "worst" and "best" bounds or min-max across your options.
- Missing scores never count as zero: totals use only what's been scored, and an "incomplete"
  badge shows how much of the weight is covered.
- No accounts. Each decision has a private edit link and a separate view-only link. Export to CSV.

Works without JavaScript (plain forms); with JavaScript, htmx updates results as you type.

## Run it locally

Rust is pinned to the stable channel by `rust-toolchain.toml` (rustup installs it on first use).

```sh
STORE=memory cargo run -p choosey-web
# open http://127.0.0.1:3000
```

The memory store loses everything when the process exits. Open the app at exactly
`http://127.0.0.1:3000` (not `localhost`): POSTs whose `Origin` doesn't match `PUBLIC_BASE_URL`
are rejected. To use another address, set `PUBLIC_BASE_URL` to match it.

## Tests

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- `crates/core`: scoring maths with hand-calculated examples, ROC weights for n = 1..6 with ties,
  and property tests (totals stay in [0, 100]; scaling every weight changes nothing).
- `crates/store`: a shared conformance check run against each backend.
- `crates/web/tests/http.rs`: the HTTP flows, driven in-process with `tower::ServiceExt::oneshot`,
  including security headers, CSP-clean HTML, validation, 404s and Origin/secret checks.

### DynamoDB Local integration test

Ignored by default because it needs Docker:

```sh
docker run --rm -d -p 8000:8000 --name choosey-dynamo amazon/dynamodb-local
cargo test -p choosey-store --test dynamo_local -- --ignored
docker stop choosey-dynamo
```

Set `DYNAMODB_ENDPOINT` to point somewhere other than `http://localhost:8000`. You can also run the
whole app against it:

```sh
AWS_REGION=us-east-1 AWS_ACCESS_KEY_ID=local AWS_SECRET_ACCESS_KEY=local \
AWS_ENDPOINT_URL_DYNAMODB=http://localhost:8000 STORE=dynamo TABLE_NAME=<table> \
cargo run -p choosey-web
```

(The table must exist with a string hash key `pk`; the integration test shows how to create one.)

## Configuration

All configuration is through environment variables, read once at startup (`crates/web/src/config.rs`).

| Variable               | Meaning |
|------------------------|---------|
| `STORE`                | `memory` or `dynamo`. Default: `memory` locally, `dynamo` in Lambda. |
| `TABLE_NAME`           | DynamoDB table name. Required with `STORE=dynamo`. |
| `PUBLIC_BASE_URL`      | e.g. `https://choosey.app`. Used to build share/edit links (never the `Host` header) and for the `Origin` check. Default `http://127.0.0.1:3000`. HSTS is sent when this is `https`. |
| `ORIGIN_VERIFY_SECRET` | If set, every request must carry header `x-origin-verify` with this value (CloudFront adds it). **Required in Lambda**: the app refuses to start without it. |
| `RUST_LOG`             | Log filter, default `info` (HTTP/Lambda libraries at `warn`). |
| `AWS_REGION` etc.      | Standard AWS SDK settings, used by the DynamoDB store. |

The app name lives in one place, `choosey_core::APP_NAME`.

## Architecture

```
crates/core    domain types, scoring, ROC, templates, validation (pure: no async, no I/O)
crates/store   Store trait; MemoryStore; DynamoStore (one item per decision)
crates/web     axum app, askama templates, static assets, main.rs (binary `choosey`)
infra/         Terraform (AWS + Cloudflare)
.github/       CI (fmt, clippy, test, cargo deny, terraform) and deploy
```

- **One binary, two modes.** If `AWS_LAMBDA_RUNTIME_API` is set, `main.rs` runs the router under
  `lambda_http` (Lambda Function URL, `provided.al2023`, arm64); otherwise it serves
  `127.0.0.1:3000`.
- **Web.** axum + askama (templates compiled into the binary, auto-escaped) + htmx 2 (vendored)
  and hand-written CSS. Static files are embedded with `include_bytes!` and served under
  content-versioned names. Every change is a POST that redirects back to a GET page; htmx makes the
  same requests in the background and swaps in the relevant fragment (`hx-select`).
- **Data.** Each decision is one DynamoDB item: `pk = "DEC#<view_id>"` plus the serde
  serialisation of `Decision` (raw values, not normalised scores, so changing weights or bounds
  recomputes everything). Scores are keyed by participant for future multi-person decisions; the
  MVP has one participant. Writes are conditional on a `version` attribute (optimistic concurrency;
  a lost race shows a "changed elsewhere, reload" page). DynamoStore only uses `GetItem`, `PutItem`
  and `DeleteItem`.
- **Scoring.** See `crates/core/src/scoring.rs` and PLAN.md section 2.

### Build for Lambda

```sh
cargo lambda build --release --arm64 --output-format zip -p choosey-web
# -> target/lambda/choosey/bootstrap.zip (handler: bootstrap, runtime: provided.al2023, arm64)
```

Needs [cargo-lambda](https://www.cargo-lambda.info/) and zig. CI does this on every push to `main`
and deploys with `aws lambda update-function-code` after assuming `vars.AWS_DEPLOY_ROLE_ARN` over
GitHub OIDC, in the `production` environment. The deploy job skips cleanly while
`AWS_DEPLOY_ROLE_ARN` is unset.

## Security notes

- **Secret links.** `view_id` and the edit secret are each 128 bits from the OS CSPRNG, base62
  encoded. Only a SHA-256 hash of the edit secret is stored, and it's compared in constant time.
  Unknown decisions and wrong secrets give the identical 404 page. Malformed ids are rejected before
  touching the store.
- **No leaks of links.** `Referrer-Policy: no-referrer` everywhere, `X-Robots-Tag: noindex` on
  `/d/*`, `Cache-Control: no-store` on everything except `/static/*`, and htmx's history cache is
  disabled so pages aren't copied into `localStorage`. Request logs record the route template
  (`/d/:id/edit/<redacted>/scores`), method, status and latency, never the URL; only the first 4
  characters of a view id are ever logged. Don't run production with `RUST_LOG=debug`: dependencies
  may log request details at that level.
- **Strict CSP.** `default-src 'self'`, no inline scripts, styles, `style=` attributes or event
  handlers (tests check the HTML). htmx is configured not to inject its indicator `<style>` and not
  to use `eval`. The breakdown bar is an SVG sized with geometry attributes rather than inline CSS.
  Plus `nosniff`, `frame-ancestors 'none'`/`X-Frame-Options: DENY`, a restrictive
  `Permissions-Policy`, and HSTS (with preload) when served over https.
- **Origin protection.** The Lambda Function URL is public (auth type `NONE`, because CloudFront's
  signed origin requests would require clients to hash POST bodies, which HTML forms can't).
  CloudFront adds a secret `x-origin-verify` header; the app rejects requests without it (403, no
  detail) and refuses to start in Lambda if the secret isn't configured.
- **CSRF.** There are no cookies or other ambient credentials, so classic CSRF doesn't apply. As
  defence in depth, state changes are POST-only and POSTs with a foreign `Origin` are rejected.
- **Input.** 64 KB body limit; names up to 100 characters (control characters stripped), up to 30
  criteria and 50 options; non-finite and absurdly large numbers rejected; ratings must be 0-10 in
  half steps. CSV export guards against spreadsheet formula injection and uses a fixed file name.
- **Supply chain.** Every GitHub Action is pinned to a commit SHA; workflows default to
  `contents: read` and only the deploy job gets `id-token: write`. `cargo deny` checks advisories,
  licences and sources; Dependabot watches cargo, actions and Terraform. `Cargo.lock` is committed.
- **Vendored htmx.** `crates/web/static/htmx-2.0.11.min.js` is `dist/htmx.min.js` from the npm
  package `htmx.org@2.0.11` (tarball integrity verified against the registry). SHA-256:
  `d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717`. To upgrade, replace the
  file and update its name and hash in `crates/web/src/assets.rs` and here.
- **Private data.** Never commit real decision data. `.gitignore` excludes `NOTES.md`, `*.csv`
  (except under `fixtures/`), `.env*`, Terraform state and `*.tfvars`. Fixtures use made-up names.
