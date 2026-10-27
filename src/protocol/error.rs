#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid {protocol} request: {message}")]
    InvalidRequest {
        protocol: &'static str,
        message: String,
    },
    #[error("invalid {protocol} response: {message}")]
    InvalidResponse {
        protocol: &'static str,
        message: String,
    },
    #[error("field `{field}` cannot be represented in {target}")]
    UnsupportedField { field: String, target: &'static str },
    #[error("invalid field `{field}`: {message}")]
    InvalidField { field: String, message: String },
}

impl ProtocolError {
    pub fn invalid_request(protocol: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            protocol,
            message: message.into(),
        }
    }

    pub fn invalid_response(protocol: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidResponse {
            protocol,
            message: message.into(),
        }
    }

    pub fn unsupported(field: impl Into<String>, target: &'static str) -> Self {
        Self::UnsupportedField {
            field: field.into(),
            target,
        }
    }

    pub fn invalid_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl From<ProtocolError> for crate::error::AppError {
    fn from(error: ProtocolError) -> Self {
        crate::error::AppError::BadRequest(error.to_string())
    }
}
