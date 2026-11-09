use serde::{Deserialize, Serialize};

pub const MIN_PASSWORD_LENGTH: usize = 10;
pub const MAX_PASSWORD_LENGTH: usize = 1024;
pub const MAX_USERNAME_LENGTH: usize = 64;

/// Returned by `GET /api/v1/auth/status`; available without authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// No administrator exists yet: the first-run setup page must be shown.
    pub setup_required: bool,
    pub authenticated: bool,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub remember_me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyResponse {
    pub api_key: String,
}

/// Validates credentials for setup and password changes. Returns a user-facing message.
pub fn validate_credentials(username: &str, password: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("Username is required.".into());
    }
    if username.chars().count() > MAX_USERNAME_LENGTH {
        return Err(format!(
            "Username must be at most {MAX_USERNAME_LENGTH} characters."
        ));
    }
    if username.chars().any(|c| c.is_control()) {
        return Err("Username contains invalid characters.".into());
    }
    validate_password(password)
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LENGTH {
        return Err(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters."
        ));
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err("Password is too long.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_validation() {
        assert!(validate_credentials("admin", "correct horse battery").is_ok());
        assert!(validate_credentials("", "correct horse battery").is_err());
        assert!(validate_credentials("admin", "short").is_err());
        assert!(validate_credentials("ad\nmin", "correct horse battery").is_err());
    }
}
