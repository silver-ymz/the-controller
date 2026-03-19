use std::fmt;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Forbidden(String),
    NotFound(String),
    Internal(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(msg)
            | Self::Forbidden(msg)
            | Self::NotFound(msg)
            | Self::Internal(msg) => f.write_str(msg),
        }
    }
}

// For Tauri commands: AppError → String
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}

#[cfg(feature = "server")]
impl From<AppError> for (axum::http::StatusCode, String) {
    fn from(e: AppError) -> (axum::http::StatusCode, String) {
        use axum::http::StatusCode;
        match e {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        }
    }
}

impl AppError {
    /// Wrap a mutex poisoning or I/O error as Internal.
    pub fn internal(e: impl fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_request_converts_to_string() {
        let err = AppError::BadRequest("x".into());
        let s: String = err.into();
        assert_eq!(s, "x");
    }

    #[test]
    fn not_found_converts_to_string() {
        let err = AppError::NotFound("x".into());
        let s: String = err.into();
        assert_eq!(s, "x");
    }

    #[test]
    fn internal_converts_to_string() {
        let err = AppError::Internal("x".into());
        let s: String = err.into();
        assert_eq!(s, "x");
    }

    #[test]
    fn internal_convenience_wraps_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err = AppError::internal(io_err);
        let s: String = err.into();
        assert_eq!(s, "gone");
    }

    #[cfg(feature = "server")]
    #[test]
    fn not_found_converts_to_status_code() {
        let err = AppError::NotFound("x".into());
        let (status, body): (axum::http::StatusCode, String) = err.into();
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(body, "x");
    }

    #[cfg(feature = "server")]
    #[test]
    fn internal_converts_to_status_code() {
        let err = AppError::Internal("x".into());
        let (status, body): (axum::http::StatusCode, String) = err.into();
        assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "x");
    }

    #[cfg(feature = "server")]
    #[test]
    fn bad_request_converts_to_status_code() {
        let err = AppError::BadRequest("x".into());
        let (status, body): (axum::http::StatusCode, String) = err.into();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(body, "x");
    }
}
