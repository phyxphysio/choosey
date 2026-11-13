//! Choosey web app: axum + askama + htmx, runnable locally or on AWS Lambda.
//!
//! The binary (`src/main.rs`) reads [`config::Config`], picks a store and serves [`app::router`].

pub mod app;
mod assets;
pub mod config;
mod error;
mod export;
mod handlers;
mod middleware;
mod views;

pub use middleware::ORIGIN_VERIFY_HEADER;
