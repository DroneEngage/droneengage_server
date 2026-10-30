//! Server-to-Server (S2S) authentication helper (js_s2s_auth.js).
//!
//! Secures S2S WebSocket channels with a challenge-response handshake:
//!   1- The ACCEPTING side sends a random nonce challenge.
//!   2- The CONNECTING side answers with an Ed25519 signature of the nonce
//!      (private key `s2s_my_private_key`) or, when the challenge's `method`
//!      is `hmac-sha256`, with HMAC-SHA256(nonce, shared secret).
//!   3- The ACCEPTING side verifies the answer before trusting the peer.
//!
//! The comm server connects out to AUTH, the storage server, UDP relays and
//! (as a child) a super server, and accepts children when it is a super
//! server (`s2s_trusted_server_keys`).
//!
//! Keys and the secret are loaded on first use and cached; a failed load is
//! retried on the next use. Key file paths resolve against the server root.

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde_json::{json, Map, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Mutex;
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::util::{js_key, js_to_string};

pub const S2S_AUTH_CHALLENGE: &str = "challenge";
pub const S2S_AUTH_RESPONSE: &str = "response";
/// ms to complete the handshake before the connection is dropped
pub const S2S_AUTH_HANDSHAKE_TIMEOUT_MS: u64 = 8000;

pub const S2S_AUTH_METHOD_ED25519: &str = "ed25519";
pub const S2S_AUTH_METHOD_HMAC: &str = "hmac-sha256";

pub const S2S_SHARED_SECRET_ENV: &str = "de_s2s_shared_secret";
pub const S2S_SHARED_SECRET_MIN_LENGTH: usize = 16;

/// Like Node's Buffer.from(x, 'base64'): padding optional.
const STANDARD_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// True when the string looks like a shipped placeholder rather than a real
/// secret. A placeholder value in config must fail closed — a shipped default
/// is a public credential.
pub fn is_placeholder_secret(p_value: Option<&str>) -> bool {
    let Some(v) = p_value else { return true };
    let v = v.trim().to_lowercase();
    if v.is_empty() {
        return true;
    }
    const PLACEHOLDERS: [&str; 10] = [
        "change_me",
        "changeme",
        "change-me",
        "change this",
        "placeholder",
        "your-secret",
        "your_secret",
        "todo",
        "secret",
        "xxx",
    ];
    PLACEHOLDERS.contains(&v.as_str())
}

/// Generates a random nonce (challenge): 32 random bytes, hex.
pub fn generate_nonce() -> String {
    let mut v_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut v_bytes);
    v_bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// HMAC-SHA256(nonce, secret), base64.
pub fn hmac_with(p_secret: &str, p_nonce: &str) -> String {
    let mut v_mac = Hmac::<Sha256>::new_from_slice(p_secret.as_bytes()).expect("HMAC accepts any key length");
    v_mac.update(p_nonce.as_bytes());
    STANDARD.encode(v_mac.finalize().into_bytes())
}

/// Ed25519 signature of the nonce, base64.
pub fn sign_with(p_key: &SigningKey, p_nonce: &str) -> String {
    STANDARD.encode(p_key.sign(p_nonce.as_bytes()).to_bytes())
}

/// Builds the challenge envelope (sent by the accepting side).
pub fn build_challenge(p_nonce: &str, p_method: Option<&str>) -> String {
    let mut v_obj = json!({ "s2s_auth": S2S_AUTH_CHALLENGE, "nonce": p_nonce });
    if let Some(m) = p_method {
        v_obj["method"] = json!(m);
    }
    crate::util::js_stringify(&v_obj)
}

/// Parses a raw WS message into an S2S auth envelope. None if the message is
/// not an S2S auth message (e.g. normal traffic, or a binary payload).
pub fn parse_envelope(p_msg: &[u8]) -> Option<Value> {
    if p_msg.first() != Some(&b'{') {
        return None;
    }
    let v_obj = crate::util::js_parse_bytes(p_msg)?;
    match v_obj.get("s2s_auth") {
        None | Some(Value::Null) => None,
        Some(_) => Some(v_obj),
    }
}

/// The S2S credentials of this server, loaded lazily from the configuration.
pub struct S2sAuth {
    m_config: Config,
    m_env_secret: Option<String>,
    m_private_key: Mutex<Option<SigningKey>>,
    m_public_keys: Mutex<Option<HashMap<String, VerifyingKey>>>,
    m_shared_secret: Mutex<Option<String>>,
}

impl S2sAuth {
    /// @param p_env_secret the de_s2s_shared_secret environment variable, if set
    pub fn new(p_config: &Config, p_env_secret: Option<String>) -> S2sAuth {
        S2sAuth {
            m_config: p_config.clone(),
            m_env_secret: p_env_secret,
            m_private_key: Mutex::new(None),
            m_public_keys: Mutex::new(None),
            m_shared_secret: Mutex::new(None),
        }
    }

    /// The Ed25519 PRIVATE key used to sign challenges.
    pub fn private_key(&self) -> Result<SigningKey, String> {
        let mut v_cache = self.m_private_key.lock().unwrap();
        if let Some(k) = v_cache.as_ref() {
            return Ok(k.clone());
        }
        let Some(v_file) = self.m_config.string("s2s_my_private_key") else {
            return Err("s2s_my_private_key is not configured".into());
        };
        let v_path = self.m_config.resolve(&v_file);
        let v_pem = std::fs::read_to_string(&v_path)
            .map_err(|e| format!("ENOENT: no such file or directory, open '{}' ({})", v_path.display(), e))?;
        let v_key = SigningKey::from_pkcs8_pem(&v_pem)
            .map_err(|e| format!("invalid private key {}: {}", v_path.display(), e))?;
        *v_cache = Some(v_key.clone());
        Ok(v_key)
    }

    /// The Ed25519 PUBLIC keys (server_id -> key) used to verify responses.
    pub fn public_keys(&self) -> Result<HashMap<String, VerifyingKey>, String> {
        let mut v_cache = self.m_public_keys.lock().unwrap();
        if let Some(k) = v_cache.as_ref() {
            return Ok(k.clone());
        }
        let v_keys = match self.m_config.get("s2s_trusted_server_keys") {
            None | Some(Value::Null) => return Err("s2s_trusted_server_keys is not configured".into()),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => Map::new(),
        };
        let mut v_out = HashMap::new();
        for (v_id, v_file) in v_keys {
            let v_path = self.m_config.resolve(&js_to_string(&v_file));
            let v_pem =
                std::fs::read_to_string(&v_path).map_err(|e| format!("cannot read {}: {}", v_path.display(), e))?;
            let v_key = VerifyingKey::from_public_key_pem(&v_pem)
                .map_err(|e| format!("invalid public key {}: {}", v_path.display(), e))?;
            v_out.insert(v_id, v_key);
        }
        *v_cache = Some(v_out.clone());
        Ok(v_out)
    }

    /// The shared secret for HMAC mode, from de_s2s_shared_secret or the
    /// s2s_shared_secret_file (never generated here: a locally generated
    /// secret would never match the accepting server's).
    pub fn shared_secret(&self) -> Result<String, String> {
        let mut v_cache = self.m_shared_secret.lock().unwrap();
        if let Some(s) = v_cache.as_ref() {
            return Ok(s.clone());
        }
        let v_secret = if let Some(v_env) = &self.m_env_secret {
            v_env.trim().to_string()
        } else {
            let Some(v_file) = self.m_config.string("s2s_shared_secret_file") else {
                return Err(format!(
                    "S2S shared-secret mode requested but no secret is configured. Set {} or s2s_shared_secret_file.",
                    S2S_SHARED_SECRET_ENV
                ));
            };
            let v_path = self.m_config.resolve(&v_file);
            if !v_path.exists() {
                return Err(format!("S2S shared secret file not found: {}", v_path.display()));
            }
            std::fs::read_to_string(&v_path).map_err(|e| e.to_string())?.trim().to_string()
        };
        if is_placeholder_secret(Some(&v_secret)) {
            return Err(
                "S2S shared secret is missing or a placeholder value — refusing to run insecurely (fail closed)".into(),
            );
        }
        if v_secret.chars().count() < S2S_SHARED_SECRET_MIN_LENGTH {
            return Err(format!(
                "S2S shared secret too short (min {} chars) — refusing to run insecurely (fail closed)",
                S2S_SHARED_SECRET_MIN_LENGTH
            ));
        }
        *v_cache = Some(v_secret.clone());
        Ok(v_secret)
    }

    fn nonce_text(p_nonce: Option<&Value>) -> Result<String, String> {
        match p_nonce {
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err("The first argument must be of type string".into()),
        }
    }

    fn response(&self, p_sig: String) -> String {
        let mut v_obj = Map::new();
        v_obj.insert("s2s_auth".into(), json!(S2S_AUTH_RESPONSE));
        v_obj.insert("sig".into(), json!(p_sig));
        if let Some(v_id) = self.m_config.get("server_id") {
            v_obj.insert("id".into(), v_id.clone());
        }
        crate::util::js_stringify(&Value::Object(v_obj))
    }

    /// The Ed25519 response envelope for a challenge nonce (fn_buildResponse).
    pub fn build_response(&self, p_nonce: Option<&Value>) -> Result<String, String> {
        let v_nonce = Self::nonce_text(p_nonce)?;
        Ok(self.response(sign_with(&self.private_key()?, &v_nonce)))
    }

    /// The HMAC response envelope for a challenge nonce (fn_buildHmacResponse).
    pub fn build_hmac_response(&self, p_nonce: Option<&Value>) -> Result<String, String> {
        let v_nonce = js_key(p_nonce);
        Ok(self.response(hmac_with(&self.shared_secret()?, &v_nonce)))
    }

    /// The response for a challenge envelope, honouring its `method`
    /// (absent or anything but hmac-sha256 = Ed25519).
    pub fn answer_challenge(&self, p_env: &Value) -> Result<String, String> {
        if p_env.get("method").and_then(Value::as_str) == Some(S2S_AUTH_METHOD_HMAC) {
            self.build_hmac_response(p_env.get("nonce"))
        } else {
            self.build_response(p_env.get("nonce"))
        }
    }

    /// Verifies a base64 Ed25519 signature of the nonce with p_server_id's
    /// trusted key (fn_verify). Any error is a failed verification.
    pub fn verify(&self, p_nonce: &str, p_signature: Option<&Value>, p_server_id: Option<&Value>) -> bool {
        let (Some(v_sig), Some(v_id)) = (p_signature.filter(|v| !v.is_null()), p_server_id.filter(|v| !v.is_null()))
        else {
            return false;
        };
        let v_keys = match self.public_keys() {
            Ok(k) => k,
            Err(e) => {
                println!("{}S2S verify error: {}{}", crate::colors::ERROR, e, crate::colors::RESET);
                return false;
            }
        };
        let v_id = js_to_string(v_id);
        let Some(v_key) = v_keys.get(&v_id) else {
            println!("{}S2S verify error: unknown server_id {}{}", crate::colors::ERROR, v_id, crate::colors::RESET);
            return false;
        };
        let Ok(v_bytes) = STANDARD_LENIENT.decode(js_to_string(v_sig).trim()) else {
            return false;
        };
        let Ok(v_sig) = Signature::from_slice(&v_bytes) else {
            return false;
        };
        v_key.verify_strict(p_nonce.as_bytes(), &v_sig).is_ok()
    }

    /// Verifies a base64 HMAC response in constant time (fn_verifyHmac).
    pub fn verify_hmac(&self, p_nonce: &str, p_hmac: Option<&Value>) -> bool {
        let Some(v_hmac) = p_hmac.filter(|v| !v.is_null()) else { return false };
        let Ok(v_secret) = self.shared_secret() else { return false };
        let v_expected = hmac_with(&v_secret, p_nonce);
        let v_actual = js_to_string(v_hmac);
        v_expected.len() == v_actual.len() && bool::from(v_expected.as_bytes().ct_eq(v_actual.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};

    fn keys_config(p_dir: &std::path::Path) -> (Config, SigningKey) {
        let v_key = SigningKey::generate(&mut rand::rngs::OsRng);
        std::fs::write(p_dir.join("me_private.pem"), v_key.to_pkcs8_pem(Default::default()).unwrap().as_bytes())
            .unwrap();
        std::fs::write(
            p_dir.join("X_public.pem"),
            v_key.verifying_key().to_public_key_pem(Default::default()).unwrap(),
        )
        .unwrap();
        let v_cfg = Config::from_value(
            json!({ "server_id": "X", "s2s_my_private_key": "./me_private.pem", "s2s_trusted_server_keys": { "X": "X_public.pem" } }),
            p_dir,
        );
        (v_cfg, v_key)
    }

    #[test]
    fn nonce_challenge_and_envelopes() {
        let n = generate_nonce();
        assert_eq!(n.len(), 64);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(n, generate_nonce());
        assert_eq!(build_challenge("abc", None), r#"{"s2s_auth":"challenge","nonce":"abc"}"#);
        assert!(parse_envelope(br#"{"s2s_auth":"challenge","nonce":"n"}"#).is_some());
        assert!(parse_envelope(br#"{"mt":1}"#).is_none());
        assert!(parse_envelope(b"\x01\x02").is_none());
    }

    #[test]
    fn sign_and_verify_with_matching_keys() {
        let v_dir = tempfile::tempdir().unwrap();
        let (v_cfg, v_key) = keys_config(v_dir.path());
        let v_auth = S2sAuth::new(&v_cfg, None);
        let v_resp: Value = serde_json::from_str(&v_auth.build_response(Some(&json!("nonce-1"))).unwrap()).unwrap();
        assert_eq!(v_resp["s2s_auth"], "response");
        assert_eq!(v_resp["id"], "X");
        assert!(v_auth.verify("nonce-1", v_resp.get("sig"), Some(&json!("X"))));
        assert!(!v_auth.verify("nonce-2", v_resp.get("sig"), Some(&json!("X"))));
        assert!(!v_auth.verify("nonce-1", v_resp.get("sig"), Some(&json!("Z"))));
        assert!(!v_auth.verify("nonce-1", None, Some(&json!("X"))));
        let v_other = SigningKey::generate(&mut rand::rngs::OsRng);
        assert!(!v_auth.verify("n", Some(&json!(sign_with(&v_other, "n"))), Some(&json!("X"))));
        assert!(v_auth.verify("n", Some(&json!(sign_with(&v_key, "n"))), Some(&json!("X"))));
    }

    #[test]
    fn missing_keys_and_secrets_fail() {
        let v_auth = S2sAuth::new(&Config::from_value(json!({}), "."), None);
        assert_eq!(v_auth.private_key().unwrap_err(), "s2s_my_private_key is not configured");
        assert_eq!(v_auth.public_keys().unwrap_err(), "s2s_trusted_server_keys is not configured");
        assert!(v_auth.shared_secret().unwrap_err().contains("no secret is configured"));
        assert!(S2sAuth::new(&Config::from_value(json!({}), "."), Some("tiny".into()))
            .shared_secret()
            .unwrap_err()
            .contains("too short"));
        let v_hmac =
            S2sAuth::new(&Config::from_value(json!({"server_id": "C"}), "."), Some("a-long-enough-secret-123".into()));
        let v_env = json!({"s2s_auth": "challenge", "nonce": "n1", "method": "hmac-sha256"});
        let v_resp: Value = serde_json::from_str(&v_hmac.answer_challenge(&v_env).unwrap()).unwrap();
        assert_eq!(v_resp["sig"], hmac_with("a-long-enough-secret-123", "n1"));
        assert!(v_hmac.verify_hmac("n1", v_resp.get("sig")));
        // Ed25519 is the default when the challenge has no method
        assert!(v_hmac.answer_challenge(&json!({"s2s_auth": "challenge", "nonce": "n1"})).is_err());
    }
}
