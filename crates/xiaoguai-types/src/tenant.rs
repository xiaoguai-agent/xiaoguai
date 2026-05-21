//! Tenant + user domain types.

use crate::ids::{TenantId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub status: TenantStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Active,
    Suspended,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub tenant_id: TenantId,
    pub email: String,
    pub display_name: String,
    pub roles: Vec<Role>,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    SystemAdmin,
    TenantAdmin,
    Member,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serializes_snake_case() {
        let s = serde_json::to_string(&Role::TenantAdmin).unwrap();
        assert_eq!(s, "\"tenant_admin\"");
    }

    #[test]
    fn tenant_status_round_trip() {
        let s = serde_json::to_string(&TenantStatus::Active).unwrap();
        let back: TenantStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TenantStatus::Active);
    }
}
