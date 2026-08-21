use std::{
    sync::{Arc, Once},
    time::Duration,
};

use actix_web::{
    App, Error, HttpResponse,
    body::MessageBody,
    dev::{ServiceFactory, ServiceRequest, ServiceResponse, fn_service},
    error::{ErrorBadRequest, InternalError, JsonPayloadError},
    http::{
        Method, StatusCode,
        header::{self, ContentType, HeaderName, HeaderValue},
    },
    middleware::{Next, from_fn},
    web,
};

use crate::db::DbPool;
use serde::Deserialize;
use std::io;

use actix_cors::Cors;
use actix_files::{Files, NamedFile};
use tracing_actix_web::TracingLogger;

use tracing;
use validator::Validate;

use crate::error::AppError;
use crate::extractors::AuthUser;
use crate::{auth, config::AppConfig};
use common::ContactDto;

use actix_governor::{
    Governor, GovernorConfig, GovernorConfigBuilder, PeerIpKeyExtractor,
    governor::middleware::NoOpMiddleware,
};

use common::Credentials;
use common::LoginResponse;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register,
        auth::login,
        auth::refresh,
        auth::logout,
        get_contacts,
        create_contact,
        get_contact,
        update_contact,
        delete_contact
    ),
    // 👇 All components are now in a single block
    components(
        schemas(ContactDto, Credentials, LoginResponse),
    ),
    tags(
        (name = "Cornerstone API", description = "Full-stack Rust template API")
    ),
    // This part remains the same, it *applies* the security scheme to the UI
    security(
        ("bearer_auth" = [])
    )
)]
struct ApiDoc;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DbPool,
    pub app_config: AppConfig,
}

/// The rate limiter configuration shared by every worker of the HTTP server.
pub type AppGovernorConfig = GovernorConfig<PeerIpKeyExtractor, NoOpMiddleware>;

// This will cause a compilation error if neither `svelte-ui` nor `slint-ui` feature is enabled.
#[cfg(not(any(feature = "svelte-ui", feature = "slint-ui")))]
compile_error!("You must enable either the 'svelte-ui' or 'slint-ui' feature.");

// This code block will only be included if the `svelte-ui` feature is enabled
#[cfg(feature = "svelte-ui")]
const STATIC_DIR: &str = "backend/static/svelte-build";
#[cfg(feature = "svelte-ui")]
const STATIC_INDEX: &str = "backend/static/svelte-build/index.html";
#[cfg(feature = "svelte-ui")]
const STATIC_ERROR: &str = "Failed to serve Svelte app";

// This code block will only be included if the `slint-ui` feature is enabled
#[cfg(feature = "slint-ui")]
const STATIC_DIR: &str = "backend/static/slint-build";
#[cfg(feature = "slint-ui")]
const STATIC_INDEX: &str = "backend/static/slint-build/index.html";
#[cfg(feature = "slint-ui")]
const STATIC_ERROR: &str = "Failed to serve Slint app";

fn create_static_service() -> Files {
    Files::new("/", STATIC_DIR)
        .index_file("index.html")
        // Keep the mime type of text files as guessed, without a charset suffix
        .prefer_utf8(false)
        // Everything in the build directory is public, including dotted paths
        // such as `.well-known/`
        .use_hidden_files()
        // Anything that is not an existing file falls back to the SPA entry point,
        // which is served with a `404 Not Found` status.
        .default_handler(fn_service(|req: ServiceRequest| async move {
            let (req, _) = req.into_parts();
            let res = match NamedFile::open(STATIC_INDEX) {
                Ok(file) => {
                    let mut res = file.prefer_utf8(false).into_response(&req);
                    *res.status_mut() = StatusCode::NOT_FOUND;
                    res
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    HttpResponse::NotFound().finish()
                }
                Err(error) => {
                    HttpResponse::InternalServerError().body(format!("{STATIC_ERROR}: {error}"))
                }
            };
            Ok::<_, Error>(ServiceResponse::new(req, res))
        }))
}

/// Builds the rate limiter configuration and starts the background task that keeps
/// its storage from growing indefinitely. It must be called once and the returned
/// configuration shared with every worker.
pub fn create_governor_config(app_config: &AppConfig) -> Arc<AppGovernorConfig> {
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .seconds_per_request(app_config.ratelimit.per_second)
            .burst_size(app_config.ratelimit.burst_size)
            .finish()
            .unwrap(),
    );

    // This avoids the storage size growing indefinitely
    let governor_limiter = governor_conf.limiter().clone();
    let interval = Duration::from_secs(60);
    // a separate background task to clean up
    tokio::spawn(async move {
        let mut interval_timer = tokio::time::interval(interval);
        loop {
            interval_timer.tick().await;
            tracing::debug!(
                "Cleaning up rate limit storage. Current size: {}",
                governor_limiter.len()
            );
            governor_limiter.retain_recent();
        }
    });

    governor_conf
}

/// Adds an `x-request-id` header (a random UUID) to every incoming request that does
/// not already carry one, so that it shows up in the traces produced below.
async fn set_request_id<B>(
    mut req: ServiceRequest,
    next: Next<B>,
) -> Result<ServiceResponse<B>, Error> {
    let request_id_header = HeaderName::from_static("x-request-id");

    if !req.headers().contains_key(&request_id_header)
        && let Ok(request_id) = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
    {
        req.headers_mut().insert(request_id_header, request_id);
    }

    next.call(req).await
}

/// Rejections coming from the `Json` extractor: `422` when the body is valid JSON
/// but does not match the target type, `415` when the content type is wrong, `413`
/// when the body is over the size limit and `400` for everything else.
fn create_json_config() -> web::JsonConfig {
    web::JsonConfig::default().error_handler(|err, _req| {
        let status = match &err {
            JsonPayloadError::Deserialize(json_error)
                if json_error.classify() == serde_json::error::Category::Data =>
            {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            JsonPayloadError::ContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            JsonPayloadError::OverflowKnownLength { .. } | JsonPayloadError::Overflow { .. } => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            _ => StatusCode::BAD_REQUEST,
        };

        let message = err.to_string();
        InternalError::from_response(
            err,
            HttpResponse::build(status)
                .content_type(ContentType::plaintext())
                .body(message),
        )
        .into()
    })
}

/// A path segment that cannot be deserialized is a malformed request, not a missing
/// resource, so it is reported as `400 Bad Request`.
fn create_path_config() -> web::PathConfig {
    web::PathConfig::default().error_handler(|err, _req| ErrorBadRequest(err))
}

pub fn create_app(
    app_state: AppState,
    governor_conf: Arc<AppGovernorConfig>,
) -> App<
    impl ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<impl MessageBody>,
        Error = Error,
        InitError = (),
    >,
> {
    let cors = Cors::default()
        .allowed_origin(&app_state.app_config.web.cors_origin)
        // It's good practice to be specific about allowed methods and headers
        .allowed_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allowed_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        // This is required to allow the browser to send credentials (e.g., cookies, auth tokens)
        .supports_credentials();

    let mut app = App::new()
        .app_data(web::Data::new(app_state))
        .app_data(create_json_config())
        .app_data(create_path_config());

    if cfg!(debug_assertions) {
        // `create_app` is the per-worker factory, this notice is emitted once
        static DOCS_NOTICE: Once = Once::new();
        DOCS_NOTICE.call_once(|| tracing::info!("Debug mode: Enabling /docs endpoint"));

        app = app
            // Keeps `/docs` reachable, the UI itself is mounted under `/docs/`
            .service(web::redirect("/docs", "/docs/").see_other())
            .service(
                SwaggerUi::new("/docs/{_:.*}").url("/api-docs/openapi.json", ApiDoc::openapi()),
            );
    }

    app
        // Public routes that do not require authentication.
        // The rate-limiting middleware is applied to each of them.
        .service(
            web::resource("/api/v1/health")
                .wrap(Governor::new(&governor_conf))
                .route(web::get().to(health_check)),
        )
        .service(
            web::resource("/api/v1/register")
                .wrap(Governor::new(&governor_conf))
                .route(web::post().to(auth::register)),
        )
        .service(
            web::resource("/api/v1/login")
                .wrap(Governor::new(&governor_conf))
                .route(web::post().to(auth::login)),
        )
        .service(
            web::resource("/api/v1/refresh")
                .wrap(Governor::new(&governor_conf))
                .route(web::post().to(auth::refresh)),
        )
        // Protected routes that require authentication
        .service(
            web::resource("/api/v1/logout")
                .wrap(from_fn(auth::auth_middleware))
                .route(web::post().to(auth::logout)),
        )
        .service(
            web::resource("/api/v1/contacts")
                .wrap(from_fn(auth::auth_middleware))
                .route(web::get().to(get_contacts))
                .route(web::post().to(create_contact)),
        )
        .service(
            web::resource("/api/v1/contacts/{id}")
                .wrap(from_fn(auth::auth_middleware))
                .route(web::get().to(get_contact))
                .route(web::put().to(update_contact))
                .route(web::delete().to(delete_contact)),
        )
        // Registered last so that it only handles what the routes above did not match
        .service(create_static_service())
        .wrap(TracingLogger::default())
        .wrap(from_fn(set_request_id)) // This line adds the request ID
        .wrap(cors)
}
// --- API Handlers ---

#[utoipa::path(
    get,
    path = "/api/v1/health",
    responses(
        (status = 200, description = "Service is healthy")
    )
)]
async fn health_check() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[utoipa::path(
    post,
    path = "/api/v1/contacts",
    request_body = ContactDto,
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 201, description = "Contact created successfully", body = ContactDto),
        (status = 401, description = "Authentication required"),
        (status = 422, description = "Validation error"),
    )
)]
async fn create_contact(
    state: web::Data<AppState>,
    user: AuthUser,
    new_contact_dto: web::Json<ContactDto>,
) -> Result<HttpResponse, AppError> {
    let new_contact_dto = new_contact_dto.into_inner();

    tracing::info!(
        "Creating contact: {:?}, assigned to user {}",
        new_contact_dto,
        user.id
    );

    // Validate the new contact DTO
    new_contact_dto.validate()?;

    let result = sqlx::query_as!(
        ContactDto,
        r#"
        INSERT INTO contacts (user_id, name, email, age, subscribed, contact_type)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, name, email, age, subscribed, contact_type;
        "#,
        user.id, // Add the user_id here
        new_contact_dto.name,
        new_contact_dto.email,
        new_contact_dto.age,
        new_contact_dto.subscribed,
        new_contact_dto.contact_type
    )
    .fetch_one(&state.db_pool)
    .await;

    match result {
        Ok(created_contact) => Ok(HttpResponse::Created().json(created_contact)),
        Err(e) => {
            tracing::error!("Failed to create contact: {}", e);
            Err(AppError::InternalServerError(
                "Failed to create contact".to_string(),
            ))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/contacts/{id}",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("id" = i64, Path, description = "Contact ID")
    ),
    responses(
        (status = 200, body = ContactDto),
        (status = 404, description = "Contact not found"),
        (status = 401, description = "Authentication required")
    )
)]
async fn get_contact(
    state: web::Data<AppState>,
    id: web::Path<i64>,
    user: AuthUser,
) -> Result<web::Json<ContactDto>, AppError> {
    let id = id.into_inner();

    tracing::info!(
        "Fetching single contact with id: {} for user {}",
        id,
        user.id
    );

    let result = sqlx::query_as!(
        ContactDto,
        "SELECT id, name, email, age, subscribed, contact_type FROM contacts WHERE id = $1 AND user_id = $2",
        id,
        user.id
    )
    .fetch_optional(&state.db_pool)
    .await;

    match result {
        Ok(Some(contact)) => Ok(web::Json(contact)),
        Ok(None) => Err(AppError::NotFound),
        Err(e) => {
            tracing::error!("Failed to fetch contact: {}", e);
            Err(AppError::InternalServerError(
                "Failed to fetch contact".to_string(),
            ))
        }
    }
}

#[derive(Deserialize)]
pub struct Pagination {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/contacts",
    security(
        ("bearer_auth" = [])
    ),
    responses(
        (status = 200, description = "List of contacts", body = Vec<ContactDto>),
        (status = 401, description = "Authentication required")
    )
)]
async fn get_contacts(
    state: web::Data<AppState>,
    user: AuthUser,
    pagination: web::Query<Pagination>, // <-- Add this
) -> Result<web::Json<Vec<ContactDto>>, AppError> {
    let pagination = pagination.into_inner();

    // Set default values for pagination
    let page = pagination.page.unwrap_or(1) as i64;
    let per_page = pagination.per_page.unwrap_or(20) as i64;
    let offset = (page - 1) * per_page;

    tracing::info!(
        "Fetching contacts for user {}, page: {}, per_page: {}",
        user.id,
        page,
        per_page
    );

    let result = sqlx::query_as!(
        ContactDto,
        "SELECT id, name, email, age, subscribed, contact_type
         FROM contacts
         WHERE user_id = $1
         LIMIT $2 OFFSET $3",
        user.id,
        per_page,
        offset
    )
    .fetch_all(&state.db_pool)
    .await;

    // ... rest of the handler remains the same
    match result {
        Ok(contacts) => Ok(web::Json(contacts)),
        Err(e) => {
            tracing::error!("Failed to fetch contacts: {}", e);
            Err(AppError::InternalServerError(
                "Failed to fetch contacts".to_string(),
            ))
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/contacts/{id}",
    request_body = ContactDto,
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("id" = i64, Path, description = "Contact ID")
    ),
    responses(
        (status = 200, description = "Contact updated successfully", body = ContactDto),
        (status = 404, description = "Contact not found"),
        (status = 401, description = "Authentication required"),
        (status = 422, description = "Validation error"),
    )
)]
async fn update_contact(
    state: web::Data<AppState>,
    id: web::Path<i64>,
    user: AuthUser,
    updated_contact: web::Json<ContactDto>,
) -> Result<web::Json<ContactDto>, AppError> {
    let id = id.into_inner();
    let updated_contact = updated_contact.into_inner();

    tracing::info!("Updating contact with id: {} for user {}", id, user.id);

    updated_contact.validate()?;

    let result = sqlx::query_as!(
        ContactDto,
        r#"
        UPDATE contacts
        SET name = $1, email = $2, age = $3, subscribed = $4, contact_type = $5
        WHERE id = $6 AND user_id = $7
        RETURNING id, name, email, age, subscribed, contact_type
        "#,
        updated_contact.name,
        updated_contact.email,
        updated_contact.age,
        updated_contact.subscribed,
        updated_contact.contact_type,
        id,
        user.id
    )
    .fetch_optional(&state.db_pool)
    .await;

    match result {
        Ok(Some(contact)) => Ok(web::Json(contact)),
        Ok(None) => Err(AppError::NotFound),
        Err(e) => {
            tracing::error!("Failed to update contact: {}", e);
            Err(AppError::InternalServerError(
                "Failed to update contact".to_string(),
            ))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/contacts/{id}",
    security(
        ("bearer_auth" = [])
    ),
    params(
        ("id" = i64, Path, description = "Contact ID")
    ),
    responses(
        (status = 204, description = "Contact deleted successfully"),
        (status = 404, description = "Contact not found"),
    )
)]
async fn delete_contact(
    state: web::Data<AppState>,
    id: web::Path<i64>,
    user: AuthUser,
) -> Result<HttpResponse, AppError> {
    let id = id.into_inner();

    tracing::info!("Deleting contact with id: {} for user {}", id, user.id);

    let result = sqlx::query("DELETE FROM contacts WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.db_pool)
        .await;

    match result {
        Ok(execution_result) => {
            if execution_result.rows_affected() > 0 {
                Ok(HttpResponse::NoContent().finish())
            } else {
                // Use NotFound to prevent leaking information about which contacts exist
                Err(AppError::NotFound)
            }
        }
        Err(e) => {
            tracing::error!("Failed to delete contact: {}", e);
            Err(AppError::InternalServerError(
                "Failed to delete contact".to_string(),
            ))
        }
    }
}
