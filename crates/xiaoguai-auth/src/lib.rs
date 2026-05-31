//! OIDC JWT validation + Casbin RBAC.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod jwt;
pub mod rbac;

pub use jwt::{Claims, JwtError, JwtValidator};
pub use rbac::{Authz, DbPolicyRow, RbacError};
