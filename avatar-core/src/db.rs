use rusqlite::{Connection, Result};
use std::path::Path;
use argon2::{
    password_hash::{rand_core::OsRng as ArgonOsRng, SaltString},
    Argon2, PasswordHasher,
};
use ring::digest::{digest, SHA256};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use std::error::Error;
use std::sync::{Mutex, MutexGuard};

pub struct DbManager {
    conn: Mutex<Connection>,
    key: [u8; 32], // Retained in RAM to encrypt/decrypt fields
}

impl DbManager {
    /// Opens the SQLite database and initializes the key derived from the passcode.
    pub fn open_encrypted<P: AsRef<Path>>(db_path: P, passphrase: &str, salt: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let derived_key = Self::derive_key(passphrase, salt);
        
        let db = DbManager { 
            conn: Mutex::new(conn), 
            key: derived_key 
        };
        db.initialize_schema()?;
        Ok(db)
    }

    /// Derives a 32-byte key from a passphrase and a salt using Argon2id.
    pub fn derive_key(passphrase: &str, salt: &str) -> [u8; 32] {
        let salt_hash = digest(&SHA256, salt.as_bytes());
        let salt_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, salt_hash.as_ref());
        let parsed_salt = SaltString::new(&salt_b64).unwrap_or_else(|_| SaltString::from_b64("c2FsdHNhbHQ").unwrap());
        
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(passphrase.as_bytes(), &parsed_salt)
            .expect("Failed to hash passcode using Argon2id");
        
        let mut key = [0u8; 32];
        if let Some(hash) = password_hash.hash {
            let bytes = hash.as_bytes();
            let len = std::cmp::min(bytes.len(), 32);
            key[..len].copy_from_slice(&bytes[..len]);
        }
        key
    }

    /// Generates a new random salt string.
    pub fn generate_salt() -> String {
        let salt = SaltString::generate(&mut ArgonOsRng);
        salt.to_string()
    }

    /// Application-level encryption using AES-256-GCM.
    /// Returns a base64 encoded string containing `nonce || ciphertext`.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, Box<dyn Error>> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| format!("AES key init failed: {}", e))?;
            
        // Generate random 12-byte nonce
        let mut nonce_bytes = [0u8; 12];
        let mut rng = aes_gcm::aead::rand_core::OsRng;
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

    /// Application-level decryption using AES-256-GCM.
    pub fn decrypt(&self, encrypted_base64: &str) -> Result<String, Box<dyn Error>> {
        let combined = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, encrypted_base64)
            .map_err(|e| format!("Base64 decode failed: {}", e))?;
            
        if combined.len() < 12 {
            return Err("Invalid encrypted payload (too short)".into());
        }
        
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| format!("AES key init failed: {}", e))?;
            
        let decrypted_bytes = cipher.decrypt(nonce, ciphertext)
            .map_err(|e| format!("AES decryption failed: {}", e))?;
            
        let plaintext = String::from_utf8(decrypted_bytes)
            .map_err(|e| format!("Invalid UTF-8 payload: {}", e))?;
            
        Ok(plaintext)
    }

    /// Runs migrations to initialize database tables.
    fn initialize_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS devices (
                 device_id TEXT PRIMARY KEY,
                 device_name TEXT NOT NULL,
                 public_key_pem TEXT NOT NULL,
                 certificate_pem TEXT NOT NULL,
                 fingerprint TEXT NOT NULL,
                 is_approved INTEGER NOT NULL DEFAULT 0,
                 paired_at TEXT NOT NULL,
                 last_seen TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS sessions (
                 session_id TEXT PRIMARY KEY,
                 device_id TEXT NOT NULL,
                 token_hash TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 expires_at TEXT NOT NULL,
                 FOREIGN KEY(device_id) REFERENCES devices(device_id) ON DELETE CASCADE
             );

             CREATE TABLE IF NOT EXISTS memory_session (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 role TEXT NOT NULL,
                 message TEXT NOT NULL,          -- Encrypted with AES-256-GCM
                 timestamp TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS memory_short_term (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL,            -- Encrypted with AES-256-GCM
                 expires_at TEXT,
                 updated_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS memory_long_term (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 category TEXT NOT NULL,
                 fact TEXT NOT NULL,              -- Encrypted with AES-256-GCM
                 confidence REAL NOT NULL DEFAULT 1.0,
                 created_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS audit_logs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp TEXT NOT NULL,
                 event_type TEXT NOT NULL,
                 device_id TEXT,
                 description TEXT NOT NULL,       -- Encrypted with AES-256-GCM
                 severity TEXT NOT NULL,
                 signature TEXT NOT NULL
             );"
        )
    }

    /// Exposes a borrow to the connection, wrapped inside a MutexGuard.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}
