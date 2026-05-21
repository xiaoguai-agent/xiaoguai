//! Strongly-typed ID newtypes to prevent mixing tenant / user / session IDs.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_newtype {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, Uuid::new_v4().simple()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_newtype!(TenantId, "ten");
id_newtype!(UserId, "usr");
id_newtype!(SessionId, "sess");
id_newtype!(MessageId, "msg");
id_newtype!(ToolCallId, "tc");
id_newtype!(McpServerInstanceId, "mcp");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_distinct_prefixes() {
        let t = TenantId::new();
        let u = UserId::new();
        assert!(t.as_str().starts_with("ten_"));
        assert!(u.as_str().starts_with("usr_"));
        assert_ne!(t.as_str(), u.as_str());
    }

    #[test]
    fn ids_serialize_as_strings() {
        let id = SessionId::from("sess_abc".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""sess_abc""#);
    }
}
