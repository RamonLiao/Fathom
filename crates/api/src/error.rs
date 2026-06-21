use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub struct ApiError(pub anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(error = ?self.0, "request failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}
