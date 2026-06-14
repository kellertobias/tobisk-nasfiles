use serde::{Deserialize, Serialize};

/// Represents a share in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub id: String,
    pub token_hash: String,
    pub owner_user_id: String,
    pub root_kind: String,
    pub root_key: String,
    pub relative_path: String,
    pub is_directory: bool,
    pub target_kind: TargetKind,
    pub target_user_id: Option<String>,
    pub password_hash: Option<String>,
    pub allow_upload: bool,
    pub allow_download: bool,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// The kind of share target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    User,
    Guest,
    Public,
}

impl TargetKind {
    pub fn as_str(&self) -> &str {
        match self {
            TargetKind::User => "user",
            TargetKind::Guest => "guest",
            TargetKind::Public => "public",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(TargetKind::User),
            "guest" => Some(TargetKind::Guest),
            "public" => Some(TargetKind::Public),
            _ => None,
        }
    }
}

/// Request body for creating a share.
#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    pub root_key: String,
    pub path: String,
    pub target_kind: TargetKind,
    /// User ID for user-type shares
    pub target_user_id: Option<String>,
    /// Password for guest-type shares
    pub password: Option<String>,
    pub allow_upload: bool,
    pub allow_download: bool,
    /// Expiry in seconds from now (null = never)
    pub expires_in: Option<i64>,
}

/// Public metadata for a share (returned to unauthenticated users).
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct ShareMetadata {
    pub name: String,
    pub is_directory: bool,
    pub requires_password: bool,
    pub owner_display_name: String,
    pub allow_upload: bool,
    pub allow_download: bool,
}

/// Auth request body for guest shares.
#[derive(Debug, Deserialize)]
pub struct ShareAuthRequest {
    pub password: Option<String>,
}
