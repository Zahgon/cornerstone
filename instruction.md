# Task: Migrate this project's web framework from Axum to Actix Web

The `backend` crate in this Cargo workspace serves a JSON API and a single-page frontend
using Axum. Port it to Actix Web, preserving its behavior exactly.

## Requirements
- Replace Axum and its ecosystem crates (`axum`, `axum-extra`, `tower`, `tower-http`,
  `tower_governor`) with Actix Web equivalents. No Axum dependency may remain.
- Keep the HTTP surface identical: same paths, same methods, same status codes, same
  response bodies and headers, for both success and every error case.
- Keep authentication, rate limiting, CORS, request tracing, API documentation, static
  file serving and graceful shutdown working as they do now.
- Leave the database layer, configuration loading, migrations, shared DTOs in `common/`,
  and both frontends unchanged.
- Update the README to reflect the new framework.

## Done when
`cargo test` passes, `cargo fmt --all --check` is clean, and
`cargo clippy --workspace --all-targets -- -D warnings` reports no warnings.
