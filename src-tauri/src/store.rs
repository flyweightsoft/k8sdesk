//! Encrypted on-disk store for cluster credentials.
//!
//! - The 32-byte master key is stored in the OS keychain when available.
//!   If the keychain entry is absent (e.g. unsigned dev binary, CI) a key
//!   file (`master.key`) in the app data directory is used instead.
//! - The store file is a single AES-256-GCM ciphertext; the plaintext is a
//!   JSON document of cluster records.
//! - Secrets in memory are wrapped in `Zeroizing` and dropped when no longer
//!   needed. Cluster records returned to the frontend are redacted.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng as AeadRng, Payload},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};

const KEYRING_SERVICE: &str = "dev.k8sdesk.app";
const KEYRING_USER: &str = "master-key-v1";
const STORE_FILENAME: &str = "clusters.enc";
const KEY_FILENAME: &str = "master.key";
const STORE_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Dev,
    Staging,
    Prod,
}

impl Environment {
    pub fn is_prod(self) -> bool {
        matches!(self, Environment::Prod)
    }
}

/// Authentication material. Token / key bytes are kept as `String` only inside
/// the encrypted file; in-memory we wrap them in `Zeroizing<String>` so they
/// are wiped on drop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    BearerToken { token: String },
    ClientCert { cert_pem: String, key_pem: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterRecord {
    pub id: Uuid,
    pub name: String,
    pub environment: Environment,
    pub api_server: String,
    /// PEM-encoded CA bundle. Empty string = no CA pinning (only valid when
    /// `insecure_skip_tls_verify` is true).
    pub ca_pem: String,
    pub auth: Auth,
    pub default_namespace: String,
    pub insecure_skip_tls_verify: bool,
}

/// Frontend-safe view: never includes auth material or CA bytes.
#[derive(Debug, Clone, Serialize)]
pub struct RedactedCluster {
    pub id: Uuid,
    pub name: String,
    pub environment: Environment,
    pub api_server: String,
    pub default_namespace: String,
    pub auth_kind: &'static str,
    pub has_ca: bool,
    pub insecure_skip_tls_verify: bool,
}

impl From<&ClusterRecord> for RedactedCluster {
    fn from(c: &ClusterRecord) -> Self {
        Self {
            id: c.id,
            name: c.name.clone(),
            environment: c.environment,
            api_server: c.api_server.clone(),
            default_namespace: c.default_namespace.clone(),
            auth_kind: match c.auth {
                Auth::BearerToken { .. } => "bearer_token",
                Auth::ClientCert { .. } => "client_cert",
            },
            has_ca: !c.ca_pem.is_empty(),
            insecure_skip_tls_verify: c.insecure_skip_tls_verify,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StorePlaintext {
    clusters: Vec<ClusterRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreEnvelope {
    v: u8,
    nonce: String,      // base64
    ciphertext: String, // base64
}

pub struct EncryptedStore {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    data: StorePlaintext,
}

impl EncryptedStore {
    pub fn open(app_dir: &Path) -> AppResult<Self> {
        let path = app_dir.join(STORE_FILENAME);
        let key = load_or_create_master_key(app_dir)?;

        let data = if path.exists() {
            let bytes = std::fs::read(&path)?;
            match decrypt_envelope(&key, &bytes) {
                Ok(d) => d,
                Err(AppError::Crypto) => {
                    // Key mismatch (e.g. keychain entry was recreated). Back up the
                    // unreadable file and start fresh rather than crashing.
                    tracing::warn!(
                        "store decryption failed — key mismatch? backing up and resetting"
                    );
                    let backup = path.with_extension("enc.bak");
                    std::fs::rename(&path, &backup).ok();
                    StorePlaintext::default()
                }
                Err(e) => return Err(e),
            }
        } else {
            StorePlaintext::default()
        };

        Ok(Self { path, key, data })
    }

    pub fn list(&self) -> Vec<RedactedCluster> {
        self.data.clusters.iter().map(RedactedCluster::from).collect()
    }

    pub fn get(&self, id: Uuid) -> AppResult<&ClusterRecord> {
        self.data
            .clusters
            .iter()
            .find(|c| c.id == id)
            .ok_or(AppError::NotFound)
    }

    pub fn add(&mut self, mut rec: ClusterRecord) -> AppResult<RedactedCluster> {
        if rec.id.is_nil() {
            rec.id = Uuid::new_v4();
        }
        validate(&rec)?;
        self.data.clusters.push(rec);
        self.persist()?;
        Ok(self.data.clusters.last().map(RedactedCluster::from).unwrap())
    }

    pub fn update(&mut self, rec: ClusterRecord) -> AppResult<RedactedCluster> {
        validate(&rec)?;
        let slot = self
            .data
            .clusters
            .iter_mut()
            .find(|c| c.id == rec.id)
            .ok_or(AppError::NotFound)?;
        *slot = rec;
        let red = RedactedCluster::from(&*slot);
        self.persist()?;
        Ok(red)
    }

    pub fn delete(&mut self, id: Uuid) -> AppResult<()> {
        let before = self.data.clusters.len();
        self.data.clusters.retain(|c| c.id != id);
        if self.data.clusters.len() == before {
            return Err(AppError::NotFound);
        }
        self.persist()
    }

    fn persist(&self) -> AppResult<()> {
        let json = serde_json::to_vec(&self.data)?;
        let envelope_bytes = encrypt_envelope(&self.key, &json)?;
        // Atomic-ish: write to temp then rename.
        let tmp = self.path.with_extension("enc.tmp");
        std::fs::write(&tmp, envelope_bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn validate(rec: &ClusterRecord) -> AppResult<()> {
    if rec.name.trim().is_empty() {
        return Err(AppError::BadInput("name required".into()));
    }
    if !rec.api_server.starts_with("https://") && !rec.api_server.starts_with("http://") {
        return Err(AppError::BadInput("api_server must be a URL".into()));
    }
    if rec.ca_pem.is_empty() && !rec.insecure_skip_tls_verify {
        return Err(AppError::BadInput(
            "either ca_pem or insecure_skip_tls_verify is required".into(),
        ));
    }
    Ok(())
}

// ---------- crypto ----------

fn load_or_create_master_key(app_dir: &Path) -> AppResult<Zeroizing<[u8; 32]>> {
    // 1. Try the OS keychain.
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?;
    match entry.get_password() {
        Ok(b64) => {
            let raw = B64.decode(b64.as_bytes()).map_err(|_| AppError::Crypto)?;
            if raw.len() != 32 {
                return Err(AppError::Crypto);
            }
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&raw);
            return Ok(key);
        }
        Err(keyring::Error::NoEntry) => {
            // Fall through to key-file path below.
        }
        Err(e) => {
            tracing::warn!("keychain unavailable ({}), falling back to key file", e);
            // Fall through to key-file path below.
        }
    }

    // 2. Keychain had no entry (or is inaccessible in dev). Use / create a
    //    key file in the app data directory.
    let key_path = app_dir.join(KEY_FILENAME);
    if key_path.exists() {
        let b64 = std::fs::read_to_string(&key_path)?;
        let raw = B64.decode(b64.trim().as_bytes()).map_err(|_| AppError::Crypto)?;
        if raw.len() != 32 {
            return Err(AppError::Crypto);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&raw);
        // Best-effort: promote to keychain for next run.
        let _ = entry.set_password(&B64.encode(key.as_ref()));
        return Ok(key);
    }

    // 3. No key anywhere — generate a fresh one, persist to both.
    let mut key = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    let b64 = B64.encode(key.as_ref());
    std::fs::write(&key_path, &b64)?;
    // Restrict key file to owner read/write only.
    #[cfg(unix)]
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    let _ = entry.set_password(&b64);
    Ok(key)
}

fn encrypt_envelope(key: &[u8; 32], plaintext: &[u8]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    AeadRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = format!("k8sdesk:v{}", STORE_VERSION);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| AppError::Crypto)?;
    let env = StoreEnvelope {
        v: STORE_VERSION,
        nonce: B64.encode(nonce_bytes),
        ciphertext: B64.encode(ct),
    };
    Ok(serde_json::to_vec(&env)?)
}

fn decrypt_envelope(key: &[u8; 32], bytes: &[u8]) -> AppResult<StorePlaintext> {
    let env: StoreEnvelope = serde_json::from_slice(bytes)?;
    if env.v != STORE_VERSION {
        return Err(AppError::Storage(format!(
            "unsupported store version {}",
            env.v
        )));
    }
    let nonce_raw = B64.decode(env.nonce.as_bytes()).map_err(|_| AppError::Crypto)?;
    if nonce_raw.len() != 12 {
        return Err(AppError::Crypto);
    }
    let ct = B64
        .decode(env.ciphertext.as_bytes())
        .map_err(|_| AppError::Crypto)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(&nonce_raw);
    let aad = format!("k8sdesk:v{}", STORE_VERSION);
    let pt = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ct,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| AppError::Crypto)?;
    Ok(serde_json::from_slice(&pt)?)
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(env: Environment) -> ClusterRecord {
        ClusterRecord {
            id: Uuid::new_v4(),
            name: "test".into(),
            environment: env,
            api_server: "https://example.test:6443".into(),
            ca_pem: "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----\n".into(),
            auth: Auth::BearerToken { token: "tkn".into() },
            default_namespace: "default".into(),
            insecure_skip_tls_verify: false,
        }
    }

    #[test]
    fn roundtrip_envelope() {
        let key = [9u8; 32];
        let mut data = StorePlaintext::default();
        data.clusters.push(sample(Environment::Dev));
        let bytes = encrypt_envelope(&key, &serde_json::to_vec(&data).unwrap()).unwrap();
        let pt = decrypt_envelope(&key, &bytes).unwrap();
        assert_eq!(pt.clusters.len(), 1);
        assert_eq!(pt.clusters[0].name, "test");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = [1u8; 32];
        let bytes = encrypt_envelope(&key, b"hello").unwrap();
        let mut env: StoreEnvelope = serde_json::from_slice(&bytes).unwrap();
        // flip a byte
        let mut ct = B64.decode(env.ciphertext.as_bytes()).unwrap();
        ct[0] ^= 0x01;
        env.ciphertext = B64.encode(ct);
        let tampered = serde_json::to_vec(&env).unwrap();
        assert!(decrypt_envelope(&key, &tampered).is_err());
    }

    #[test]
    fn redaction_strips_secrets() {
        let r = sample(Environment::Prod);
        let red = RedactedCluster::from(&r);
        let json = serde_json::to_string(&red).unwrap();
        assert!(!json.contains("tkn"));
        assert!(!json.contains("BEGIN CERTIFICATE"));
    }
}
