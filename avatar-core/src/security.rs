use rcgen::{Certificate, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, SanType};
use ring::digest::{digest, SHA256};
use std::error::Error;
use std::path::Path;
use std::time::SystemTime;
use uuid::Uuid;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
    Argon2,
};

pub struct GeneratedCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: String,
}

/// Generates a self-signed certificate and private key.
/// Configures proper Distinguished Name (DN) and Subject Alternative Names (SANs).
pub fn generate_self_signed_cert(common_name: &str) -> Result<GeneratedCert, Box<dyn Error>> {
    let mut params = CertificateParams::default();
    
    // Configure validity period: 10 years
    let start = SystemTime::now();
    let end = start + std::time::Duration::from_secs(365 * 24 * 60 * 60 * 10);
    params.not_before = rcgen::date_time_ymd(2026, 1, 1); // fixed start time or dynamic
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    
    // Set DN
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    dn.push(DnType::OrganizationName, "Avatar Private Ecosystem");
    params.distinguished_name = dn;
    
    // Subject Alternative Names (SANs)
    params.subject_alt_names = vec![
        SanType::DnsName(rcgen::Ia5String::try_from("localhost").unwrap()),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
    ];
    
    // Key settings
    params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    
    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    
    let fingerprint = calculate_sha256_fingerprint(&cert_pem)?;
    
    Ok(GeneratedCert {
        cert_pem,
        key_pem,
        fingerprint,
    })
}

/// Computes the SHA-256 fingerprint of a PEM certificate in colon-separated hex format.
pub fn calculate_sha256_fingerprint(cert_pem: &str) -> Result<String, Box<dyn Error>> {
    // Decode PEM to DER
    let lines: Vec<&str> = cert_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    
    let der_base64 = lines.concat();
    let der_bytes = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, der_base64)?;
    
    let hash = digest(&SHA256, &der_bytes);
    let hex_fingerprint: String = hash
        .as_ref()
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<String>>()
        .join(":");
    
    Ok(hex_fingerprint)
}

/// Helper to generate a new unique session pairing token for QR code display.
pub fn generate_pairing_token() -> String {
    let uuid = Uuid::new_v4();
    let random_part = Uuid::new_v4().simple().to_string();
    format!("{}-{}", uuid, &random_part[..8])
}

/// Verifies a passcode against a stored Argon2 hash file, or creates one on first setup.
pub fn verify_or_init_passcode(auth_path: &Path, passcode: &str) -> Result<(), Box<dyn Error>> {
    let argon2 = Argon2::default();

    if auth_path.exists() {
        let stored = std::fs::read_to_string(auth_path)?;
        let parsed = PasswordHash::new(stored.trim())
            .map_err(|_| "Invalid stored passcode hash")?;
        argon2
            .verify_password(passcode.as_bytes(), &parsed)
            .map_err(|_| "Invalid PIN/Passcode")?;
        return Ok(());
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2
        .hash_password(passcode.as_bytes(), &salt)
        .map_err(|e| format!("Failed to hash passcode: {}", e))?;
    std::fs::write(auth_path, hash.to_string())?;
    Ok(())
}

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

/// Standalone helper to encrypt a string using AES-256-GCM.
/// Returns base64 encoded string containing `nonce || ciphertext || tag`.
pub fn encrypt_aes_gcm(key: &[u8; 32], plaintext: &str) -> Result<String, Box<dyn Error>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| format!("AES key init failed: {}", e))?;
        
    // Generate random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    
    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("AES encryption failed: {}", e))?;
        
    // Prepend nonce to ciphertext
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    
    let encoded = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, combined);
    Ok(encoded)
}

/// Standalone helper to decrypt a base64 encoded string using AES-256-GCM.
pub fn decrypt_aes_gcm(key: &[u8; 32], ciphertext_b64: &str) -> Result<String, Box<dyn Error>> {
    let combined = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, ciphertext_b64)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;
        
    if combined.len() < 12 {
        return Err("Invalid encrypted payload (too short)".into());
    }
    
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| format!("AES key init failed: {}", e))?;
        
    let decrypted_bytes = cipher.decrypt(nonce, ciphertext)
        .map_err(|e| format!("AES decryption failed: {}", e))?;
        
    let plaintext = String::from_utf8(decrypted_bytes)
        .map_err(|e| format!("Invalid UTF-8 payload: {}", e))?;
        
    Ok(plaintext)
}

/// Reads discovery_keys.json and publishes target to all keys on keyvalue.immanuel.co.
pub async fn publish_discovery_keys(data_dir: &std::path::Path, target: &str) {
    let keys_file = data_dir.join("discovery_keys.json");
    if !keys_file.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&keys_file) {
        Ok(c) => c,
        Err(_) => return,
    };
    let keys: Vec<String> = match serde_json::from_str(&content) {
        Ok(k) => k,
        Err(_) => return,
    };

    let client = reqwest::Client::new();
    for key in keys {
        let kv_target = target.replace(':', "_");
        let url = format!("https://keyvalue.immanuel.co/api/KeyVal/UpdateValue/0gcpgxva/{}/{}/", key, kv_target);
        match client.post(&url).header("content-length", "0").send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                log::info!("Published KV for key: {}, Status: {}, Body: {}", key, status, body.trim());
            }
            Err(e) => log::warn!("Failed to publish connection target to KV: {}", e),
        }
    }
}

/// Clears values for all saved discovery keys on keyvalue.immanuel.co.
pub async fn clear_discovery_keys(data_dir: &std::path::Path) {
    let keys_file = data_dir.join("discovery_keys.json");
    if !keys_file.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&keys_file) {
        Ok(c) => c,
        Err(_) => return,
    };
    let keys: Vec<String> = match serde_json::from_str(&content) {
        Ok(k) => k,
        Err(_) => return,
    };

    let client = reqwest::Client::new();
    for key in keys {
        let url = format!("https://keyvalue.immanuel.co/api/KeyVal/UpdateValue/0gcpgxva/{}/offline/", key);
        let _ = client.post(&url).header("content-length", "0").send().await;
    }
}

/// Standalone helper to encrypt binary bytes using AES-256-GCM.
/// Returns a byte array containing `nonce || ciphertext || tag`.
pub fn encrypt_aes_gcm_bin(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| format!("AES key init failed: {}", e))?;
        
    let mut nonce_bytes = [0u8; 12];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    
    let ciphertext = cipher.encrypt(nonce, plaintext)
        .map_err(|e| format!("AES encryption failed: {}", e))?;
        
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(combined)
}

/// Standalone helper to decrypt binary bytes using AES-256-GCM.
pub fn decrypt_aes_gcm_bin(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    if ciphertext.len() < 12 {
        return Err("Invalid encrypted payload (too short)".into());
    }
    
    let (nonce_bytes, payload) = ciphertext.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| format!("AES key init failed: {}", e))?;
        
    let decrypted_bytes = cipher.decrypt(nonce, payload)
        .map_err(|e| format!("AES decryption failed: {}", e))?;
        
    Ok(decrypted_bytes)
}
