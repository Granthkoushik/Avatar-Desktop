use rusqlite::{params, Row};
use chrono::{Utc, Duration};
use crate::db::DbManager;
use crate::ai::ChatMessage;
use std::error::Error;

pub struct MemoryManager<'a> {
    db: &'a DbManager,
}

impl<'a> MemoryManager<'a> {
    pub fn new(db: &'a DbManager) -> Self {
        MemoryManager { db }
    }

    // --- Session Memory (Chat History) ---

    /// Encrypts and adds a message to the conversation session log.
    pub fn save_message(&self, session_id: &str, role: &str, message: &str) -> Result<(), Box<dyn Error>> {
        let encrypted_message = self.db.encrypt(message)?;
        let timestamp = Utc::now().to_rfc3339();
        
        self.db.conn().execute(
            "INSERT INTO memory_session (session_id, role, message, timestamp) 
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, encrypted_message, timestamp],
        )?;
        Ok(())
    }

    /// Fetches the recent messages for a session and decrypts them.
    pub fn get_conversation_history(&self, session_id: &str, limit: usize) -> Result<Vec<ChatMessage>, Box<dyn Error>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "SELECT role, message FROM memory_session 
             WHERE session_id = ?1 
             ORDER BY id ASC LIMIT ?2"
        )?;
        
        let rows = stmt.query_map(params![session_id, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut history = Vec::new();
        for r in rows {
            let (role, encrypted_message) = r?;
            let decrypted_message = self.db.decrypt(&encrypted_message)?;
            history.push(ChatMessage {
                role,
                content: decrypted_message,
            });
        }
        Ok(history)
    }

    // --- Short-Term Memory (Ephemeral Cache) ---

    /// Encrypts and caches a key-value pair. Expires after `duration_secs` if specified.
    pub fn set_short_term(&self, key: &str, value: &str, duration_secs: Option<i64>) -> Result<(), Box<dyn Error>> {
        let encrypted_value = self.db.encrypt(value)?;
        let now = Utc::now();
        let expires_at = duration_secs.map(|s| (now + Duration::seconds(s)).to_rfc3339());
        let updated_at = now.to_rfc3339();

        self.db.conn().execute(
            "INSERT INTO memory_short_term (key, value, expires_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET 
                value = excluded.value, 
                expires_at = excluded.expires_at, 
                updated_at = excluded.updated_at",
            params![key, encrypted_value, expires_at, updated_at],
        )?;
        Ok(())
    }

    /// Retrieves a cached value, checking expiration, and decrypts it. Evicts expired entries.
    pub fn get_short_term(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        let now_str = Utc::now().to_rfc3339();
        
        // Evict expired entries first
        self.db.conn().execute(
            "DELETE FROM memory_short_term WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now_str],
        )?;

        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "SELECT value FROM memory_short_term WHERE key = ?1"
        )?;

        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            let encrypted_val: String = row.get(0)?;
            let decrypted_val = self.db.decrypt(&encrypted_val)?;
            Ok(Some(decrypted_val))
        } else {
            Ok(None)
        }
    }

    // --- Long-Term Memory (Factual Records) ---

    /// Encrypts and inserts a fact about the user or system preferences.
    pub fn save_long_term_fact(&self, category: &str, fact: &str, confidence: f64) -> Result<(), Box<dyn Error>> {
        let encrypted_fact = self.db.encrypt(fact)?;
        let created_at = Utc::now().to_rfc3339();
        
        self.db.conn().execute(
            "INSERT INTO memory_long_term (category, fact, confidence, created_at) 
             VALUES (?1, ?2, ?3, ?4)",
            params![category, encrypted_fact, confidence, created_at],
        )?;
        Ok(())
    }

    /// Searches facts within long term memory and decrypts them.
    pub fn search_long_term(&self, category: Option<&str>) -> Result<Vec<String>, Box<dyn Error>> {
        let mut query = "SELECT fact FROM memory_long_term".to_string();
        let mut params_vec: Vec<String> = Vec::new();

        if let Some(cat) = category {
            query.push_str(" WHERE category = ?1");
            params_vec.push(cat.to_string());
        }
        query.push_str(" ORDER BY id DESC");

        let conn = self.db.conn();
        let mut stmt = conn.prepare(&query)?;
        
        // Use a single mapper closure variable to prevent mismatched type closure compile error
        let mapper = |row: &Row| row.get::<_, String>(0);

        let rows = if params_vec.is_empty() {
            stmt.query_map([], mapper)?
        } else {
            stmt.query_map(params![params_vec[0]], mapper)?
        };

        let mut facts = Vec::new();
        for f in rows {
            let encrypted_fact = f?;
            let decrypted_fact = self.db.decrypt(&encrypted_fact)?;
            facts.push(decrypted_fact);
        }
        Ok(facts)
    }
}
