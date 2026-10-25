//! OIDC policy around maintained JWT verification. JWT parsing, JWK conversion and signature
//! verification belong to jsonwebtoken's AWS-LC provider; no private-key RSA arithmetic is used.
use crate::AuthService;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha384, Sha512};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub scopes_supported: Option<Vec<String>>,
    pub id_token_signing_alg_values_supported: Vec<String>,
}
#[derive(Debug)]
pub(crate) enum VerifyError {
    Unreachable,
    Invalid,
}
type Result<T> = std::result::Result<T, VerifyError>;

fn endpoint(value: &str) -> Result<()> {
    let url = url::Url::parse(value).map_err(|_| VerifyError::Invalid)?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(VerifyError::Invalid);
    }
    Ok(())
}
async fn document(http: &oauth2::reqwest::Client, endpoint: &str) -> Result<Value> {
    let mut response = http
        .get(endpoint)
        .send()
        .await
        .map_err(|_| VerifyError::Unreachable)?;
    if !response.status().is_success() {
        return Err(VerifyError::Unreachable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| VerifyError::Unreachable)?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(VerifyError::Invalid);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| VerifyError::Invalid)
}
pub(crate) async fn discover(http: &oauth2::reqwest::Client, issuer: &str) -> Result<Provider> {
    endpoint(issuer)?;
    let value = document(
        http,
        &format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ),
    )
    .await?;
    let provider: Provider = serde_json::from_value(value).map_err(|_| VerifyError::Invalid)?;
    if provider.issuer != issuer || provider.id_token_signing_alg_values_supported.is_empty() {
        return Err(VerifyError::Invalid);
    }
    for value in [
        &provider.authorization_endpoint,
        &provider.token_endpoint,
        &provider.jwks_uri,
    ] {
        endpoint(value)?;
    }
    Ok(provider)
}
fn supported(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::EdDSA
    )
}
fn equal(first: &str, second: &str) -> bool {
    bool::from(first.as_bytes().ct_eq(second.as_bytes()))
}
fn token_hash(token: &str, algorithm: Algorithm) -> Result<String> {
    let bytes = match algorithm {
        Algorithm::RS256 | Algorithm::PS256 | Algorithm::ES256 => {
            Sha256::digest(token.as_bytes()).to_vec()
        }
        Algorithm::RS384 | Algorithm::PS384 | Algorithm::ES384 => {
            Sha384::digest(token.as_bytes()).to_vec()
        }
        Algorithm::RS512 | Algorithm::PS512 | Algorithm::EdDSA => {
            Sha512::digest(token.as_bytes()).to_vec()
        }
        _ => return Err(VerifyError::Invalid),
    };
    Ok(URL_SAFE_NO_PAD.encode(&bytes[..bytes.len() / 2]))
}

struct Binding<'a> {
    issuer: &'a str,
    client_id: &'a str,
    nonce: &'a str,
    refresh: bool,
    access: &'a str,
    code: Option<&'a str>,
    algorithm: Algorithm,
    now: i64,
}
fn claims_policy(claims: &Value, binding: Binding<'_>) -> Result<()> {
    let Binding {
        issuer,
        client_id,
        nonce,
        refresh,
        access,
        code,
        algorithm,
        now,
    } = binding;
    if claims["iss"].as_str() != Some(issuer)
        || claims["sub"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 255)
    {
        return Err(VerifyError::Invalid);
    }
    let audiences: Vec<&str> = match &claims["aud"] {
        Value::String(value) => vec![value],
        Value::Array(values) => values
            .iter()
            .map(|value| value.as_str().ok_or(VerifyError::Invalid))
            .collect::<Result<_>>()?,
        _ => return Err(VerifyError::Invalid),
    };
    if !audiences.contains(&client_id)
        || (audiences.len() > 1 && claims["azp"].as_str() != Some(client_id))
        || claims
            .get("azp")
            .is_some_and(|value| value.as_str() != Some(client_id))
    {
        return Err(VerifyError::Invalid);
    }
    let exp = claims["exp"].as_i64().ok_or(VerifyError::Invalid)?;
    let iat = claims["iat"].as_i64().ok_or(VerifyError::Invalid)?;
    if exp <= now.saturating_sub(60)
        || iat > now.saturating_add(60)
        || exp <= iat
        || claims.get("nbf").is_some_and(|value| {
            value
                .as_i64()
                .is_none_or(|time| time > now.saturating_add(60))
        })
    {
        return Err(VerifyError::Invalid);
    }
    match claims.get("nonce").and_then(Value::as_str) {
        Some(actual) if equal(actual, nonce) => {}
        None if refresh => {}
        _ => return Err(VerifyError::Invalid),
    }
    if let Some(expected) = claims.get("at_hash") {
        if !expected.as_str().is_some_and(|expected| {
            token_hash(access, algorithm).is_ok_and(|actual| equal(&actual, expected))
        }) {
            return Err(VerifyError::Invalid);
        }
    }
    if let (Some(expected), Some(code)) = (claims.get("c_hash"), code) {
        if !expected.as_str().is_some_and(|expected| {
            token_hash(code, algorithm).is_ok_and(|actual| equal(&actual, expected))
        }) {
            return Err(VerifyError::Invalid);
        }
    }
    Ok(())
}

pub(crate) async fn verify(
    auth: &AuthService,
    token: &str,
    access: &str,
    nonce: &str,
    refresh: bool,
    code: Option<&str>,
) -> Result<Value> {
    if token.len() > 64 * 1024 {
        return Err(VerifyError::Invalid);
    }
    let header = decode_header(token).map_err(|_| VerifyError::Invalid)?;
    let algorithm = format!("{:?}", header.alg);
    let provider = auth.0.provider.get().ok_or(VerifyError::Unreachable)?;
    if !supported(header.alg)
        || !provider
            .id_token_signing_alg_values_supported
            .contains(&algorithm)
        || header
            .crit
            .as_ref()
            .is_some_and(|fields| !fields.is_empty())
    {
        return Err(VerifyError::Invalid);
    }
    // Fetch from the discovered JWKS URI, never token-controlled jku/x5u URLs. Reading current
    // keys also handles provider key rotation without a restart or accepting stale keys forever.
    let jwks: JwkSet = serde_json::from_value(document(&auth.0.http, &provider.jwks_uri).await?)
        .map_err(|_| VerifyError::Invalid)?;
    let keys = jwks
        .keys
        .iter()
        .filter(|key| {
            header.kid.as_ref().map_or(jwks.keys.len() == 1, |kid| {
                key.common.key_id.as_ref() == Some(kid)
            })
        })
        .collect::<Vec<_>>();
    if keys.len() != 1 {
        return Err(VerifyError::Invalid);
    }
    let key = keys[0];
    let raw = serde_json::to_value(key).map_err(|_| VerifyError::Invalid)?;
    if raw
        .get("use")
        .is_some_and(|value| value.as_str() != Some("sig"))
        || raw
            .get("alg")
            .is_some_and(|value| value.as_str() != Some(&algorithm))
        || raw.get("key_ops").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|values| !values.iter().any(|value| value.as_str() == Some("verify")))
        })
    {
        return Err(VerifyError::Invalid);
    }
    let key = DecodingKey::from_jwk(key).map_err(|_| VerifyError::Invalid)?;
    let mut validation = Validation::new(header.alg);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
    validation.set_issuer(&[provider.issuer.as_str()]);
    validation.set_audience(&[auth.0.idp.client_id.as_str()]);
    validation.validate_nbf = true;
    let decoded = decode::<Value>(token, &key, &validation).map_err(|_| VerifyError::Invalid)?;
    claims_policy(
        &decoded.claims,
        Binding {
            issuer: &provider.issuer,
            client_id: &auth.0.idp.client_id,
            nonce,
            refresh,
            access,
            code,
            algorithm: header.alg,
            now: auth.now() / 1000,
        },
    )?;
    Ok(decoded.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn aws_lc_verifies_discovered_jwk_and_rejects_forgery_wrong_kid_and_hmac() {
        use aws_lc_rs::{
            rand::SystemRandom,
            signature::{Ed25519KeyPair, KeyPair},
        };
        use axum::{Json, Router, routing::get};
        use jsonwebtoken::{EncodingKey, Header, encode};
        use std::sync::Arc;
        let private = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let key = Ed25519KeyPair::from_pkcs8(private.as_ref()).unwrap();
        let jwks = json!({"keys":[{"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),"kid":"current","use":"sig","key_ops":["verify"],"alg":"EdDSA"}]});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/keys", get(move || async move { Json(jwks) })),
            )
            .await
            .unwrap()
        });
        let db = pf_db::temp_db();
        let mut loaded = crate::tests::loaded();
        loaded.site.idp.issuer = issuer.clone();
        loaded.site.idp.client_id = "client".into();
        let auth = AuthService::from_loaded(
            db.db.clone(),
            Arc::new(pf_caps::EpochStore::default()),
            &loaded,
        )
        .unwrap();
        auth.0
            .provider
            .set(Provider {
                issuer: issuer.clone(),
                authorization_endpoint: format!("{issuer}/auth"),
                token_endpoint: format!("{issuer}/token"),
                jwks_uri: format!("{issuer}/keys"),
                scopes_supported: None,
                id_token_signing_alg_values_supported: vec!["EdDSA".into(), "HS256".into()],
            })
            .unwrap();
        let now = auth.now() / 1000;
        let claims = json!({"iss":issuer,"aud":"client","sub":"person","exp":now+3600,"iat":now,"nonce":"nonce","at_hash":token_hash("access",Algorithm::EdDSA).unwrap()});
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("current".into());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_ed_der(private.as_ref()),
        )
        .unwrap();
        assert!(
            verify(&auth, &token, "access", "nonce", false, None)
                .await
                .is_ok()
        );
        assert!(
            verify(&auth, &token, "different-access", "nonce", false, None)
                .await
                .is_err()
        );
        assert!(
            verify(&auth, &token, "access", "different-nonce", false, None)
                .await
                .is_err()
        );
        let mut pieces = token.split('.').map(str::to_owned).collect::<Vec<_>>();
        let mut signature = URL_SAFE_NO_PAD.decode(&pieces[2]).unwrap();
        signature[0] ^= 1;
        pieces[2] = URL_SAFE_NO_PAD.encode(signature);
        assert!(
            verify(&auth, &pieces.join("."), "access", "nonce", false, None)
                .await
                .is_err()
        );
        header.kid = Some("other-key".into());
        let wrong_key = encode(
            &header,
            &claims,
            &EncodingKey::from_ed_der(private.as_ref()),
        )
        .unwrap();
        assert!(
            verify(&auth, &wrong_key, "access", "nonce", false, None)
                .await
                .is_err()
        );
        let mut hmac = Header::new(Algorithm::HS256);
        hmac.kid = Some("current".into());
        let confused = encode(
            &hmac,
            &claims,
            &EncodingKey::from_secret(key.public_key().as_ref()),
        )
        .unwrap();
        assert!(
            verify(&auth, &confused, "access", "nonce", false, None)
                .await
                .is_err()
        );
        server.abort();
    }
    #[test]
    fn oidc_binding_rejects_wrong_issuer_audience_nonce_azp_time_and_hashes() {
        let baseline = json!({"iss":"https://idp.example.com","sub":"person","aud":"client","exp":2000,"iat":1000,"nonce":"nonce"});
        let check = |value: &Value, refresh| {
            claims_policy(
                value,
                Binding {
                    issuer: "https://idp.example.com",
                    client_id: "client",
                    nonce: "nonce",
                    refresh,
                    access: "access",
                    code: Some("code"),
                    algorithm: Algorithm::RS256,
                    now: 1500,
                },
            )
        };
        assert!(check(&baseline, false).is_ok());
        for (field, value) in [
            ("iss", json!("https://other.example.com")),
            ("aud", json!("other-client")),
            ("nonce", json!("other-nonce")),
            ("azp", json!("other-client")),
            ("iat", json!(1700)),
            ("exp", json!(1400)),
            ("sub", json!("")),
            ("at_hash", json!("wrong")),
            ("c_hash", json!("wrong")),
        ] {
            let mut claims = baseline.clone();
            claims[field] = value;
            assert!(check(&claims, false).is_err(), "{field}");
        }
        let mut claims = baseline.clone();
        claims["aud"] = json!(["client", "other"]);
        assert!(check(&claims, false).is_err());
        claims["azp"] = json!("client");
        assert!(check(&claims, false).is_ok());
        claims["at_hash"] = json!(token_hash("access", Algorithm::RS256).unwrap());
        claims["c_hash"] = json!(token_hash("code", Algorithm::RS256).unwrap());
        assert!(check(&claims, false).is_ok());
        claims.as_object_mut().unwrap().remove("nonce");
        assert!(check(&claims, false).is_err());
        assert!(check(&claims, true).is_ok());
        assert!(!supported(Algorithm::HS256));
        assert!(!supported(Algorithm::HS384));
        assert!(!supported(Algorithm::HS512));
    }
}
