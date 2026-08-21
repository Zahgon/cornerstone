# Ground Truth — Axum → Actix Web Migration of `cornerstone`

Derived from `golden.patch` (the reference solution), read against the full pre-
and post-patch source trees and validated by running **both** trees side by side.

- **Base repo:** `gramistella/cornerstone`
- **Base commit:** `33ab1145066d3e359e9dd2bb41cfb138a43c7f3b` — *chore: fmt code*
- **Task:** port the HTTP layer from Axum 0.8.6 to Actix Web 4.14.1, preserving observable behaviour
- **Reference solution:** `golden.patch` — 11 file entries, **all modified**;
  0 new / 0 deleted / 0 renamed; **+403 / −249** lines
- **Split for evaluation:** `test.patch` (`backend/tests/contact_api.rs`, +8 / −8 — applied at eval
  time) and `fix.patch` (the other 10 files, +395 / −241 — the withheld reference). Concatenated they
  have identical file/line coverage to `golden.patch`, and applied in sequence onto `33ab114` they
  reproduce the reference tree exactly (verified; only `Cargo.lock` differs, excluded per §1.1).
  `backend/tests/helpers.rs` sits in `fix.patch`, **not** `test.patch`, deliberately: its *public*
  surface (`TEST_JWT_SECRET`, `spawn_app`, `get_auth_token`) is byte-identical across the migration,
  but it internally calls `create_governor_config` and `create_app` — names this migration invents.
  Shipping it as an eval-time test would pin those signatures and leak §2.7's shared-config design.
- **Task statement given to the candidate:** `instruction.md` — deliberately free of the obligations
  below. Every trap in §1.4, §2.5, §2.6, §2.7 and §3.1 is discoverable from the pre-migration source,
  not disclosed.

> **Verification status — executed, not inferred.**
> The claims in this document were confirmed by building both trees and diffing their live HTTP
> behaviour, not by reading source alone. Specifically: the patch applies cleanly to `33ab114` and
> the resulting tree is byte-identical to the working tree it was cut from (checked); `cargo fmt
> --all --check`, `clippy --all-targets -- -D warnings` and the 8-test integration suite all pass;
> and the pre-patch (Axum) and post-patch (Actix) binaries were run concurrently on two ports and
> compared across ~60 request probes plus a full CRUD/auth transcript, which came back
> **byte-identical** apart from the single deviation in §4.1. The §7.2 probe tables below are
> recorded observations from that run.

Obligations are written so a verifier can grade a candidate migration without requiring it to be
textually identical to the golden patch. **Completion is gated on three independent test types —
§7.1 test cases, §7.2 behaviour tests, §7.3 coverage tests. All three MUST pass.** `MUST` items
are load-bearing; `SHOULD` items the golden patch satisfies but a different-but-correct solution may
achieve another way.

---

## 0. Task framing

The Axum surface in this repo is concentrated in five seams. Everything else — `sqlx` queries and
migrations, `figment` config loading, JWT issue/verify, bcrypt, the `common` DTOs, the Slint and
Svelte frontends — is framework-agnostic and MUST be left alone.

| Seam | Axum 0.8 | Actix Web 4 |
|---|---|---|
| App construction | `Router` value, `.route()/.nest()/.merge()`, `.with_state()` | `App` rebuilt **per worker** by a closure; `.service()` / `.app_data()` |
| Handler return | `impl IntoResponse` — tuples, `StatusCode`, `Json<T>` | `Result<HttpResponse, E>` / `Result<web::Json<T>, E>` |
| Error type | `impl IntoResponse for AppError` | `impl ResponseError for AppError` (`status_code` + `error_response`) |
| Extractors | `State<T>`, `Path<T>`, `Query<T>`, `Json<T>`, `FromRequestParts` | `web::Data<T>`, `web::Path<T>`, `web::Query<T>`, `web::Json<T>`, `FromRequest` |
| Middleware | `tower` layers, applied **bottom-up**, `.layer()/.route_layer()` | `.wrap()`, applied **in reverse registration order** |

The mechanical rewrite compiles easily. The five places where it silently changes behaviour — and
therefore where a candidate is actually graded — are §1.4 (fallback ordering), §2.5 (extractor
rejection mapping), §2.6 (SPA fallback status), §2.7 (per-worker rate-limiter construction) and §3.3 (graceful
shutdown under `disable_signals`).

---

## 1. Structural obligations

### 1.1 Files: none added, none deleted, none renamed

The reference solution modifies exactly these 11 files and creates nothing:

| File | +/− | Nature of change |
|---|---:|---|
| `backend/src/web_server.rs` | +246 / −133 | **the migration** — app factory, routing, CORS, static, governor, extractor configs |
| `backend/src/auth.rs` | +58 / −28 | handler signatures + `auth_middleware` rewritten as `from_fn` |
| `backend/src/error.rs` | +23 / −17 | `IntoResponse` → `ResponseError` |
| `backend/src/main.rs` | +21 / −10 | `HttpServer` bootstrap + graceful shutdown via `ServerHandle` |
| `backend/tests/helpers.rs` | +13 / −13 | test harness spawns `HttpServer` on a std `TcpListener` |
| `backend/src/extractors.rs` | +11 / −12 | `FromRequestParts` → `FromRequest` |
| `backend/Cargo.toml` | +8 / −12 | dependency swap + utoipa feature flags |
| `backend/tests/contact_api.rs` | +8 / −8 | `#[tokio::test]` → `#[actix_web::test]` ×8 |
| `readme.md` | +8 / −8 | docs + architecture diagram |
| `Cargo.toml` | +6 / −7 | workspace dependency swap |
| `common/Cargo.toml` | +1 / −1 | utoipa `axum_extras` → `actix_extras` |

`Cargo.lock` (+608 / −290) is **excluded from the reference patch by design** — it is derived output
that `cargo build` regenerates from the `Cargo.toml` changes already in the patch. A candidate MUST
NOT be required to reproduce it byte-for-byte, and MUST NOT be penalised for including it.

The crate names, the `backend` / `common` / `frontend_slint` / `frontend_svelte` workspace layout,
and the four feature flags (`svelte-ui`, `slint-ui`, `db-sqlite`, `db-postgres`) MUST NOT change.

### 1.2 Dependency swap

Removed from `[workspace.dependencies]`:

```
axum 0.8.6 · axum-extra 0.10.3 · tower 0.5.2 · tower-http 0.6.6 · tower_governor 0.8.0
hyper 1.7.0 · http-body-util 0.1.3          (the last two were test-only)
```

Added:

```
actix-web 4.14.1 · actix-cors 0.7.1 · actix-files 0.7.0 · actix-governor 0.10.0
actix-web-httpauth 0.8.2 · tracing-actix-web 0.7.22
```

Feature-flag renames that MUST be made or the build fails:

| Crate | Before | After |
|---|---|---|
| `utoipa` (backend + **common**) | `axum_extras` | `actix_extras` |
| `utoipa-swagger-ui` | `axum` | `actix-web` |
| `actix-web` (backend) | `axum` had `macros` | `macros` |

`common/Cargo.toml` is easy to miss — it carries its own `utoipa` feature list and a candidate that
updates only `backend/Cargo.toml` leaves `axum_extras` resolving in the workspace.

The four `[dev-dependencies]` that existed only to drive Axum's `oneshot` test style — `hyper`,
`http-body-util`, `tower` — MUST be dropped. The suite talks to a real socket via `reqwest` both
before and after, so nothing replaces them.

### 1.3 Signature conversions

| Before (Axum) | After (Actix) |
|---|---|
| `State(state): State<AppState>` | `state: web::Data<AppState>` |
| `Path(id): Path<i64>` | `id: web::Path<i64>` + `.into_inner()` |
| `axum::extract::Query(p): Query<Pagination>` | `p: web::Query<Pagination>` + `.into_inner()` |
| `Json(dto): Json<T>` | `dto: web::Json<T>` + `.into_inner()` |
| `-> Result<Json<T>, AppError>` | `-> Result<web::Json<T>, AppError>` |
| `-> Result<StatusCode, AppError>` | `-> Result<HttpResponse, AppError>` + `.finish()` |
| `-> Result<(StatusCode, Json<T>), AppError>` | `-> Result<HttpResponse, AppError>` + `.json(v)` |
| `#[debug_handler]` | removed (no actix equivalent; not a behaviour change) |
| `impl FromRequestParts<AppState> for AuthUser` | `impl FromRequest for AuthUser` |
| `impl IntoResponse for AppError` | `impl ResponseError for AppError` |
| `#[tokio::main]` / `#[tokio::test]` | `#[actix_web::main]` / `#[actix_web::test]` |

`AuthUser`'s extractor MUST return a `Ready` future and MUST **clone** out of `req.extensions()` —
holding the `Ref` across an await point does not compile, and returning a reference is impossible
through `FromRequest`.

### 1.4 The static/SPA service MUST be registered last — mandatory, load-bearing

Axum distinguished routes from the fallback structurally (`.fallback_service(...)`). Actix does not:
`Files::new("/", …)` is a prefix service at the root that will swallow **every** request if it is
registered before the API resources. The reference registers all seven API resources first and the
static service last:

```rust
app.service(/* /api/v1/health   */)
   .service(/* /api/v1/register */)
   … five more …
   .service(create_static_service())   // MUST be last
   .wrap(TracingLogger::default())
   .wrap(from_fn(set_request_id))
   .wrap(cors)
```

A candidate that registers `Files` earlier returns the SPA index for `/api/v1/login` and **still
passes a naive smoke test of `/`**. §7.2 probes 1–4 are what catch it.

Equally load-bearing: `.wrap()` applies in **reverse** registration order, so the three calls above
produce the outermost-to-innermost chain `cors → set_request_id → TracingLogger`. `set_request_id`
MUST be wrapped *after* `TracingLogger` so the header exists by the time the span is created. A
candidate that reverses them compiles, serves traffic, and silently logs `request_id` as absent.

---

## 2. Preservation obligations

### 2.1 Route table — exact, 7 resources / 11 method-route pairs

| Path | Methods | Handler | Rate-limited | Auth |
|---|---|---|:--:|:--:|
| `/api/v1/health` | GET | `health_check` | ✅ | — |
| `/api/v1/register` | POST | `auth::register` | ✅ | — |
| `/api/v1/login` | POST | `auth::login` | ✅ | — |
| `/api/v1/refresh` | POST | `auth::refresh` | ✅ | — |
| `/api/v1/logout` | POST | `auth::logout` | — | ✅ |
| `/api/v1/contacts` | GET, POST | `get_contacts`, `create_contact` | — | ✅ |
| `/api/v1/contacts/{id}` | GET, PUT, DELETE | `get_contact`, `update_contact`, `delete_contact` | — | ✅ |
| `/docs` | GET | redirect `303` → `/docs/` | — | — |
| `/docs/{_:.*}` | GET | `SwaggerUi` | — | — |
| `/api-docs/openapi.json` | GET | `ApiDoc::openapi()` | — | — |
| `/{everything else}` | GET, HEAD | `Files` → SPA fallback | — | — |

The Axum original expressed the prefix with `.nest("/api/v1", …)`. Actix has no equivalent that
composes with per-resource middleware here, so the reference **inlines the full path** into each
`web::resource(...)`. That is an accepted implementation choice, not a required one — a candidate
using `web::scope("/api/v1")` is equally correct **provided** the rate-limit and auth wrapping stay
per-resource as in §2.2.

`/docs` and `/api-docs/openapi.json` MUST remain behind `cfg!(debug_assertions)`.

**The `/docs` → `/docs/` redirect is required.** The Axum build answers a bare `GET /docs` with
**303** (observed, not assumed — `utoipa-swagger-ui`'s axum integration redirects to the trailing-slash
form). The actix wildcard mount `SwaggerUi::new("/docs/{_:.*}")` does not match the bare `/docs` at
all, so the reference reproduces the 303 explicitly with
`web::redirect("/docs", "/docs/").see_other()`. A candidate that omits it regresses `/docs` from
**303 to 404** while `/docs/` keeps working — easy to miss by hand, caught by probe 5.

**`/api/v1/health` is absent from the OpenAPI `paths(...)` list.** This is pre-existing — it is
absent before the migration too — and MUST NOT be "fixed". The generated spec is byte-identical
across the migration (§7.2.3) and adding `health_check` to `ApiDoc` would break that.

### 2.2 Middleware stacking — exact

| Scope | Middleware | Note |
|---|---|---|
| App, outermost | `Cors` | `allowed_origin(cfg.web.cors_origin)`, methods GET/POST/PUT/DELETE/OPTIONS, headers `AUTHORIZATION`+`CONTENT_TYPE`, `supports_credentials()` |
| App | `from_fn(set_request_id)` | inserts `x-request-id` UUID on the **request** if absent |
| App, innermost | `TracingLogger::default()` | replaces `tower_http::trace::TraceLayer` |
| The 4 public resources | `Governor::new(&governor_conf)` | one **shared** limiter, see §2.7 |
| The 3 protected resources | `from_fn(auth::auth_middleware)` | replaces `middleware::from_fn_with_state` |

`set_request_id` sets the header on the **request only**, never on the response — matching
`tower_http`'s `SetRequestIdLayer`, which is what the original used (`PropagateRequestIdLayer`, the
response-side counterpart, was never wired up). A candidate that also echoes `x-request-id` back to
the client has changed observable behaviour and fails §7.2 probe 21.

### 2.3 Request-scoped `AuthUser` — exact

`auth_middleware` MUST insert `AuthUser { id, email }` into `req.extensions_mut()` **before**
calling `next.call(req)`, and `AuthUser::from_request` MUST read it back out. A missing value is an
**internal error**, not a 401 — the middleware is supposed to have run:

```rust
AppError::InternalServerError(
    "AuthUser not found in request extensions. Is the auth middleware missing?".into()
)
```

This preserves the original's contract exactly, including the message.

`auth_middleware` reads `AppState` from `req.app_data::<web::Data<AppState>>()` rather than through
a state parameter (actix `from_fn` middleware takes no state argument). A `None` there MUST also map
to `InternalServerError`, not to a panic or a 401.

### 2.4 Status codes and response bodies — exact

`ResponseError` MUST implement **both** methods, and they MUST agree:

| `AppError` variant | Status | Body |
|---|---:|---|
| `InternalServerError(msg)` | 500 | `{"error": msg}` |
| `DatabaseError(e)` | 500 | `{"error":"A database error occurred"}` |
| `JwtError(_)` | 401 | `{"error":"Invalid token"}` |
| `PasswordError(_)` | 401 | `{"error":"Invalid password"}` |
| `Unauthorized` | 401 | `{"error":"Invalid credentials"}` |
| `Conflict(msg)` | 409 | `{"error": msg}` |
| `NotFound` | 404 | `{"error":"Resource not found"}` |
| `ValidationError(errs)` | 422 | `{"error": "Input validation failed: …", "details": errs}` |

Handler-level statuses that MUST be preserved: `register` → **201** empty, `create_contact` → **201**
+ JSON body, `delete_contact` → **204** empty, `logout` → **204** empty, `get_contact`/`update_contact`
miss → **404** (never 403 — the original deliberately hides existence), `health_check` → **200** empty.

Because `self` is borrowed in `error_response(&self)` rather than moved as in `into_response(self)`,
the `String` payloads MUST be `.clone()`d. This is a borrow-checker consequence, not a design choice.

> The `details` object is the load-bearing half of the 422 body and is asserted byte-for-byte in
> §7.2. The human-readable `error` string interpolates a `HashMap`, so **field order inside it is
> non-deterministic across runs in both frameworks** — a verifier MUST compare `details`, not the
> prose string.

### 2.5 Extractor rejection mapping — the real re-implementation work

Axum's `Json`/`Path` rejections have no automatic actix equivalent. The reference restores them with
two `app_data` configs. **These are the highest-value non-mechanical obligations in the task.**

`web::JsonConfig::default().error_handler(...)` MUST map:

| `JsonPayloadError` | Status | Axum counterpart |
|---|---:|---|
| `Deserialize(e)` where `e.classify() == Category::Data` | **422** | `JsonDataError` |
| `ContentType` | **415** | `MissingJsonContentType` |
| `OverflowKnownLength { .. }` \| `Overflow { .. }` | **413** | `DefaultBodyLimit` / `LengthLimitError` |
| everything else (syntax, EOF, IO) | **400** | `JsonSyntaxError` |

`web::PathConfig::default().error_handler(|e, _| ErrorBadRequest(e))` → **400**, matching Axum's
`Path` rejection. Query rejections need no config; actix already returns 400.

Two traps here:

1. **`Category::Data` vs `Category::Syntax`.** Collapsing all `Deserialize` errors to 400 turns
   every wrong-shaped-but-valid JSON body from 422 into 400. Probes 14–15 catch it.
2. **Both overflow variants.** Matching only `OverflowKnownLength` leaves chunked / unknown-length
   bodies falling through the `_` arm to 400 while `Content-Length` bodies correctly give 413.
   Probe 18 sends a chunked 3 MB body specifically to catch a candidate that handles only the
   known-length case.

The 2 MB limit itself is unchanged: actix's `JsonConfig` default and Axum's `DefaultBodyLimit`
default are both 2 MiB, so the boundary needs no configuration — only the rejection *status* does.

### 2.6 Static files and the SPA fallback — exact

`create_static_service()` MUST reproduce `ServeDir::new(dir).not_found_service(ServeFile::new(index))`:

| Requirement | Mechanism | Why |
|---|---|---|
| Fallback serves `index.html` **with status 404** | `*res.status_mut() = StatusCode::NOT_FOUND` | `tower_http`'s `not_found_service` wraps the fallback in `SetStatus(404)`. It is **not** a 200. |
| Dotfiles are served | `.use_hidden_files()` | actix-files blocks leading-dot segments by default; `ServeDir` did not. Needed for `.well-known/`. |
| No charset suffix on text mime types | `.prefer_utf8(false)` | matches `ServeDir`'s `content-type: text/css`, not `text/css; charset=utf-8` |
| Directory requests serve `index.html` | `.index_file("index.html")` | |
| Missing index → 404; unreadable index → 500 | `io::ErrorKind::NotFound` arm vs `_` arm | preserves `handle_error`'s 500-with-message |

The 404-not-200 detail is the one most likely to be "corrected" by a candidate that assumes SPA deep
links should return 200. **It MUST return 404**, because that is what the pre-migration server did.

`.use_hidden_files()` loosens only the leading-dot check. It does **not** affect `..` resolution —
verified against 14 traversal vectors (`../`, `..%2f`, `%2e%2e`, double-encoded `..%252f`,
`....//`, backslash, mixed `/./../`, and a climb to `/etc/passwd`), none of which served a byte from
outside the static root in either build (§7.2 probes 25–27).

### 2.7 Rate limiter — method rename and per-worker construction

```rust
// Axum                                    // Actix
GovernorConfigBuilder::default()           GovernorConfigBuilder::default()
    .per_second(cfg.ratelimit.per_second)      .seconds_per_request(cfg.ratelimit.per_second)
    .burst_size(cfg.ratelimit.burst_size)      .burst_size(cfg.ratelimit.burst_size)
```

`seconds_per_request` is the exact rename of `tower_governor`'s `per_second` — both bodies are
literally `self.period = Duration::from_secs(seconds)`, confirmed in both crates' sources. The
configured rate is therefore unchanged by the migration.

`actix_governor` **also still exposes `per_second`**, `#[deprecated(since = "0.6.0")]` with the note
*"Might be the inverse of what's expected. Use `seconds_per_request` as an exact replacement."*
Be precise about what that costs a candidate: the deprecated method's body is **identical**, so
keeping the same-named call does **not** invert the rate — it produces a deprecation warning, which
fails the `-D warnings` gate in §7.3 but leaves §7.2 probes 22–24 green. A verifier MUST NOT claim a
behavioural rate change here; the failure is a lint failure.

The genuine behavioural trap in this section is the next paragraph — per-worker config construction,
which no lint catches.

Three further requirements:

- The config MUST be built **once** and shared. `HttpServer::new` runs its factory per worker; a
  `GovernorConfigBuilder` inside the closure gives each worker its own bucket, multiplying the
  effective limit by the core count. The reference hoists it into
  `create_governor_config(&AppConfig) -> Arc<AppGovernorConfig>` called once in `main`.
- `Governor::new(&conf)` clones the `Arc`'d limiter, so all four public resources share **one**
  bucket — matching Axum, where a single `GovernorLayer` wrapped the merged public router.
- The `retain_recent()` cleanup task (60 s interval) MUST be preserved and MUST be spawned once,
  alongside the config.

The public type alias `AppGovernorConfig = GovernorConfig<PeerIpKeyExtractor, NoOpMiddleware>` is
required only because `helpers.rs` needs to name the type; it is an implementation detail.

### 2.8 Untouched surface

`backend/src/config.rs`, `backend/src/db.rs`, `backend/src/lib.rs`, `backend/build.rs`,
`backend/migrations/**`, `.sqlx/**`, `common/src/**`, `frontend_svelte/**`, `frontend_slint/**`,
`Dockerfile`, `docker-compose.yml`, `justfile`, `.github/workflows/**`, `Config.toml`, `.env.example`
are **not modified by the reference patch** and MUST NOT be modified by a candidate. The CI workflow
and Dockerfile are already framework-agnostic (they drive `just` targets and cargo features); a
candidate that edits them has done unnecessary work and MUST be flagged.

---

## 3. Behavioural contract

### 3.1 `auth_middleware` truth table — MUST hold exactly

| Request `Authorization` | Result |
|---|---|
| absent | **401** `{"error":"Invalid credentials"}` |
| present but unparseable as Bearer (`garbage`, `Basic abc`, `Bearer ` empty) | **400** `invalid HTTP header (authorization)` |
| `Bearer <malformed/expired/wrong-signature>` | **401** |
| `Bearer <valid>` but user row deleted | **401** |
| `Bearer <valid>` and user exists | `next.call(req)` with `AuthUser` in extensions |

The **absent-vs-malformed split is load-bearing**: it reproduces `Option<TypedHeader<Authorization<Bearer>>>`,
where a missing header deserialised to `None` (→ the handler's own 401) but a present-and-broken
header was a `TypedHeaderRejection` (→ 400). The reference reproduces it explicitly:

```rust
let auth_header = if req.headers().contains_key(header::AUTHORIZATION) {
    Some(Authorization::<Bearer>::parse(&req)
        .map_err(|_| ErrorBadRequest("invalid HTTP header (authorization)"))?)
} else { None };
```

A candidate that maps all header failures to 401 collapses the distinction and fails probe 9.

Note that resource-level `.wrap()` runs **before** method matching, so `PATCH /api/v1/contacts`
without a token returns **401, not 405**. This matches Axum, where `route_layer` behaved the same
way — verified on both builds (probe 12), so it is parity, not a regression.

### 3.2 Authorization

Every contacts query MUST keep its `AND user_id = $2` clause. A contact belonging to another user
MUST surface as **404**, never 403 — this is deliberate existence-hiding and is asserted by
`test_contacts_authorization`.

### 3.3 Server bootstrap and graceful shutdown

`main` MUST:

1. build the governor config **once**, before `HttpServer::new`;
2. construct the app inside the factory closure — `move || create_app(state.clone(), conf.clone())`;
3. call `.disable_signals()`, so actix's built-in handlers do not race the repo's own
   `shutdown_signal()` (SIGINT + SIGTERM);
4. spawn a task that awaits `shutdown_signal()` then calls `server_handle.stop(true)` — `true` is
   the graceful flag and MUST NOT be `false`;
5. `await` the server, then log and drop the pool.

Omitting `.disable_signals()` leaves two handlers competing for SIGTERM and makes shutdown
non-deterministic. Passing `stop(false)` drops in-flight requests. Both were verified: post-patch
SIGTERM exits cleanly with `Server shut down gracefully. Closing database connections.`, identical
to pre-patch (probe 28).

`#[actix_web::main]` gives the main thread a single-threaded `actix-rt` System; `tokio::spawn` still
works inside it, which is why the governor cleanup task and the shutdown watcher are unchanged.

### 3.4 Test harness

`helpers::spawn_app` MUST switch to a **std** `TcpListener` (`std::net::TcpListener::bind`, not
`tokio::net`) because `HttpServer::listen` takes the std type, and MUST hand the running server to
`actix_web::rt::spawn`. The suite continues to drive a real socket through `reqwest`; no test uses
in-process `test::call_service`, so no assertion needed rewriting beyond the attribute swap.

---

## 4. Accepted deviations

Framework-inherent differences the golden patch accepts. A verifier MUST NOT penalise them.

| # | Deviation | Rationale |
|---|---|---|
| 4.1 | `OPTIONS` preflight from a **disallowed** origin: Axum `200` → Actix **`400`** | `actix-cors` rejects the preflight outright; `tower-http` returned 200 with no CORS headers. Neither grants access, so browsers block identically. **The only status-code difference in the entire probe set.** |
| 4.2 | Percent-encoded-slash paths (`/..%2fsecret`, `/%2e%2e%2fx`): Axum `404` → Actix `400` | actix-router rejects the encoded segment before routing. Both refuse; neither serves content. |
| 4.3 | Static responses gain `ETag` and `content-disposition: inline; filename="…"` | `actix-files` sets them, `ServeDir` did not. Strictly better caching; no consumer depends on their absence. |
| 4.4 | Extractor rejection **message text** differs (`Json deserialize error: …` vs `Failed to deserialize the JSON body into the target type: …`) | Framework-generated prose. Statuses and content types match; no test or client asserts the string. |
| 4.5 | `vary` header casing: `origin, access-control-request-method…` → `Origin, Access-Control-Request-Method…` | HTTP header values here are case-insensitive to every real consumer. |
| 4.6 | Access-log lines change from `tower_http`'s format to `tracing-actix-web`'s span fields | Cosmetic; `request_id` is still present and still populated by §2.2. |
| 4.7 | `#[debug_handler]` removed with no replacement | Actix has no equivalent attribute. It was a compile-time diagnostic aid only. |

---

## 5. Divergences the golden patch does **not** resolve

**5.1 The migration closes a CORS leak — this is an improvement, and it is unavoidable.**
Pre-migration, `tower_http`'s `CorsLayer` with a single `allow_origin(HeaderValue)` emitted
`access-control-allow-origin: http://localhost:5173` on **every** response, including responses to
requests carrying a disallowed `Origin` or no `Origin` at all. `actix-cors` emits it only for
permitted origins. A verifier comparing raw response headers will see the Axum header vanish and
MUST read that as correct, not as a dropped feature. It is not restorable without deliberately
reintroducing the flaw.

**5.2 `/api/v1/health` is undocumented in the OpenAPI spec.** Pre-existing (§2.1). The spec is
byte-identical pre/post, so a candidate that adds it breaks the §7.2.3 equality check even though
the addition is an improvement in isolation. Out of scope.

**5.3 The `error` prose string in 422 bodies has non-deterministic field order.** Pre-existing:
`ValidationErrors` is backed by a `HashMap` and the original interpolated it the same way. The
`details` object is stable. A candidate that sorts the fields is **more** correct than the reference
and MUST NOT be marked down.

**5.4 bcrypt still blocks the worker thread.** `hash`/`verify` run at `DEFAULT_COST` directly in the
handler, with no `web::block`. This was equally true under Axum. Actix's per-worker arbiters mean the
practical impact is unchanged. Wrapping them in `web::block` is an improvement, out of scope, and
MUST NOT be penalised.

---

## 6. Documentation obligations

| File | Required change |
|---|---|
| `readme.md` | Key-features line says **`actix-web`**; rate-limiting bullet says **`actix-governor`**; the Mermaid architecture diagram node `Axum[Axum Web Server]` becomes `Actix[Actix Web Server]` with all four edge references and the `class` line updated; the project-structure comment reads "The Rust Actix Web web server" |

`Config.toml`, `.env.example`, the `justfile` and the CI workflow contain **no** framework-specific
strings and MUST NOT be touched (§2.8). The repo/module name `cornerstone` stays.

A candidate MUST leave no residual references. The gate is:

```bash
! grep -rniE "axum|tower|hyper" \
    --include='*.rs' --include='*.toml' --include='*.md' --include='*.yml' \
    --include='justfile' --include='Dockerfile' \
    --exclude='Cargo.lock' --exclude-dir=target --exclude-dir=.git --exclude-dir=node_modules .
```

`--exclude-dir=target` is not optional: vendored dependency sources under `target/` contain thousands
of matches and will mask the real result. `Cargo.lock` is excluded because the resolved graph
legitimately still names transitive `tower*` / `hyper` crates — `reqwest` depends on both. **Only
first-party sources are in scope for this gate.**

The reference passes this with zero hits (verified).

---

## 7. Completion gates — three test types, all mandatory

All three MUST be green. They are independent: the integration suite cannot see routing precedence
or rate-limit periods, the differential probes cannot see unexercised branches, and coverage cannot
see correctness.

| Gate | What it proves | Command | Pass condition |
|---|---|---|---|
| **§7.1 Test cases** | The ported code is internally correct | `just test-backend-sqlite` | 8/8 pass, 0 failures, 0 skips |
| **§7.2 Behaviour tests** | The live API is observably unchanged | §7.2 differential harness | 28/28 probes match; CRUD transcript byte-identical |
| **§7.3 Coverage tests** | No silent loss of exercised code | `cargo llvm-cov … -p backend` | per-file floors in §7.3.3 met |

**All three MUST pass. A migration that clears two of the three is not complete.**

Preconditions (gates on the build, not on behaviour):

```bash
git checkout 33ab114
git apply --check golden.patch
git apply        golden.patch
cargo build --workspace                 # regenerates Cargo.lock
cargo fmt --all --check                 # repo's CI gate, mandatory

# feature matrix — CI's real enforcement; a precondition, not one of the three gates
just lint-sqlite                        # clippy --all-targets -- -D warnings
just lint-postgres                      # needs a live Postgres — see the warning below
cargo build --workspace --release --no-default-features --features "db-sqlite,svelte-ui"
cargo check -p backend --no-default-features --features "db-sqlite,slint-ui"
```

> **Environment warning — the Postgres legs need a live database.** The checked-in `.sqlx/` offline
> cache was generated against SQLite. With `SQLX_OFFLINE=true` and `--features db-postgres`, the
> `sqlx::query!` macros fail with ~14 `E0271` type-mismatch errors. **This is not a migration
> defect** — the pre-patch tree fails identically under the same conditions (verified). CI avoids it
> by running `just db-migrate-postgres` against a live `postgres:15` service and regenerating the
> cache. A verifier without Postgres MUST run only the SQLite legs and MUST NOT record the Postgres
> failure against the candidate. The `slint-ui` leg is a `cargo check` rather than a full build
> because the Slint frontend emits unrelated pre-existing warnings about non-`Window` components.

---

### 7.1 Gate 1 — Test cases

`backend/tests/contact_api.rs`, **8 integration tests**, each spawning a real server via
`helpers::spawn_app` and driving it over `reqwest`:

| # | Test | Covers |
|---:|---|---|
| 1 | `test_register_login_logout_flow` | 201/409 register, login, 204 logout |
| 2 | `test_token_refresh` | refresh rotation, old-token rejection |
| 3 | `test_contacts_crud_flow` | 201/200/200/204 + 404 after delete |
| 4 | `test_contacts_authorization` | cross-user access → 404 |
| 5 | `test_protected_routes_require_auth` | 401 on all protected routes |
| 6 | `test_invalid_and_expired_tokens` | malformed / expired / wrong-signature |
| 7 | `test_validation_errors` | 422 + `details` payload |
| 8 | `test_contacts_pagination` | `?page=&per_page=` |

```bash
just test-backend-sqlite     # cargo test -p backend --no-default-features --features "db-sqlite,svelte-ui"
```

**Pass condition:** `test result: ok. 8 passed; 0 failed; 0 ignored`.

> These 8 tests pass against a **wrong** migration in several of the ways that matter. None asserts a
> rate limit, a body-size rejection, static-file behaviour, `/docs`, CORS, or routing precedence
> against the SPA fallback. Gate 1 is necessary and badly insufficient — hence §7.2.

---

### 7.2 Gate 2 — Behaviour tests (differential)

The distinguishing gate for this task. Build the **pre-patch** tree and the **candidate** tree, run
both concurrently, and diff their responses. Framework migrations are graded on sameness, so the
Axum binary is the oracle.

#### 7.2.1 Harness

```bash
# oracle: pre-patch tree
git worktree add /tmp/oracle 33ab114 && cd /tmp/oracle && cargo build -p backend \
  --no-default-features --features "db-sqlite,svelte-ui"

# both need a cwd containing Config.toml and backend/static/svelte-build/{index.html,assets/,.hidden}
export APP_JWT__SECRET="a-test-secret-that-is-long-enough-32b" SQLX_OFFLINE=true
export APP_RATELIMIT__BURST_SIZE=100000          # lift the limiter for probes 1–21
DATABASE_URL="sqlite:new.db?mode=rwc"  APP_WEB__PORT=18091 ./candidate/backend &
DATABASE_URL="sqlite:orig.db?mode=rwc" APP_WEB__PORT=18092 ./oracle/backend &
```

Plant a `secret.txt` **outside** the static root for probes 25–27.

#### 7.2.2 Probes — all 28 MUST match the oracle

Recorded values are from the verified reference run.

| # | Probe | Expected | Guards |
|---:|---|---|---|
| 1 | `GET /api/v1/health` | 200 | §1.4 — API not shadowed by `Files` |
| 2 | `GET /api/v1/contacts` no token | 401 | §1.4 — protected route reachable |
| 3 | `GET /api/v1/nonexistent` | 404 | §1.4 — unknown API path falls to SPA |
| 4 | `GET /deep/spa/route` | **404** + `text/html` + index body | §2.6 — **not 200** |
| 5 | `GET /docs` | **303** | §2.1 — redirect present |
| 6 | `GET /docs/` | 200 | §2.1 |
| 7 | `GET /api-docs/openapi.json` | 200 | §2.1 |
| 8 | `POST /api/v1/health` | 405 | method matching intact |
| 9 | `GET /contacts` `Authorization: garbage` | **400** | §3.1 — malformed ≠ absent |
| 10 | `GET /contacts` `Authorization: Basic abc` | **400** | §3.1 |
| 11 | `GET /contacts` `Authorization: Bearer nope` | **401** | §3.1 |
| 12 | `PATCH /api/v1/contacts` no token | **401** (not 405) | §3.1 — wrap precedes method match |
| 13 | `POST /login` no `Content-Type` | **415** | §2.5 |
| 14 | `POST /login` `{bad` | **400** | §2.5 — syntax |
| 15 | `POST /login` `{"a":1}` | **422** | §2.5 — `Category::Data` |
| 16 | `POST /login` `{"email":123,…}` | **422** | §2.5 — type mismatch is Data |
| 17 | `POST /login` 2.1 MB body | **413** | §2.5 — `OverflowKnownLength` |
| 18 | `POST /login` 3 MB **chunked** body | **413** | §2.5 — `Overflow`, the missed variant |
| 19 | `POST /login` 2.0 MB body | 401 (accepted) | §2.5 — limit boundary unmoved |
| 20 | `GET /contacts/abc` + token | **400** | §2.5 — `PathConfig` |
| 21 | `GET /api/v1/health` response headers | **no** `x-request-id` | §2.2 — request-side only |
| 22 | 6× `GET /health`, `burst_size=3` | `200 200 200 429 429 429` | §2.7 — period + burst honoured |
| 23 | `health`, `register`, `refresh` after burst | `429 429 429` | §2.7 — one shared bucket |
| 24 | 8× `GET /contacts` no token | `401` ×8, never 429 | §2.7 — protected routes exempt |
| 25 | `GET /../secret.txt` | 404, no leak | §2.6 |
| 26 | `GET /assets/../../secret.txt` | 404, no leak | §2.6 |
| 27 | `GET /.hidden` | **200** + body | §2.6 — `use_hidden_files()` |
| 28 | `SIGTERM` | exits cleanly, logs graceful-shutdown line | §3.3 |

```bash
# probe 4 — the SPA-status trap
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' localhost:18091/deep/spa/route   # 404 text/html
# probe 18 — the overflow-variant trap
python3 -c "print('{\"a\":\"'+'x'*3000000+'\"}')" | \
  curl -s -o /dev/null -w '%{http_code}\n' -X POST -H 'Content-Type: application/json' \
       -H 'Transfer-Encoding: chunked' --data-binary @- localhost:18091/api/v1/login      # 413
# probe 22 — shared-bucket / period check (restart with APP_RATELIMIT__BURST_SIZE=3)
for i in $(seq 6); do curl -s -o /dev/null -w '%{http_code} ' localhost:18091/api/v1/health; done
```

**Highest-value probes:** 4 (SPA status), 18 (chunked overflow), 22–23 (rate-limit period and bucket
sharing), 9–11 (the 400/401 header split). Each is reachable only here — none of §7.1 or §7.3 touches
them, and each corresponds to a mistake that compiles and serves traffic.

#### 7.2.3 Full-transcript equality

Beyond the probe table, two whole-surface comparisons MUST come back identical:

```bash
# 1. OpenAPI spec — byte-identical
diff <(curl -s localhost:18091/api-docs/openapi.json | python3 -m json.tool --sort-keys) \
     <(curl -s localhost:18092/api-docs/openapi.json | python3 -m json.tool --sort-keys)

# 2. Full CRUD/auth transcript — register, dup-register, login, wrong-password, create,
#    get, get-404, list, paginate, bad page, bad path, update, update-404, invalid-create
#    (status + `details`), refresh, refresh-reuse, delete, delete-again, logout,
#    refresh-after-logout.  Capture from both, diff.
```

**Pass condition:** the OpenAPI diff is empty, and the transcript diff is empty modulo §5.3's
`error`-string field ordering. Both were empty in the reference run.

**§4.1 is the sole permitted probe difference** (`OPTIONS` from a disallowed origin, 400 vs 200).

---

### 7.3 Gate 3 — Coverage tests

#### 7.3.1 How coverage is produced

The repo ships **no** coverage tooling — no tarpaulin, no llvm-cov, no threshold in the `justfile`
or CI. The floors below are therefore **task-defined**, not inherited, and the tool is a verifier
prerequisite rather than a candidate deliverable. **A candidate MUST NOT be required to add coverage
tooling to the repo, and MUST NOT be credited for doing so** — the golden patch adds none.

What the repo *does* have is the test suite of §7.1, which is all a coverage run needs:

```bash
cargo install cargo-llvm-cov --locked        # verifier-side; llvm-tools component required
export SQLX_OFFLINE=true
cargo llvm-cov --no-default-features --features "db-sqlite,svelte-ui" -p backend --summary-only
cargo llvm-cov --no-default-features --features "db-sqlite,svelte-ui" -p backend \
  --text --output-path cov.txt               # per-line, to locate a regression
```

Measured against the repo's own 8 tests — **no verifier-authored tests are added, and a candidate
MUST NOT add tests to raise these numbers** (see §7.3.5).

#### 7.3.2 Measured reference values

Region coverage, `-p backend`, `db-sqlite,svelte-ui`, driven by the 8 tests of §7.1. Both columns
were measured, not estimated:

| File | Pre-migration (`33ab114`) | Reference post-migration | Δ |
|---|---:|---:|---:|
| `web_server.rs` | 87.95% | **83.08%** | **−4.87** |
| `auth.rs` | 83.26% | **82.88%** | −0.38 |
| `extractors.rs` | 66.67% | **72.73%** | +6.06 |
| `error.rs` | 52.27% | **53.70%** | +1.43 |
| `config.rs` | 0.00% | **0.00%** | 0 |
| `main.rs` | 0.00% | **0.00%** | 0 |
| **TOTAL (regions)** | 66.83% | **72.92%** | **+6.09** |
| **TOTAL (lines)** | 68.28% | **73.68%** | +5.40 |

> **Warning — a naive per-file "no regression" rule fails the golden patch.**
> `web_server.rs` legitimately drops **4.87 points**. It is the file that gained the most new logic
> (+246 / −133), and the new logic — the §2.5 rejection arms and the §2.6 SPA fallback — is precisely
> what the 8 tests never reach. Use the absolute floors in §7.3.3.
>
> The **total** rising by 6.09 points is largely an artefact, not an achievement: `main.rs` shrinks
> from 84 measured regions to 34 (Axum's `axum::serve(...).with_graceful_shutdown(...)` expanded to
> far more regions than Actix's `HttpServer` builder), and since `main.rs` is 0% in both trees,
> removing dead-weight regions from the denominator lifts the total. A verifier MUST NOT read the
> +6.09 as evidence the migration improved test quality.

#### 7.3.3 Floors

| File | Required floor (regions) | Rationale |
|---|---:|---|
| `web_server.rs` | ≥ **78%** | reference 83.08%, ~5 pt margin |
| `auth.rs` | ≥ **78%** | reference 82.88% |
| `extractors.rs` | ≥ **65%** | reference 72.73% |
| `error.rs` | ≥ **48%** | reference 53.70%; see §7.3.4 |
| `config.rs`, `main.rs` | **no floor** | structurally unreachable, see §7.3.4 |
| **TOTAL** | ≥ **68%** regions | reference 72.92% |

**Pass condition:** every floor met, measured across `-p backend` with the SQLite feature set.

#### 7.3.4 What coverage cannot certify here — and why the other gates exist

Three structural facts a verifier MUST account for before reading these numbers:

- **`main.rs` is 0% in both trees and MUST NOT be gated on.** `helpers::spawn_app` builds the app by
  calling `create_app` directly; it never goes through `main`. So the entire §3.3 bootstrap contract
  — `disable_signals()`, `ServerHandle::stop(true)`, the shutdown task — is **structurally
  uncoverable by this suite**. That is exactly why §7.2 probe 28 exists, and why this gate cannot
  substitute for it.
- **`config.rs` is 0% in both trees.** `AppConfig::from_env` is bypassed by the test harness, which
  constructs `AppConfig` literally. Pre-existing; not a migration defect.
- **Coverage counts execution, not correctness.** `error.rs` at 53.70% is not a migration
  regression — it is 52.27% before. Every arm it misses is still asserted end-to-end by §7.2.

#### 7.3.5 The findings this gate actually delivers

These are measured, and they are the argument for the gate's existence — each is a place where a
wrong candidate passes §7.1 clean:

**(a) Three of the four §2.5 rejection arms are never executed.** From the reference `cov.txt`:

```
181|     80|fn create_json_config() -> web::JsonConfig {
183|      1|        let status = match &err {
187|      1|                StatusCode::UNPROCESSABLE_ENTITY        <- 422, the ONLY covered arm
189|      0|            JsonPayloadError::ContentType => …          <- 415, never executed
191|      0|                StatusCode::PAYLOAD_TOO_LARGE           <- 413, never executed
193|      0|            _ => StatusCode::BAD_REQUEST,               <- 400, never executed
```

A candidate could return **500 from all three** and pass all 8 tests. Only §7.2 probes 13–18 catch
it. This is the single strongest justification for running §7.2 and §7.3 together.

**(b) `ResponseError`'s two methods are exercised on disjoint paths.** `status_code()` runs 16 times
but **only** through its `UNAUTHORIZED` arm — actix calls it on the middleware error path (§3.1's
401s) — while `error_response()` runs 19 times and covers the handler path. The
`Conflict` / `NotFound` / `ValidationError` / `InternalServerError` arms of `status_code()` are
**never executed**, even though all four statuses are returned by the API via `error_response()`.

Consequence a verifier MUST understand: **the two methods can disagree and no test in §7.1 will
notice.** A candidate whose `status_code()` returns 500 for `NotFound` while `error_response()`
returns 404 is green on Gate 1. §2.4 requires them to agree; this is the measurement that shows the
requirement is not self-enforcing.

**(c) The entire §2.6 SPA fallback is uncovered** — `web_server.rs` lines 112–127, all zero. The 8
tests never request a static path, so the 404-not-200 status, the `NotFound` arm and the 500 arm are
verified **only** by §7.2 probes 4 and 25–27.

**(d) `health_check` is never called** (lines 314–316, zero). No test in §7.1 hits `/api/v1/health`,
despite it being the one route with no auth and no body. §7.2 probe 1 is its only coverage.

**Adding tests for (a)–(d) raises coverage above the reference and MUST NOT be penalised** — but it
also MUST NOT be *required*, because the golden patch does not do it. A candidate that ships the
reference's coverage profile is complete; one that improves it is complete and better.

---

## 8. Scoring rubric

Applied only once all three gates in §7 are green. A gate failure is not a deduction — it is an
incomplete task.

| Weight | Obligation | Fail condition |
|---:|---|---|
| 20% | §1.4 static service registered last; `.wrap()` order correct | probes 1–4 fail, or `request_id` absent from spans |
| 15% | §2.5 Json rejection mapping (422/415/413/400) | probes 13–18 fail |
| 15% | §2.7 governor config built **once** and shared across workers | probes 22–23 fail (per-worker buckets multiply the limit by core count) |
| 10% | §2.6 SPA fallback returns **404**, dotfiles served | probes 4, 27 fail |
| 10% | §3.1 absent-vs-malformed `Authorization` split | probes 9–11 fail |
| 10% | §2.4 `ResponseError` status + body parity | any status or `details` payload differs |
| 5% | §3.3 `disable_signals()` + `stop(true)` | probe 28 fails or shutdown hangs |
| 5% | §2.1 `/docs` → `/docs/` redirect | probe 5 returns 404 |
| 5% | §1.2 `common/Cargo.toml` utoipa feature updated | `axum_extras` still resolves |
| 5% | §6 docs updated, no residual `axum`/`tower`/`hyper` | the §6 grep returns hits |

Deviations in §4 are worth 0. Fixes to §5 items are worth 0 and MUST NOT be penalised.

---

## 9. Reference environment

Rust 1.98.0 (edition **2024**) · `cargo 1.98.0` ·
`axum` 0.8.6 + `axum-extra` 0.10.3 + `tower-http` 0.6.6 + `tower_governor` 0.8.0 (source) →
`actix-web` 4.14.1 + `actix-cors` 0.7.1 + `actix-files` 0.7.0 + `actix-governor` 0.10.0 +
`actix-web-httpauth` 0.8.2 + `tracing-actix-web` 0.7.22 (target) ·
`utoipa` 5.4.0 + `utoipa-swagger-ui` 9.0.2 · `sqlx` 0.8.6 · `tokio` 1.48.0 ·
`jsonwebtoken` 10.1.0 · `bcrypt` 0.17.1 · `validator` (workspace) ·
SQLite via `.sqlx` offline cache; PostgreSQL 15 service for the second CI matrix leg ·
`reqwest` for the integration suite · `just` as the task runner
