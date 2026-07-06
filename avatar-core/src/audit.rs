use rusqlite::{Connection, Result, params};
use ring::hmac;
use chrono::Utc;

pub struct AuditLogger<'a> {
    conn: &'a Connection,
    key: hmac::Key,
}

impl<'a> AuditLogger<'a> {
    /// Initializes the logger. The hmac_key should be derived from the user PIN.
    pub fn new(conn: &'a Connection, hmac_key_bytes: &[u8]) -> Self {
        let key = hmac::Key::new(hmac::HMAC_SHA256, hmac_key_bytes);
        AuditLogger { conn, key }
    }

    /// Logs an event to the audit table, automatically chaining it to the previous entry.
    pub fn log_event(&self, event_type: &str, device_id: Option<&str>, description: &str, severity: &str) -> Result<()> {
        let timestamp = Utc::now().to_rfc3339();
        
        // Get signature of the last log entry
        let previous_signature = self.get_last_signature()?;
        
        // Compute signature: HMAC(timestamp + event_type + description + previous_signature)
        let mut data_to_sign = Vec::new();
        data_to_sign.extend_from_slice(timestamp.as_bytes());
        data_to_sign.extend_from_slice(event_type.as_bytes());
        data_to_sign.extend_from_slice(description.as_bytes());
        data_to_sign.extend_from_slice(previous_signature.as_bytes());
        
        let tag = hmac::sign(&self.key, &data_to_sign);
        let signature = hex::encode(tag.as_ref());
        
        self.conn.execute(
            "INSERT INTO audit_logs (timestamp, event_type, device_id, description, severity, signature)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![timestamp, event_type, device_id, description, severity, signature],
        )?;
        
        Ok(())
    }

    /// Validates the entire audit log chain. Returns true if intact, false if tampered.
    pub fn verify_integrity(&self) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, event_type, device_id, description, severity, signature 
             FROM audit_logs ORDER BY id ASC"
        )?;
        
        let mut rows = stmt.query([])?;
        let mut previous_signature = String::from("GENESIS_SEED");
        
        while let Some(row) = rows.next()? {
            let id: i32 = row.get(0)?;
            let timestamp: String = row.get(1)?;
            let event_type: String = row.get(2)?;
            let _device_id: Option<String> = row.get(3)?;
            let description: String = row.get(4)?;
            let _severity: String = row.get(5)?;
            let stored_signature: String = row.get(6)?;
            
            // Recompute signature
            let mut data_to_sign = Vec::new();
            data_to_sign.extend_from_slice(timestamp.as_bytes());
            data_to_sign.extend_from_slice(event_type.as_bytes());
            data_to_sign.extend_from_slice(description.as_bytes());
            data_to_sign.extend_from_slice(previous_signature.as_bytes());
            
            let tag = hmac::sign(&self.key, &data_to_sign);
            let computed_signature = hex::encode(tag.as_ref());
            
            if computed_signature != stored_signature {
                // Verification failed at this log ID
                log::error!("Audit log verification failed at entry ID: {}", id);
                return Ok(false);
            }
            
            previous_signature = stored_signature;
        }
        
        Ok(true)
    }

    /// Retrieves the signature of the last log entry, or returns "GENESIS_SEED" if empty.
    fn get_last_signature(&self) -> Result<String> {
        let mut stmt = self.conn.prepare("SELECT signature FROM audit_logs ORDER BY id DESC LIMIT 1")?;
        let mut rows = stmt.query([])?;
        
        if let Some(row) = rows.next()? {
            let sig: String = row.get(0)?;
            Ok(sig)
        } else {
            Ok(String::from("GENESIS_SEED"))
        }
    }
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }
}
