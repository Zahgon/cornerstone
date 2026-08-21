use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde_json::json;
use thiserror::Error;
use validator::ValidationErrors;

// Define a custom error type
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Internal Server Error: {0}")]
    InternalServerError(String),

    #[error("Database error")]
    DatabaseError(sqlx::Error),

    #[error("Authentication error")]
    JwtError(jsonwebtoken::errors::Error),

    #[error("Authentication error")]
    PasswordError(bcrypt::BcryptError),

    #[error("{0}")]
    Conflict(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Resource not found")]
    NotFound,

    #[error("Validation error: {0}")]
    ValidationError(ValidationErrors),
}

// Implement ResponseError to convert AppError into an HTTP response
impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::InternalServerError(_) | AppError::DatabaseError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AppError::JwtError(_) | AppError::PasswordError(_) | AppError::Unauthorized => {
                StatusCode::UNAUTHORIZED
            }
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::ValidationError(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let (status, error_message) = match self {
            AppError::InternalServerError(msg) => {
                tracing::error!("Internal server error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
            AppError::DatabaseError(e) => {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            AppError::JwtError(e) => {
                tracing::warn!("JWT error: {}", e);
                (StatusCode::UNAUTHORIZED, "Invalid token".to_string())
            }
            AppError::PasswordError(e) => {
                tracing::warn!("Password error: {}", e);
                (StatusCode::UNAUTHORIZED, "Invalid password".to_string())
            }
            // ... other error mappings ...
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()),
            AppError::NotFound => (StatusCode::NOT_FOUND, "Resource not found".to_string()),
            AppError::ValidationError(errors) => {
                // The `errors` object contains detailed information on which fields failed.
                // We can serialize this to JSON for a rich client-side error message.
                let message = format!("Input validation failed: {errors}").replace('\n', ", ");
                return HttpResponse::build(StatusCode::UNPROCESSABLE_ENTITY)
                    .json(json!({ "error": message, "details": errors }));
            } // Handle other variants...
        };

        HttpResponse::build(status).json(json!({ "error": error_message }))
    }
}

// Add From implementations for easy '?' conversion in handlers
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::DatabaseError(e)
    }
}

impl From<ValidationErrors> for AppError {
    fn from(errors: ValidationErrors) -> Self {
        AppError::ValidationError(errors)
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(e: jsonwebtoken::errors::Error) -> Self {
        AppError::JwtError(e)
    }
}
impl From<bcrypt::BcryptError> for AppError {
    fn from(e: bcrypt::BcryptError) -> Self {
        AppError::PasswordError(e)
    }
}

// Add From for other error types...
