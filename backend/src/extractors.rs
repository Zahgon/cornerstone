use crate::error::AppError;
use actix_web::{FromRequest, HttpMessage, HttpRequest, dev::Payload};
use std::future::{Ready, ready};

// The struct is the same
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: i64,
    pub email: String,
}

// But the extractor logic changes completely
impl FromRequest for AuthUser {
    type Error = AppError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        // The middleware is responsible for putting AuthUser in extensions.
        // If it's not there, it's a 500 Internal Server Error because
        // the middleware should have been run.
        let user = req.extensions().get::<AuthUser>().cloned();

        ready(user.ok_or_else(|| {
            AppError::InternalServerError(
                "AuthUser not found in request extensions. Is the auth middleware missing?".into(),
            )
        }))
    }
}
