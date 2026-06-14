use crate::config::AppConfig;

/// Get the session cookie name.
pub fn cookie_name() -> &'static str {
    "nasfiles.sid"
}

/// Whether to set Secure flag on cookies.
pub fn is_secure(config: &AppConfig) -> bool {
    config.base_url.starts_with("https://")
}
