use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};
use serde::{Serialize, Deserialize};

use avatar_core::db::DbManager;
use avatar_core::streaming::StreamingServer;
use avatar_core::system_monitor::{SystemMonitor, SystemMetrics};
use avatar_core::ai::{AiManager, ChatMessage};
use avatar_core::memory::MemoryManager;
use avatar_core::voice::VoiceManager;
use avatar_core::intent::{IntentDispatcher, ExecutionResult, extract_json_intent};
use avatar_core::security::{generate_self_signed_cert, generate_pairing_token, calculate_sha256_fingerprint, verify_or_init_passcode};

pub struct AppState {
    pub db: Arc<Mutex<Option<Arc<DbManager>>>>,
    pub streaming: Mutex<Option<Arc<StreamingServer>>>,
    pub monitor: Mutex<SystemMonitor>,
    pub ai: Arc<AiManager>,
    pub voice: Arc<VoiceManager>,
    pub dispatcher: Arc<Mutex<IntentDispatcher>>,
    pub salt: String,
    pub tunnel_url: Mutex<String>,
    pub passcode_hash: Arc<Mutex<String>>,
    pub publish_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

#[derive(Serialize)]
pub struct PairingPayload {
    pub server_ip: String,
    pub port: u16,
    pub fingerprint: String,
    pub token: String,
}

// --- Tauri Commands ---

#[tauri::command]
fn is_locked(state: State<'_, AppState>) -> bool {
    state.db.lock().unwrap().is_none()
}

#[tauri::command]
async fn unlock_database(app: AppHandle, state: State<'_, AppState>, passcode: String) -> Result<String, String> {
    log::info!("unlock_database command called");
    let mut db_lock = state.db.lock().unwrap();
    log::info!("Acquired db lock");
    if db_lock.is_some() {
        log::info!("Database already unlocked");
        return Ok("already_unlocked".to_string());
    }

    // Derive a SHA-256 hash of the passcode to be used as the session key
    log::info!("Deriving passcode session key...");
    let hash = ring::digest::digest(&ring::digest::SHA256, passcode.as_bytes());
    let passcode_hash_str = hex::encode(hash.as_ref());
    {
        let mut pass_hash = state.passcode_hash.lock().unwrap();
        *pass_hash = passcode_hash_str.clone();
    }
    
    {
        if let Some(streaming) = &*state.streaming.lock().unwrap() {
            let mut stream_pass_hash = streaming.passcode_hash.lock().unwrap();
            *stream_pass_hash = passcode_hash_str;
        }
    }
    log::info!("Passcode session key derived and stored");

    // Get app local data directory path for storing the database file safely
    let data_dir = app.path().app_local_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    log::info!("App local data directory: {:?}", data_dir);
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    
    let db_path = data_dir.join("avatar_secure.db");
    let auth_path = data_dir.join("avatar_auth.hash");
    
    // Verify passcode before opening the encrypted database vault
    log::info!("Verifying passcode against auth hash file...");
    verify_or_init_passcode(&auth_path, &passcode).map_err(|e| {
        log::error!("Passcode verification failed: {}", e);
        e.to_string()
    })?;
    log::info!("Passcode verified successfully");
    
    // Open encrypted database using derived Argon2id key
    log::info!("Opening encrypted database at {:?}...", db_path);
    match DbManager::open_encrypted(&db_path, &passcode, &state.salt) {
        Ok(db_manager) => {
            log::info!("Database opened, running schema migrations and direct insertions...");
            let shared_db = Arc::new(db_manager);
            *db_lock = Some(Arc::clone(&shared_db));
            
            // Record login attempt in secure audit logger
            let conn = shared_db.conn();
            
            // Satisfy foreign key constraints
            let _ = conn.execute(
                "INSERT INTO devices (device_id, device_name, public_key_pem, certificate_pem, fingerprint, is_approved, paired_at, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
                 ON CONFLICT(device_id) DO NOTHING",
                rusqlite::params!["desktop_direct_session", "Desktop Host", "", "", "", "", ""],
            );
            let _ = conn.execute(
                "INSERT INTO sessions (session_id, device_id, token_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id) DO NOTHING",
                rusqlite::params!["desktop_direct_session", "desktop_direct_session", "", "", ""],
            );
            let _ = conn.execute(
                "INSERT INTO devices (device_id, device_name, public_key_pem, certificate_pem, fingerprint, is_approved, paired_at, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
                 ON CONFLICT(device_id) DO NOTHING",
                rusqlite::params!["mobile_chat_session", "Mobile Client", "", "", "", "", ""],
            );
            let _ = conn.execute(
                "INSERT INTO sessions (session_id, device_id, token_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id) DO NOTHING",
                rusqlite::params!["mobile_chat_session", "mobile_chat_session", "", "", ""],
            );

            log::info!("Logging AUTH_SUCCESS event in database...");
            let logger = avatar_core::audit::AuditLogger::new(&*conn, passcode.as_bytes());
            if let Err(e) = logger.log_event("AUTH_SUCCESS", None, "User unlocked system database.", "INFO") {
                log::error!("Database logger failed: {}", e);
                return Err(format!("Database loaded but logging failed: {}", e));
            }

            log::info!("Database unlocked successfully");
            Ok("unlocked".to_string())
        }
        Err(e) => {
            log::error!("Failed to open encrypted database: {}", e);
            Err("Invalid PIN/Passcode or database corrupted.".to_string())
        }
    }
}

#[tauri::command]
fn get_telemetry(state: State<'_, AppState>) -> Result<SystemMetrics, String> {
    let mut monitor = state.monitor.lock().unwrap();
    Ok(monitor.get_metrics())
}

#[tauri::command]
async fn start_streaming(app: tauri::AppHandle, state: State<'_, AppState>, port: u16) -> Result<String, String> {
    // Signaling server and tunnel are now managed on application startup.
    // Retaining handler for backwards-compatibility.
    Ok(format!("Stream server listening on port {}", port))
}

#[tauri::command]
fn get_pairing_payload(app: AppHandle, state: State<'_, AppState>) -> Result<PairingPayload, String> {
    // Acquire database manager handle
    let db = {
        let db_lock = state.db.lock().unwrap();
        db_lock.as_ref().map(|d| Arc::clone(d))
    }.ok_or("Database locked")?;

    // Create self-signed root cert for streaming identity if missing
    let cert = generate_self_signed_cert("AvatarServer")
        .map_err(|e| format!("Certificate generation failed: {}", e))?;

    // Retrieve local ip addresses to pass to mobile client
    let mut local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            socket.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let mut port = 8086;

    // Override with tunnel URL if tunnel is active
    {
        let tunnel_lock = state.tunnel_url.lock().unwrap();
        if !tunnel_lock.is_empty() {
            local_ip = tunnel_lock.clone();
            port = 443;
        }
    }

    let pairing_token = generate_pairing_token();

    // Register pairing session in database
    let now = chrono::Utc::now();
    let expiry = now + chrono::Duration::minutes(5);
    
    // Hash token
    let hash = ring::digest::digest(&ring::digest::SHA256, pairing_token.as_bytes());
    let token_hash = hex::encode(hash.as_ref());
    
    // Register temporary session in SQL (or pair device record)
    db.conn().execute(
        "INSERT INTO devices (device_id, device_name, public_key_pem, certificate_pem, fingerprint, is_approved, paired_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)
         ON CONFLICT(device_id) DO NOTHING",
        rusqlite::params![token_hash, "Pending Mobile Client", cert.cert_pem, cert.cert_pem, cert.fingerprint, now.to_rfc3339(), now.to_rfc3339()],
    ).map_err(|e| e.to_string())?;

    // Derive discovery key
    let discovery_input = format!("discovery_{}", pairing_token);
    let discovery_hash = ring::digest::digest(&ring::digest::SHA256, discovery_input.as_bytes());
    let discovery_key = hex::encode(discovery_hash.as_ref());

    // Save to discovery_keys.json in local data directory
    let data_dir = app.path().app_local_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let discovery_keys_path = data_dir.join("discovery_keys.json");
    let mut keys: Vec<String> = if discovery_keys_path.exists() {
        std::fs::read_to_string(&discovery_keys_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(Vec::new)
    } else {
        Vec::new()
    };
    if !keys.contains(&discovery_key) {
        keys.push(discovery_key);
        if let Ok(json_content) = serde_json::to_string(&keys) {
            let _ = std::fs::write(&discovery_keys_path, json_content);
        }
    }

    // Publish current connection target to KV store
    let target = if port == 443 {
        format!("{}:443", local_ip)
    } else {
        format!("{}:8086", local_ip)
    };
    let _ = state.publish_tx.send(target);

    Ok(PairingPayload {
        server_ip: local_ip,
        port,
        fingerprint: cert.fingerprint,
        token: pairing_token,
    })
}

#[tauri::command]
fn get_active_connections(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    if let Some(streaming) = &*state.streaming.lock().unwrap() {
        let active = streaming.active_connections.lock().unwrap();
        Ok(active.clone())
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
async fn send_command(state: State<'_, AppState>, text: &str, session_id: &str) -> Result<String, String> {
    let db = {
        let db_lock = state.db.lock().unwrap();
        db_lock.as_ref().map(|d| Arc::clone(d))
    }.ok_or("Database locked")?;

    let memory = MemoryManager::new(&db);
    
    // 1. Save input message in chat history
    memory.save_message(session_id, "user", text).map_err(|e| e.to_string())?;
    
    // 2. Fetch context history (last 10 messages)
    let history = memory.get_conversation_history(session_id, 10).map_err(|e| e.to_string())?;
    
    // 3. Process with Local AI Coordinator
    match state.ai.query_llm(None, &history).await {
        Ok(reply) => {
            // 4. Try parsing reply as intent JSON
            if let Some(intent) = extract_json_intent(&reply) {
                let mut dispatcher = state.dispatcher.lock().unwrap();
                let exec_res = dispatcher.dispatch_intent(intent);
                
                let result_str = match exec_res {
                    ExecutionResult::Success(msg) => format!("Executed: {}", msg),
                    ExecutionResult::PermissionRequired { action, details, token } => {
                        format!("WARNING: Safe execution block. Approval required for action [{}] ({}) Token: {}", action, details, token)
                    }
                    ExecutionResult::Failed(e) => format!("Execution failed: {}", e),
                };
                
                memory.save_message(session_id, "assistant", &result_str).map_err(|e| e.to_string())?;
                return Ok(result_str);
            }
            
            // Default plain text response
            memory.save_message(session_id, "assistant", &reply).map_err(|e| e.to_string())?;
            
            // Speak text using native Windows TTS
            let voice = Arc::clone(&state.voice);
            let speech_text = reply.clone();
            tokio::spawn(async move {
                let _ = voice.speak(&speech_text);
            });
            
            Ok(reply)
        }
        Err(e) => Err(format!("AI query failure: {}", e)),
    }
}

#[tauri::command]
fn trigger_emergency_kill(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let db = {
        let db_lock = state.db.lock().unwrap();
        db_lock.as_ref().map(|d| Arc::clone(d))
    }.ok_or("Database locked")?;

    // Drop all device pairs, revoke certificates, disconnect sessions
    db.conn().execute("DELETE FROM sessions", []).map_err(|e| e.to_string())?;
    db.conn().execute("DELETE FROM devices", []).map_err(|e| e.to_string())?;

    // Delete discovery keys file
    let data_dir = app.path().app_local_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let discovery_keys_path = data_dir.join("discovery_keys.json");
    let data_dir_clone = data_dir.clone();
    tauri::async_runtime::spawn(async move {
        avatar_core::security::clear_discovery_keys(&data_dir_clone).await;
    });
    if discovery_keys_path.exists() {
        let _ = std::fs::remove_file(discovery_keys_path);
    }

    // Reset passcode hash
    {
        let mut pass_hash = state.passcode_hash.lock().unwrap();
        *pass_hash = String::new();
    }
    if let Some(streaming) = &*state.streaming.lock().unwrap() {
        let mut stream_pass_hash = streaming.passcode_hash.lock().unwrap();
        *stream_pass_hash = String::new();
    }

    // Close active streaming connection
    let mut streaming_lock = state.streaming.lock().unwrap();
    *streaming_lock = None;

    Ok("EMERGENCY KILL COMPLETED. ALL DEVICES DEAUTHORIZED AND DISCONNECTED.".to_string())
}

// --- Main Runner ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    avatar_core::init_logger();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir().unwrap_or_else(|_| PathBuf::from("."));
            let _ = std::fs::create_dir_all(&data_dir);
            let db_path = data_dir.join("avatar_secure.db");
            let salt = "avatar_secure_salt_value_123".to_string();

            let passcode_hash = Arc::new(Mutex::new(String::new()));
            let passcode_hash_clone = passcode_hash.clone();

            let db = Arc::new(Mutex::new(None));
            let ai = Arc::new(AiManager::new(None, Some("Avatar".to_string())));
            let voice = Arc::new(VoiceManager::new(None));

            let streaming = Arc::new(StreamingServer::new(
                db.clone(),
                db_path,
                salt.clone(),
                ai.clone(),
                voice.clone(),
                passcode_hash_clone,
            ));

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let data_dir_clone = data_dir.clone();
            tauri::async_runtime::spawn(async move {
                let mut last_published = String::new();
                while let Some(target) = rx.recv().await {
                    let mut latest_target = target;
                    while let Ok(newer) = rx.try_recv() {
                        latest_target = newer;
                    }
                    if latest_target == last_published {
                        continue;
                    }
                    avatar_core::security::publish_discovery_keys(&data_dir_clone, &latest_target).await;
                    last_published = latest_target.clone();
                }
            });

            let app_state = AppState {
                db,
                streaming: Mutex::new(Some(streaming)),
                monitor: Mutex::new(SystemMonitor::new()),
                ai,
                voice,
                dispatcher: Arc::new(Mutex::new(IntentDispatcher::new())),
                salt,
                tunnel_url: Mutex::new(String::new()),
                passcode_hash,
                publish_tx: tx.clone(),
            };
            app.manage(app_state);

            // Start signaling server immediately
            let streaming_clone = app.state::<AppState>().streaming.lock().unwrap().as_ref().unwrap().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = streaming_clone.start_signaling_server(8086).await {
                    log::error!("Signaling server crash on startup: {}", e);
                }
            });

            // Publish local IP target on startup
            let local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
                .and_then(|socket| {
                    socket.connect("8.8.8.8:80")?;
                    socket.local_addr()
                })
                .map(|addr| addr.ip().to_string())
                .unwrap_or_else(|_| "127.0.0.1".to_string());
            let startup_target = format!("{}:8086", local_ip);
            let _ = tx.send(startup_target);

            // Start SSH tunnel background thread immediately
            let app_handle = app.handle().clone();
            let local_ip_clone = local_ip.clone();
            std::thread::spawn(move || {
                log::info!("Starting localhost.run SSH tunnel on port 8086");
                #[cfg(target_os = "windows")]
                let _ = std::process::Command::new("taskkill")
                    .args(&["/F", "/IM", "ssh.exe"])
                    .output();

                loop {
                    let mut cmd = std::process::Command::new("ssh");
                    cmd.args(&[
                        "-o", "StrictHostKeyChecking=no",
                        "-o", "UserKnownHostsFile=/dev/null",
                        "-R", "80:127.0.0.1:8086",
                        "nokey@localhost.run"
                    ]);
                    cmd.stdout(std::process::Stdio::piped());
                    cmd.stderr(std::process::Stdio::null());

                    match cmd.spawn() {
                        Ok(mut child) => {
                            log::info!("SSH tunnel spawned successfully");
                            let stdout = child.stdout.take().expect("Failed to take SSH stdout");
                            use std::io::{BufRead, BufReader};
                            let reader = BufReader::new(stdout);
                            for line in reader.lines() {
                                if let Ok(line_str) = line {
                                    log::info!("SSH output: {}", line_str);
                                    if line_str.contains(".lhr.life") {
                                        let parts: Vec<&str> = line_str.split_whitespace().collect();
                                        for part in parts {
                                            if part.contains(".lhr.life") {
                                                let mut domain = part.trim();
                                                if let Some(idx) = domain.find("://") {
                                                    domain = &domain[idx + 3..];
                                                }
                                                if domain.ends_with('/') {
                                                    domain = &domain[..domain.len() - 1];
                                                }
                                                log::info!("Detected SSH tunnel domain: {}", domain);
                                                let state = app_handle.state::<AppState>();
                                                let mut tunnel_url_lock = state.tunnel_url.lock().unwrap();
                                                *tunnel_url_lock = domain.to_string();
                                                
                                                // Trigger publish of connection details to all discovery keys!
                                                let target = format!("{}:443", domain);
                                                let _ = state.publish_tx.send(target);
                                                
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            let _ = child.wait();
                        }
                        Err(e) => {
                            log::error!("Failed to spawn SSH tunnel: {}", e);
                        }
                    }
                    log::info!("SSH tunnel disconnected. Retrying in 5 seconds...");
                    
                    // Publish local IP fallback while tunnel is down!
                    let local_ip_fallback = format!("{}:8086", local_ip_clone);
                    let state = app_handle.state::<AppState>();
                    let _ = state.publish_tx.send(local_ip_fallback);
                    
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_locked,
            unlock_database,
            get_telemetry,
            get_active_connections,
            start_streaming,
            get_pairing_payload,
            send_command,
            trigger_emergency_kill
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }
}
// trigger rebuild

