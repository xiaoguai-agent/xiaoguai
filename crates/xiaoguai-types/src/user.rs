//! User domain type.

use crate::ids::UserId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
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
}
