# Choosey: architecture and build plan

Brief for a coding agent. The owner is an experienced software engineer who is learning Rust, so write idiomatic,
well-commented Rust and keep things simple. Don't add anything outside this plan without asking.

## 1. What it is
A weighted decision-matrix web app that helps people get past analysis paralysis. A user:
1. Creates a decision (blank or from a template), e.g. "Which flat?" or "Where to retire?"
2. Lists criteria and sets how important each one is. They can rank the criteria to get suggested weights,
   then fine-tune with sliders.
3. Adds options (the properties, towns, etc.).
4. Scores each option on each criterion, either as a 0-10 rating or as a real value (price, commute minutes)
   that the app scales automatically.
5. Sees a ranked result with a breakdown of what contributed to each score.

No accounts. Each decision has a secret edit link and a separate view-only link to share.

## 2. Scoring model (the `core` crate: pure functions, no I/O, thoroughly tested)

Definitions:
- Criterion i has weight `w_i` (an integer from 0 to 10; 0 means the criterion is ignored).
- Option o has a normalised score `s_oi` from 0 to 10 for criterion i, or None if it hasn't been scored.

Criterion kinds:
- `Rated`: the user enters 0-10 (half steps allowed). `s = value`.
- `Numeric { direction: HigherBetter | LowerBetter, unit: String, bounds: Option<(worst, best)> }`:
  the user enters a raw value.
  - If `bounds` is set: `s = 10 * clamp((x - worst) / (best - worst), 0, 1)`. This handles both directions,
    because for lower-is-better the worst value is the larger number.
  - Otherwise use min-max across the options that have a value: best gets 10, worst gets 0, the rest are linear in between.
    If all values are the same, or only one option has a value, every option gets 10 (the criterion can't
    separate them, so nobody is penalised).
  - Show a hint in the UI that min-max exaggerates small differences, and suggest setting bounds.

Total for option o, counting only the criteria it has a score for (C_o):
  `total_o = 100 * Σ_{i∈C_o} w_i * s_oi / Σ_{i∈C_o} (w_i * 10)`
- Also return coverage: `Σ_{i∈C_o} w_i / Σ_all w_i`. If coverage is below 100%, show an "incomplete" badge.
  Missing scores must never count as 0 without the user seeing it. (The original spreadsheet treated blanks as 0,
  and that was a bug.)
- Return a per-criterion contribution for each option, `w_i * s_oi / Σ_all(w_i * 10) * 100`, for a stacked
  breakdown bar.
- Rank by total, highest first. Ties are broken by coverage, then by the score on the highest-weighted criterion.
- If every weight is 0, return an error state rather than dividing by zero.

Ranking to set weights: the user orders the criteria (ties allowed). Compute Rank Order Centroid weights
`ROC_k = (1/n) Σ_{j=k..n} 1/j`, then scale so the top criterion is 10 and round to integers (minimum 1).
Tied criteria share the average of their ROC values. These are only suggestions; the user can then move the sliders.

Required tests: hand-calculated examples, min-max in both directions, bounds clamping, a single option,
all values the same, missing scores and coverage, all weights 0, ROC values for n = 1..6 with ties,
and property tests (proptest) that every total is in [0, 100] and that scaling every weight by k changes nothing.

## 3. Architecture

Cargo workspace:
```
crates/core     # domain types and scoring (no async, no I/O)
crates/store    # Store trait + MemoryStore (local and tests) + DynamoStore
crates/web      # axum app, askama templates, static assets, main.rs
infra/          # Terraform
.github/workflows/
```

- **Web:** axum, askama templates, HTMX (vendor `htmx.min.js` into `crates/web/static`, no CDN),
  and hand-written mobile-first CSS (no build step). Serve static assets from the binary, either embedded
  with `include_bytes!`/rust-embed or via tower-http.
- **Interactivity:** forms POST to the server, which re-renders fragments, and HTMX swaps them in.
  For example, a change to a rating triggers `hx-post` and returns the updated results panel. The pages must
  still work as plain forms if JS fails.
- **One binary, two modes:** `main.rs` checks the `AWS_LAMBDA_RUNTIME_API` environment variable. If it is set,
  run `lambda_http::run(app)`; otherwise run `axum::serve` on `127.0.0.1:3000`.
  Pick the store with an environment variable: `STORE=memory|dynamo`.
- Build and deploy with **cargo-lambda**, targeting arm64 (Graviton), with 256 MB memory.
- Use rustls throughout (no OpenSSL), since that keeps cross-compiling for Lambda simple.

## 4. Data model

Store each decision as one DynamoDB item (the size limit is 400 KB, far more than needed):
```
pk           = "DEC#<view_id>"
schema_version: u32
title, template_id: Option, created_at, updated_at
edit_secret_hash  (SHA-256 of the edit secret; never store the secret itself)
criteria:  [{ id, name, kind, weight, position }]
options:   [{ id, name, notes, position }]
scores:    { participant_id -> { option_id -> { criterion_id -> RawValue } } }
participants: [{ id, name }]   # the MVP always has exactly one: "owner"
```
- `view_id` and `edit_secret` are each 128-bit random values, encoded as base62.
- URLs: view at `/d/{view_id}`, edit at `/d/{view_id}/edit/{edit_secret}`. Compare the secret hash in
  constant time.
- Raw values (not normalised scores) are stored, so changing bounds or weights recalculates everything.
- Keying scores by participant is for future multi-person decisions. The MVP UI ignores it.
- Use optimistic concurrency: an `updated_at` / version attribute with a conditional write. If it fails,
  ask the user to reload.
- Serde-serialise the domain struct to a DynamoDB item (`serde_dynamo` crate).

`Store` trait: `create`, `get(view_id)`, `put(decision, expected_version)`, `delete(view_id)`.

## 5. Pages and routes
- `GET /` explains the app, with buttons to start blank or pick a template.
- `POST /d` creates a decision and redirects to its edit URL. Show the edit link once, prominently:
  "Bookmark this link. It's the only way back in."
- The edit page uses mobile-first tabs/steps: **Criteria → Options → Score → Results**.
  - Criteria: add, rename, delete, choose the kind. Weights are set either with sliders or with a
    "Rank instead" mode that offers up/down buttons (avoid drag-and-drop JS), then applies the ROC suggestion.
  - Options: add, rename, delete, notes.
  - Score: one option at a time on mobile (a card for each criterion), with a grid on wide screens.
  - Results: ranked list with the total, a coverage badge and a stacked breakdown bar (CSS only).
- `GET /d/{view_id}` shows a read-only results view with the full table.
- Delete the decision (with a confirmation step on the page itself).
- Export to CSV (stretch goal for the MVP).

Templates are defined as static data in code. Each is a list of criteria with suggested kinds and weights:
- Rent/buy a home: price (numeric, lower is better), commute, sun, warmth, bedrooms, kitchen, storage, parking, neighbourhood.
- Where to retire: cost of living, near family, healthcare access, climate, community, walkability, activities.
- Job offer: pay, growth, commute, team, flexibility, stability.
- Blank.

## 6. Security and privacy
- Send `Referrer-Policy: no-referrer` and `X-Robots-Tag: noindex` on every `/d/*` response so secret links
  don't leak or get indexed. Also set a CSP that allows only self (HTMX is vendored).
- Never log full URLs that contain edit secrets.
- Lambda reserved concurrency is 5, and add an AWS Budgets alert at $5, so a traffic spike can't run up a bill.
- Put CloudFront in front of the Function URL using Origin Access Control (Function URL auth type
  `AWS_IAM`), so the Lambda can't be called directly.
- Validate input lengths: names up to 100 characters, up to 30 criteria, up to 50 options.
- Don't commit the owner's real property spreadsheet. Any test fixtures must use made-up names.

## 7. Infrastructure (Terraform in `infra/`)
- AWS provider: the DynamoDB table (on-demand, pk only), the Lambda function and its IAM role (least privilege
  on the one table), the Function URL, CloudFront (OAC, TLS 1.2+, caching disabled for dynamic routes and
  enabled for `/static/*`), an ACM certificate in **us-east-1**, a CloudWatch log group with 14-day retention,
  the budget alarm, and a GitHub OIDC provider plus a deploy role that can only update this function's code.
- Cloudflare provider: DNS records for the ACM validation CNAME and the apex/www CNAME pointing to CloudFront,
  with the Cloudflare proxy **off** (DNS only). The domain is registered through Cloudflare Registrar.
- State goes in an S3 backend with `use_lockfile = true`. Create the state bucket once by hand (document how).
  Never commit state or `*.tfvars` containing secrets.
- Terraform deploys a placeholder zip at first. From then on, the application code is deployed by CI.
  `terraform apply` is run manually by the owner.

## 8. CI/CD (GitHub Actions)
- On PRs and pushes: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `terraform fmt -check`
  plus `terraform validate`.
- On pushes to main, after the tests pass: build with `cargo lambda build --release --arm64`, assume the
  deploy role via OIDC, then run `cargo lambda deploy` (or `aws lambda update-function-code`).
- Cache cargo with Swatinem/rust-cache.

## 9. Build order (each phase ends with passing tests and a short README update)
0. Scaffold the workspace, CI checks, `.gitignore`, and a README with local run instructions.
1. `core`: types, scoring, ROC, normalisation, and the tests listed in section 2.
2. `store`: the trait and MemoryStore.
3. `web`: routes, templates and HTMX flows, running locally with MemoryStore. Add handler tests using
   `tower::ServiceExt::oneshot`.
4. `DynamoStore`, with an integration test against DynamoDB Local (docker) that is marked `#[ignore]` by default.
5. Terraform and the first manual deploy, then a smoke test through the custom domain.
6. The CI deploy job.
7. Polish: CSV export, a short guided onboarding, accessibility checks (labels, focus, contrast).

After the MVP (keep the design open to these, but don't build them): dealbreakers that filter options out,
a sensitivity view ("the winner changes if weight X moves by N"), several people per decision with combined
results, and importing from CSV.

## 10. Naming
Name: **Choosey**, domain **choosey.app** (the owner will register it through Cloudflare Registrar; not registered yet). Use `choosey` for crates, resources and the `APP_NAME` setting, kept in one place so it is easy
to rename later.
