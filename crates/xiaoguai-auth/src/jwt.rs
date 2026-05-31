//! OIDC JWT validation with JWKS lookup, RS256/ES256 only.
//!
//! HS256 and other symmetric algorithms are explicitly rejected because
//! shared secrets are easy to leak through logs, env-var sprawl, or
//! mis-configured key endpoints.

use std::time::Duration;

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[path = "jwks_cache.rs"]
mod jwks_cache;

pub use jwks_cache::{Jwk, JwksCache, DEFAULT_TTL, MIN_REFRESH_INTERVAL};

/// Tolerance for clock skew between issuer and verifier (seconds).
pub const CLOCK_SKEW_LEEWAY_SECS: u64 = 30;

/// Claims extracted from a verified token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user id).
    pub sub: String,
    /// Tenant id the user is acting in.
    pub tenant_id: String,
    /// Role names assigned at issue time.
    pub roles: Vec<String>,
    /// OAuth 2.0-style scope strings carried by the token (sprint-13
    /// DEC-HLD-016). Empty for legacy tokens issued before sprint-13;
    /// scope-gated routes (e.g. `POST /v1/hotl/decisions`) treat the
    /// empty set as "no scopes" and return 403.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Expiry (seconds since epoch).
    pub exp: i64,
    /// Issued-at (seconds since epoch).
    pub iat: i64,
    /// Issuer.
    pub iss: String,
    /// Audience.
    pub aud: String,
}

/// JWT validation errors.
#[derive(Debug, Error)]
pub enum JwtError {
    /// Token string was empty.
    #[error("missing token")]
    Missing,
    /// Token was structurally malformed or used a disallowed algorithm.
    #[error("malformed token")]
    Malformed,
    /// Cryptographic signature did not verify.
    #[error("signature invalid")]
    InvalidSignature,
    /// Token's `exp` is in the past (beyond clock-skew leeway).
    #[error("token expired")]
    Expired,
    /// `iss` did not match the validator's expected issuer.
    #[error("issuer mismatch")]
    IssuerMismatch,
    /// `aud` did not match the validator's expected audience.
    #[error("audience mismatch")]
    AudienceMismatch,
    /// JWKS endpoint fetch failed.
    #[error("jwks fetch failed: {0}")]
    JwksFetch(String),
    /// The token's `kid` is not in the JWKS even after a refresh.
    #[error("kid not in jwks")]
    UnknownKid,
}

/// Validator that fetches public keys from a remote JWKS endpoint and
/// verifies tokens against a fixed issuer + audience.
pub struct JwtValidator {
    issuer: String,
    audience: String,
    jwks_cache: JwksCache,
}

impl JwtValidator {
    /// Build a validator. JWKS is lazily fetched on the first `validate`.
    #[must_use]
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_url: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_cache: JwksCache::new(jwks_url),
        }
    }

    /// Build a validator with a custom JWKS TTL.
    #[must_use]
    pub fn with_jwks_ttl(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_url: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_cache: JwksCache::with_ttl(jwks_url, ttl),
        }
    }

    /// Validate the given compact-serialized JWT. Returns the parsed claims
    /// on success.
    ///
    /// # Errors
    /// See [`JwtError`].
    pub async fn validate(&self, token: &str) -> Result<Claims, JwtError> {
        if token.is_empty() {
            return Err(JwtError::Missing);
        }

        let header = decode_header(token).map_err(|_| JwtError::Malformed)?;

        // Reject symmetric / `none` algorithms outright.
        let alg = match header.alg {
            Algorithm::RS256 | Algorithm::ES256 => header.alg,
            _ => return Err(JwtError::Malformed),
        };

        let kid = header.kid.ok_or(JwtError::Malformed)?;

        let jwk = self
            .jwks_cache
            .get(&kid)
            .await
            .map_err(JwtError::JwksFetch)?
            .ok_or(JwtError::UnknownKid)?;

        let decoding_key = decoding_key_from_jwk(&jwk, alg)?;

        let mut validation = Validation::new(alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = CLOCK_SKEW_LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_aud = true;
        validation.algorithms = vec![alg];

        let token_data =
            decode::<Claims>(token, &decoding_key, &validation).map_err(|e| map_jwt_error(&e))?;

        Ok(token_data.claims)
    }
}

fn decoding_key_from_jwk(jwk: &Jwk, alg: Algorithm) -> Result<DecodingKey, JwtError> {
    match alg {
        Algorithm::RS256 => {
            if jwk.kty != "RSA" {
                return Err(JwtError::Malformed);
            }
            let n = jwk.n.as_deref().ok_or(JwtError::Malformed)?;
            let e = jwk.e.as_deref().ok_or(JwtError::Malformed)?;
            DecodingKey::from_rsa_components(n, e).map_err(|_| JwtError::Malformed)
        }
        Algorithm::ES256 => {
            if jwk.kty != "EC" {
                return Err(JwtError::Malformed);
            }
            let x = jwk.x.as_deref().ok_or(JwtError::Malformed)?;
            let y = jwk.y.as_deref().ok_or(JwtError::Malformed)?;
            DecodingKey::from_ec_components(x, y).map_err(|_| JwtError::Malformed)
        }
        _ => Err(JwtError::Malformed),
    }
}

fn map_jwt_error(err: &jsonwebtoken::errors::Error) -> JwtError {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::ExpiredSignature => JwtError::Expired,
        ErrorKind::InvalidIssuer => JwtError::IssuerMismatch,
        ErrorKind::InvalidAudience => JwtError::AudienceMismatch,
        ErrorKind::InvalidSignature
        | ErrorKind::InvalidKeyFormat
        | ErrorKind::InvalidRsaKey(_)
        | ErrorKind::InvalidEcdsaKey => JwtError::InvalidSignature,
        _ => JwtError::Malformed,
    }
}
