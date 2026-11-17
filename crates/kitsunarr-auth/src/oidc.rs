//! OpenID Connect primitives for signing in with an identity provider (Tanukarr's OIDC
//! authentication): PKCE (`S256`), and ID-token validation — the JWS signature against the
//! provider's JSON Web Key Set, then the issuer, audience, lifetime and nonce claims, as
//! ASP.NET's `OpenIdConnectHandler` checks them.
//!
//! Supported signatures: `RS256/384/512`, `PS256/384/512`, `ES256/384/512` and `EdDSA`
//! (Ed25519). Unsigned tokens (`none`) and shared-secret signatures (`HS*`) are refused.

use aws_lc_rs::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwtError {
    #[error("the ID token is malformed")]
    Malformed,
    #[error("the ID token is signed with {0}, which is not supported")]
    UnsupportedAlgorithm(String),
    #[error("no key of the provider matches the ID token")]
    UnknownKey,
    #[error("the ID token's signature is invalid")]
    BadSignature,
    #[error("the ID token is not valid: {0}")]
    Claims(String),
}

/// The PKCE `S256` challenge of a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A JWS in compact serialisation, split and decoded (not yet verified).
#[derive(Debug, Clone)]
pub struct Jws {
    pub header: Value,
    pub claims: Value,
    signing_input: String,
    signature: Vec<u8>,
}

fn b64(part: &str) -> Result<Vec<u8>, JwtError> {
    URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .map_err(|_| JwtError::Malformed)
}

/// Splits and decodes a compact JWS (`header.payload.signature`).
pub fn decode(token: &str) -> Result<Jws, JwtError> {
    let mut parts = token.trim().split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed);
    };
    let header: Value = serde_json::from_slice(&b64(h)?).map_err(|_| JwtError::Malformed)?;
    let claims: Value = serde_json::from_slice(&b64(p)?).map_err(|_| JwtError::Malformed)?;
    if !header.is_object() || !claims.is_object() {
        return Err(JwtError::Malformed);
    }
    Ok(Jws {
        header,
        claims,
        signing_input: format!("{h}.{p}"),
        signature: b64(s)?,
    })
}

#[derive(Clone, Copy)]
enum KeyKind {
    Rsa(&'static signature::RsaParameters),
    Ec(&'static str, &'static signature::EcdsaVerificationAlgorithm),
    Ed25519,
}

fn algorithm(alg: &str) -> Result<KeyKind, JwtError> {
    Ok(match alg {
        "RS256" => KeyKind::Rsa(&signature::RSA_PKCS1_2048_8192_SHA256),
        "RS384" => KeyKind::Rsa(&signature::RSA_PKCS1_2048_8192_SHA384),
        "RS512" => KeyKind::Rsa(&signature::RSA_PKCS1_2048_8192_SHA512),
        "PS256" => KeyKind::Rsa(&signature::RSA_PSS_2048_8192_SHA256),
        "PS384" => KeyKind::Rsa(&signature::RSA_PSS_2048_8192_SHA384),
        "PS512" => KeyKind::Rsa(&signature::RSA_PSS_2048_8192_SHA512),
        "ES256" => KeyKind::Ec("P-256", &signature::ECDSA_P256_SHA256_FIXED),
        "ES384" => KeyKind::Ec("P-384", &signature::ECDSA_P384_SHA384_FIXED),
        "ES512" => KeyKind::Ec("P-521", &signature::ECDSA_P521_SHA512_FIXED),
        "EdDSA" => KeyKind::Ed25519,
        other => return Err(JwtError::UnsupportedAlgorithm(other.to_string())),
    })
}

fn key_field(key: &Value, name: &str) -> Option<Vec<u8>> {
    b64(key.get(name)?.as_str()?).ok()
}

/// Whether one JWK verifies the signature with the algorithm.
fn verify_with(kind: KeyKind, key: &Value, message: &[u8], sig: &[u8]) -> bool {
    let kty = key["kty"].as_str().unwrap_or_default();
    match kind {
        KeyKind::Rsa(params) if kty == "RSA" => {
            let (Some(n), Some(e)) = (key_field(key, "n"), key_field(key, "e")) else {
                return false;
            };
            RsaPublicKeyComponents { n, e }
                .verify(params, message, sig)
                .is_ok()
        }
        KeyKind::Ec(curve, alg) if kty == "EC" && key["crv"] == curve => {
            let (Some(x), Some(y)) = (key_field(key, "x"), key_field(key, "y")) else {
                return false;
            };
            let mut point = Vec::with_capacity(1 + x.len() + y.len());
            point.push(0x04);
            point.extend(x);
            point.extend(y);
            UnparsedPublicKey::new(alg, point)
                .verify(message, sig)
                .is_ok()
        }
        KeyKind::Ed25519 if kty == "OKP" && key["crv"] == "Ed25519" => {
            let Some(x) = key_field(key, "x") else {
                return false;
            };
            UnparsedPublicKey::new(&signature::ED25519, x)
                .verify(message, sig)
                .is_ok()
        }
        _ => false,
    }
}

impl Jws {
    /// Verifies the signature with the provider's key set (`{"keys": [...]}`): a key with the
    /// token's `kid` (any suitable key when the token names none) must verify it.
    pub fn verify(&self, jwks: &Value) -> Result<(), JwtError> {
        let alg = self.header["alg"].as_str().ok_or(JwtError::Malformed)?;
        let kind = algorithm(alg)?;
        let kid = self.header["kid"].as_str();
        let candidates: Vec<&Value> = jwks["keys"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter(|k| kid.is_none_or(|kid| k["kid"].as_str() == Some(kid)))
                    .filter(|k| k["use"].as_str().is_none_or(|u| u == "sig"))
                    .filter(|k| k["alg"].as_str().is_none_or(|a| a == alg))
                    .collect()
            })
            .unwrap_or_default();
        if candidates.is_empty() {
            return Err(JwtError::UnknownKey);
        }
        let message = self.signing_input.as_bytes();
        if candidates
            .iter()
            .any(|k| verify_with(kind, k, message, &self.signature))
        {
            Ok(())
        } else {
            Err(JwtError::BadSignature)
        }
    }
}

/// What an ID token must say.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    /// Unix seconds.
    pub now: i64,
    /// Allowed clock skew in seconds (ASP.NET's default is five minutes).
    pub leeway: i64,
}

fn numeric(claims: &Value, name: &str) -> Option<i64> {
    let v = &claims[name];
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Checks a signature-verified ID token's claims (OpenID Connect Core §3.1.3.7).
pub fn check_claims(claims: &Value, expected: &Expected<'_>) -> Result<(), JwtError> {
    let fail = |m: &str| Err(JwtError::Claims(m.to_string()));
    if claims["iss"].as_str() != Some(expected.issuer) {
        return fail("issued by another provider");
    }
    let audiences: Vec<&str> = match &claims["aud"] {
        Value::String(a) => vec![a.as_str()],
        Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !audiences.contains(&expected.client_id) {
        return fail("issued for another client");
    }
    if let Some(azp) = claims["azp"].as_str()
        && azp != expected.client_id
    {
        return fail("authorized for another client");
    }
    match numeric(claims, "exp") {
        Some(exp) if exp + expected.leeway > expected.now => {}
        Some(_) => return fail("expired"),
        None => return fail("without an expiry"),
    }
    if numeric(claims, "iat").is_none() {
        return fail("without an issue time");
    }
    if numeric(claims, "nbf").is_some_and(|nbf| nbf - expected.leeway > expected.now) {
        return fail("not valid yet");
    }
    if claims["nonce"].as_str() != Some(expected.nonce) {
        return fail("for another sign-in (nonce)");
    }
    if claims["sub"].as_str().is_none_or(str::is_empty) {
        return fail("without a subject");
    }
    Ok(())
}

/// Decodes, verifies and checks an ID token; returns its claims.
pub fn validate_id_token(
    token: &str,
    jwks: &Value,
    expected: &Expected<'_>,
) -> Result<Value, JwtError> {
    let jws = decode(token)?;
    jws.verify(jwks)?;
    check_claims(&jws.claims, expected)?;
    Ok(jws.claims)
}

/// The identifiers an authenticated user is known by, as Tanukarr's `GetIdentifiers` reads
/// them: the e-mail address when the provider says it is verified (from the user-info answer
/// when it has one, else from the ID token), then the subject.
pub fn identifiers(id_claims: &Value, userinfo: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    let verified = |v: &Value| match &v["email_verified"] {
        Value::Bool(b) => *b,
        Value::String(s) => s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    match userinfo.filter(|u| u["email"].is_string()) {
        Some(u) => {
            if verified(u) {
                out.push(u["email"].as_str().unwrap_or_default().to_string());
            }
        }
        None => {
            if let Some(email) = id_claims["email"].as_str().filter(|e| !e.trim().is_empty())
                && verified(id_claims)
            {
                out.push(email.to_string());
            }
        }
    }
    let sub = userinfo
        .and_then(|u| u["sub"].as_str())
        .or_else(|| id_claims["sub"].as_str());
    if let Some(sub) = sub.filter(|s| !s.trim().is_empty()) {
        out.push(sub.to_string());
    }
    out
}

/// Signs test tokens (a stand-in identity provider for tests).
#[doc(hidden)]
pub mod testing {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{self, EcdsaKeyPair, KeyPair};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::{Value, json};

    /// An ES256 signing key and its JWK.
    pub struct TestKey {
        pair: EcdsaKeyPair,
        pub kid: String,
    }

    impl TestKey {
        pub fn generate(kid: &str) -> Self {
            Self {
                pair: EcdsaKeyPair::generate(&signature::ECDSA_P256_SHA256_FIXED_SIGNING)
                    .expect("P-256 key generation"),
                kid: kid.to_string(),
            }
        }

        pub fn jwk(&self) -> Value {
            let point = self.pair.public_key().as_ref();
            json!({
                "kty": "EC", "crv": "P-256", "use": "sig", "alg": "ES256", "kid": self.kid,
                "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            })
        }

        pub fn sign(&self, claims: &Value) -> String {
            let header = json!({"alg": "ES256", "typ": "JWT", "kid": self.kid});
            let input = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(header.to_string()),
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            let sig = self
                .pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .expect("signing");
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestKey;
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    fn claims(now: i64) -> Value {
        json!({"iss": "https://idp.example", "aud": "kitsunarr", "sub": "user-1", "exp": now + 300,
            "iat": now, "nonce": "n0nce", "email": "fox@example.com", "email_verified": true})
    }

    fn expected(now: i64) -> Expected<'static> {
        Expected {
            issuer: "https://idp.example",
            client_id: "kitsunarr",
            nonce: "n0nce",
            now,
            leeway: 300,
        }
    }

    #[test]
    fn pkce_matches_rfc7636_appendix_b() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn es256_tokens_verify_and_claims_are_checked() {
        let now = 1_800_000_000;
        let key = TestKey::generate("k1");
        let jwks = json!({"keys": [key.jwk()]});
        let token = key.sign(&claims(now));
        let c = validate_id_token(&token, &jwks, &expected(now)).unwrap();
        assert_eq!(c["sub"], "user-1");

        // Another key with the same kid: bad signature; unknown kid: no key.
        let other = TestKey::generate("k1");
        assert_eq!(
            validate_id_token(&other.sign(&claims(now)), &jwks, &expected(now)),
            Err(JwtError::BadSignature)
        );
        let stranger = TestKey::generate("k2");
        assert_eq!(
            validate_id_token(&stranger.sign(&claims(now)), &jwks, &expected(now)),
            Err(JwtError::UnknownKey)
        );
        // Tampered payload.
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut forged = claims(now);
        forged["sub"] = json!("admin");
        let payload = URL_SAFE_NO_PAD.encode(forged.to_string());
        parts[1] = &payload;
        assert_eq!(
            validate_id_token(&parts.join("."), &jwks, &expected(now)),
            Err(JwtError::BadSignature)
        );

        // Claims.
        let check = |f: &dyn Fn(&mut Value)| {
            let mut c = claims(now);
            f(&mut c);
            validate_id_token(&key.sign(&c), &jwks, &expected(now))
        };
        assert!(check(&|c| c["iss"] = json!("https://evil.example")).is_err());
        assert!(check(&|c| c["aud"] = json!("someone-else")).is_err());
        assert!(check(&|c| c["aud"] = json!(["other", "kitsunarr"])).is_ok());
        assert!(check(&|c| c["azp"] = json!("other")).is_err());
        assert!(check(&|c| c["exp"] = json!(now - 301)).is_err());
        assert!(check(&|c| c["exp"] = json!(now - 10)).is_ok(), "clock skew");
        assert!(check(&|c| c["nbf"] = json!(now + 600)).is_err());
        assert!(check(&|c| c["nonce"] = json!("replayed")).is_err());
        assert!(check(&|c| c["sub"] = json!("")).is_err());
        assert!(
            check(&|c| {
                c.as_object_mut().unwrap().remove("iat");
            })
            .is_err()
        );
    }

    #[test]
    fn unsigned_and_hmac_tokens_are_refused() {
        let now = 1_800_000_000;
        let jwks = json!({"keys": [TestKey::generate("k1").jwk()]});
        for alg in ["none", "HS256"] {
            let header = URL_SAFE_NO_PAD.encode(json!({"alg": alg}).to_string());
            let payload = URL_SAFE_NO_PAD.encode(claims(now).to_string());
            let token = format!("{header}.{payload}.");
            assert_eq!(
                validate_id_token(&token, &jwks, &expected(now)),
                Err(JwtError::UnsupportedAlgorithm(alg.into()))
            );
        }
        assert_eq!(
            validate_id_token("a.b", &jwks, &expected(now)),
            Err(JwtError::Malformed)
        );
    }

    #[test]
    fn rs256_and_eddsa_keys_verify() {
        let now = 1_800_000_000;
        let input = |alg: &str, kid: &str| {
            format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(json!({"alg": alg, "kid": kid}).to_string()),
                URL_SAFE_NO_PAD.encode(claims(now).to_string())
            )
        };
        // RS256.
        let rsa = signature::RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let components = RsaPublicKeyComponents::<Vec<u8>>::from(rsa.public_key());
        let rsa_jwk = json!({"kty": "RSA", "kid": "r", "n": URL_SAFE_NO_PAD.encode(&components.n),
            "e": URL_SAFE_NO_PAD.encode(&components.e)});
        let signing = input("RS256", "r");
        let mut sig = vec![0; rsa.public_modulus_len()];
        rsa.sign(
            &signature::RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signing.as_bytes(),
            &mut sig,
        )
        .unwrap();
        let token = format!("{signing}.{}", URL_SAFE_NO_PAD.encode(&sig));
        assert!(validate_id_token(&token, &json!({"keys": [rsa_jwk]}), &expected(now)).is_ok());
        // EdDSA.
        let ed = Ed25519KeyPair::generate().unwrap();
        let ed_jwk = json!({"kty": "OKP", "crv": "Ed25519", "kid": "e",
            "x": URL_SAFE_NO_PAD.encode(ed.public_key().as_ref())});
        let signing = input("EdDSA", "e");
        let token = format!(
            "{signing}.{}",
            URL_SAFE_NO_PAD.encode(ed.sign(signing.as_bytes()).as_ref())
        );
        assert!(validate_id_token(&token, &json!({"keys": [ed_jwk]}), &expected(now)).is_ok());
    }

    #[test]
    fn identifiers_follow_tanukarr() {
        let id = json!({"sub": "abc", "email": "fox@example.com", "email_verified": "true"});
        assert_eq!(identifiers(&id, None), vec!["fox@example.com", "abc"]);
        // The user-info answer decides about the e-mail when it has one.
        let info = json!({"sub": "abc", "email": "other@example.com", "email_verified": false});
        assert_eq!(identifiers(&id, Some(&info)), vec!["abc"]);
        let unverified = json!({"sub": "abc", "email": "fox@example.com"});
        assert_eq!(identifiers(&unverified, None), vec!["abc"]);
    }
}
