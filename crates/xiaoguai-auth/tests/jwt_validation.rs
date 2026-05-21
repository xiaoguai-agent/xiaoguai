//! End-to-end JWT validation tests against a mockito-backed JWKS endpoint.
//!
//! A pre-generated RSA-2048 keypair is embedded as PEM. The matching JWK
//! representation (n, e) is served by the mock JWKS server so the validator
//! can fetch + verify without any external dependency.

use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use xiaoguai_auth::{Claims, JwtError, JwtValidator};

/// Test RSA-2048 PKCS#8 private key (generated locally, used only in tests).
const RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCNRARiF2m31ZW4\n\
VravD9avTwu0om8YFZpHbNsJDWbZturPncsKBUJSn/XbQwim5u34MoQoJA6h2V8f\n\
/6NMJttTn6HuAHB94DwibBE7Z77kom+Ia2nVAIBY3L71z+CjIwaunBBaZo3RSiM5\n\
ejeRZDDSavBP/J77PyfMF8mcJm/0PoUJzGLexsVpH+mm18ZMqo6BSgKo6cxXz97/\n\
wgFtGkb5iO7E9n2XLbDZwZ9Dd3OKLiGZIR/ghSznWwSSkWhJsnAGx+/kgiyb7KBu\n\
2gsVb8LZQ4lV64PTJ+x7hK/jHfEPyfr4XjQ/qgx8eRb/n+VyMRwfsz5wxHQ4fMSf\n\
ypiUzurxAgMBAAECggEADMYHt27yikLv5NlCb5X5DaUSI+VEMmNRrf+z1p+1mx4l\n\
IVzdTSyokJwSOR9Ymu7qubSnqpGIGS50oYoWE/63elpG5CR6B4fNKDepXzvEURw3\n\
BZjg2vfOozpisgt3/oheRE+sUuBPFoujn8DjYWwz1fMBg9oN7h4J1TSZcFsxaF5U\n\
eO9wkgmVJahcRHHM8TYc1CygoyfQr13Braq6aPUHz1GNAzqFoZcjN7tF/vTsSz63\n\
Klujvx1vyAxd8ST+cBHrKYexIajktbK3fcDG+aaftxNJFT2uSg+9aRfquIYG5M79\n\
JWTZhX6I81Els5tNatN3dhRb+/TbZyLxIlmTqC3UWQKBgQC/UhqJNjGPANPh6vSI\n\
xBUMORhg9SedMCDJEn7qW5sFg46AOdQ9n9cFJQlxM0ofN2kxjZ/kC9xNca2LDZEE\n\
4GJeuLsYC5wJJ1iRIX5h5Fx3mUHQ+NLhPBbhlhwsli9Y9zY75jwhHitzDxkrjyZ9\n\
O1xjnMshiO8ctRDS4rkNV1hzNwKBgQC9BeO5SBGTZ8zEkTQi4QLNQ41EDhy8tjkb\n\
Irm2ZswH/ZDWiLwW1d50daCEthwU/FOmpVi1RmVbSqUCDQoVQsNxfZ57sAf9rGq1\n\
r6sdz864eKQkr2bCkZ9HKqSRc28CEkZr2iWT2bHQjl/2SwUoA6UEIUieJQ3Ju6cH\n\
iqrQtyJ3FwKBgFmgrwnEt6bUrm5R0ckYgvu866zQbIR4/YL1BTvpOlB09xcfAEz2\n\
SpcAaNH9QyYooUEzpcoBvG0TakeQTXXJYIwbYpq7JZgsNJOY60oU3zSwOWMajkAy\n\
FE4OMpi4qum0tlWNYHHrXlOCqTn8z/0vB/MqiwbkzY/XS1BgIm0blDY1AoGAA3vw\n\
TqH9cPIg3B6xD1OGcbIlEHQSI4hYVR+2vJ34dM0/tjSfAuy+RPdGFiwlKF3eTNwP\n\
XogFpkEh+X+0B+BLKfRez3jXLN3YubCbPtltvgi7PdHd2whEH1Ox5Nxz113u3l4P\n\
A0Kn/GgjbK7FUY9/oyvZ4tBcCLPkyEbODzrQ79ECgYAlJ/7Gy2g0XTbZ7n/OYhXN\n\
nogs90RIta6YVdaKmmAH2i8uJYLzTguaNk5YGj2klVgB5xhU1Yfi+maxzwgHRL+w\n\
OCErEleh/gHvJwdXg9C7Wf4gPGE0SsrPHra5AIFwsCgNkPMJmGTqYjwUPtIIlM2Y\n\
zGId9/CvXRIFOmPa1GNizw==\n\
-----END PRIVATE KEY-----\n";

/// JWK modulus (n) base64url-encoded.
const N_B64URL: &str = "jUQEYhdpt9WVuFa2rw_Wr08LtKJvGBWaR2zbCQ1m2bbqz53LCgVCUp_120MIpubt-DKEKCQOodlfH_-jTCbbU5-h7gBwfeA8ImwRO2e-5KJviGtp1QCAWNy-9c_goyMGrpwQWmaN0UojOXo3kWQw0mrwT_ye-z8nzBfJnCZv9D6FCcxi3sbFaR_pptfGTKqOgUoCqOnMV8_e_8IBbRpG-YjuxPZ9ly2w2cGfQ3dzii4hmSEf4IUs51sEkpFoSbJwBsfv5IIsm-ygbtoLFW_C2UOJVeuD0yfse4Sv4x3xD8n6-F40P6oMfHkW_5_lcjEcH7M-cMR0OHzEn8qYlM7q8Q";
const E_B64URL: &str = "AQAB";
const TEST_KID: &str = "test-key-1";

#[derive(Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    tenant_id: String,
    roles: Vec<String>,
    exp: i64,
    iat: i64,
    iss: String,
    aud: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn make_token(kid: &str, claims: &TestClaims) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_rsa_pem(RSA_PRIVATE_PEM.as_bytes()).expect("priv key");
    encode(&header, claims, &key).expect("encode")
}

fn jwks_body(kid: &str) -> String {
    json!({
        "keys": [{
            "kty": "RSA",
            "kid": kid,
            "alg": "RS256",
            "use": "sig",
            "n": N_B64URL,
            "e": E_B64URL,
        }]
    })
    .to_string()
}

fn default_claims() -> TestClaims {
    let n = now();
    TestClaims {
        sub: "user-42".into(),
        tenant_id: "tenant-a".into(),
        roles: vec!["member".into()],
        iat: n - 5,
        exp: n + 600,
        iss: "https://issuer.example.com".into(),
        aud: "xiaoguai-api".into(),
    }
}

#[tokio::test]
async fn valid_token_returns_claims() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    let token = make_token(TEST_KID, &default_claims());
    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let claims: Claims = v.validate(&token).await.expect("valid");
    assert_eq!(claims.sub, "user-42");
    assert_eq!(claims.tenant_id, "tenant-a");
    assert_eq!(claims.roles, vec!["member".to_string()]);
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    let mut c = default_claims();
    // Past the 30s leeway.
    c.exp = now() - 120;
    c.iat = now() - 600;
    let token = make_token(TEST_KID, &c);

    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::Expired), "got {err:?}");
}

#[tokio::test]
async fn wrong_issuer_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    let mut c = default_claims();
    c.iss = "https://evil.example.com".into();
    let token = make_token(TEST_KID, &c);

    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::IssuerMismatch), "got {err:?}");
}

#[tokio::test]
async fn wrong_audience_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    let mut c = default_claims();
    c.aud = "some-other-app".into();
    let token = make_token(TEST_KID, &c);

    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::AudienceMismatch), "got {err:?}");
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    let token = make_token(TEST_KID, &default_claims());
    // Flip a byte in the signature segment.
    let mut parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let mut sig = parts[2].to_string();
    sig.replace_range(0..1, if sig.starts_with('A') { "B" } else { "A" });
    parts[2] = &sig;
    let tampered = parts.join(".");

    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&tampered).await.unwrap_err();
    assert!(matches!(err, JwtError::InvalidSignature), "got {err:?}");
}

#[tokio::test]
async fn unknown_kid_triggers_refresh_then_errors() {
    let mut server = mockito::Server::new_async().await;
    // Server always returns JWKS containing only `test-key-1`, never the unknown kid.
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .expect_at_least(1)
        .create_async()
        .await;

    let token = make_token("unknown-kid-999", &default_claims());
    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::UnknownKid), "got {err:?}");
}

#[tokio::test]
async fn hs256_algorithm_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_body(jwks_body(TEST_KID))
        .create_async()
        .await;

    // Forge an HS256 token with a fake secret — validator must reject before
    // even looking at the key.
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(TEST_KID.to_string());
    let key = EncodingKey::from_secret(b"any-shared-secret");
    let token = encode(&header, &default_claims(), &key).expect("encode hs256");

    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        format!("{}/jwks", server.url()),
    );
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::Malformed), "got {err:?}");
}

#[tokio::test]
async fn empty_token_is_rejected() {
    let v = JwtValidator::new(
        "https://issuer.example.com",
        "xiaoguai-api",
        "http://127.0.0.1:1/jwks",
    );
    let err = v.validate("").await.unwrap_err();
    assert!(matches!(err, JwtError::Missing));
}

#[tokio::test]
async fn jwks_endpoint_unreachable_is_fetch_error() {
    let v = JwtValidator::with_jwks_ttl(
        "https://issuer.example.com",
        "xiaoguai-api",
        // Unroutable address.
        "http://127.0.0.1:1/jwks",
        Duration::from_secs(60),
    );
    let token = make_token(TEST_KID, &default_claims());
    let err = v.validate(&token).await.unwrap_err();
    assert!(matches!(err, JwtError::JwksFetch(_)), "got {err:?}");
}
