# Choosey

A weighted decision-matrix web app to help get past analysis paralysis. List what matters (criteria),
say how much each matters (weights), score your options, and see a ranked result with a breakdown.

Status: under construction.

## Layout

```
crates/core    domain types and scoring (pure, no I/O)
crates/store   Store trait, in-memory and DynamoDB implementations
crates/web     axum web app (binary `choosey`)
infra/         Terraform (added separately)
```

## Local development

Rust is pinned to the stable channel by `rust-toolchain.toml`.

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```
