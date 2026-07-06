use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::error::Error;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use futures_util::{StreamExt, SinkExt};
use log::{info, error, warn};

use webrtc::api::APIBuilder;
use webrtc::api::media_engine::MediaEngine;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use crate::capture::ScreenCapture;
use crate::intent::{IntentDispatcher, SystemIntent, ExecutionResult, extract_json_intent};
use crate::db::DbManager;
use crate::ai::AiManager;
use crate::voice::VoiceManager;
use crate::memory::MemoryManager;
use serde::Deserialize;
use crate::security::{encrypt_aes_gcm, decrypt_aes_gcm, encrypt_aes_gcm_bin, decrypt_aes_gcm_bin};
pub struct StreamingServer {
    db: Arc<std::sync::Mutex<Option<Arc<DbManager>>>>,
    db_path: std::path::PathBuf,
    salt: String,
    dispatcher: Arc<Mutex<IntentDispatcher>>,
    capture: Arc<ScreenCapture>,
    pub camera: Arc<crate::camera::CameraCapture>,
    ai: Arc<AiManager>,
    voice: Arc<VoiceManager>,
    pub passcode_hash: Arc<std::sync::Mutex<String>>,
    pub active_connections: Arc<std::sync::Mutex<Vec<String>>>,
}

impl StreamingServer {
    pub fn new(
        db: Arc<std::sync::Mutex<Option<Arc<DbManager>>>>,
        db_path: std::path::PathBuf,
        salt: String,
        ai: Arc<AiManager>,
        voice: Arc<VoiceManager>,
        passcode_hash: Arc<std::sync::Mutex<String>>,
    ) -> Self {
        StreamingServer {
            db,
            db_path,
            salt,
            dispatcher: Arc::new(Mutex::new(IntentDispatcher::new())),
            capture: Arc::new(ScreenCapture::new()),
            camera: Arc::new(crate::camera::CameraCapture::new()),
            ai,
            voice,
            passcode_hash,
            active_connections: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Starts the background WebSocket signaling listener on the specified port.
    /// Authenticates sessions using query tokens before proceeding with WebRTC negotiation.
    pub async fn start_signaling_server(self: Arc<Self>, port: u16) -> Result<(), Box<dyn Error + Send + Sync>> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr).await?;
        info!("Signaling Server listening on ws://{}", addr);

        let this = Arc::clone(&self);
        tokio::spawn(async move {
            while let Ok((stream, peer_addr)) = listener.accept().await {
                info!("Incoming signaling connection request from peer: {}", peer_addr);
                let server = Arc::clone(&this);

                tokio::spawn(async move {
                    match accept_async(stream).await {
                        Ok(mut ws_stream) => {
                            info!("WebSocket handshake completed with: {}", peer_addr);
                            
                            // 1. Session Token Authentication
                            if let Some(Ok(msg)) = ws_stream.next().await {
                                if let Ok(token) = msg.into_text() {
                                    if !server.validate_token(&token) {
                                        warn!("Peer {} authentication failed. Invalid or expired token.", peer_addr);
                                        let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text("AUTH_FAILED".into())).await;
                                        let _ = ws_stream.close(None).await;
                                        return;
                                    }
                                    
                                    // Derive temporary_key = sha256(token)
                                    let temp_hash = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
                                    let mut temporary_key = [0u8; 32];
                                    temporary_key.copy_from_slice(temp_hash.as_ref());
                                    
                                    let is_locked = {
                                        server.db.lock().unwrap().is_none()
                                    };

                                    let mut session_key = [0u8; 32];
                                    let mut is_authenticated = false;

                                    if is_locked {
                                        info!("Peer {} connected, but host database is locked.", peer_addr);
                                        let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text("HOST_LOCKED".into())).await;
                                        
                                        // Wait for encrypted unlock_host command
                                        if let Some(Ok(enc_msg)) = ws_stream.next().await {
                                            if let Ok(enc_text) = enc_msg.into_text() {
                                                // Decrypt with temporary_key
                                                match decrypt_aes_gcm(&temporary_key, &enc_text).map_err(|e| e.to_string()) {
                                                    Ok(decrypted_text) => {
                                                        // Process unlock
                                                        match server.handle_chat_command(&decrypted_text).await {
                                                            Ok(response_text) => {
                                                                if response_text.contains("Remote unlock successful") {
                                                                    // Extract passcode to derive session_key
                                                                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&decrypted_text) {
                                                                        let passcode = payload.get("params")
                                                                            .and_then(|p| p.get("passcode"))
                                                                            .and_then(|v| v.as_str())
                                                                            .unwrap_or("");
                                                                        
                                                                        let hash = ring::digest::digest(&ring::digest::SHA256, passcode.as_bytes());
                                                                        session_key.copy_from_slice(hash.as_ref());
                                                                        is_authenticated = true;

                                                                        // Send success encrypted with session_key
                                                                        if let Some(enc_resp) = encrypt_aes_gcm(&session_key, &response_text).ok() {
                                                                            let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(enc_resp)).await;
                                                                        }
                                                                        
                                                                        let (w, h) = server.capture.dimensions();
                                                                        let auth_ok_msg = format!("AUTH_OK {} {}", w, h);
                                                                        if let Some(enc_auth_ok) = encrypt_aes_gcm(&session_key, &auth_ok_msg).ok() {
                                                                            let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(enc_auth_ok)).await;
                                                                        }
                                                                    }
                                                                } else {
                                                                    // Send failure encrypted with temporary_key and close
                                                                    if let Some(enc_resp) = encrypt_aes_gcm(&temporary_key, &response_text).ok() {
                                                                        let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(enc_resp)).await;
                                                                    }
                                                                }
                                                            }
                                                            Err(_) => {}
                                                        }
                                                    }
                                                    Err(err_str) => {
                                                        warn!("Failed to decrypt unlock_host message from {}: {}", peer_addr, err_str);
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        // Database is unlocked. Ask for session authentication
                                        let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text("NEED_AUTH".into())).await;
                                        
                                        // Wait for encrypted authenticate command
                                        if let Some(Ok(enc_msg)) = ws_stream.next().await {
                                            if let Ok(enc_text) = enc_msg.into_text() {
                                                // Derive expected session_key from server.passcode_hash
                                                let mut derived_ok = false;
                                                {
                                                    let pass_hash = server.passcode_hash.lock().unwrap();
                                                    if !pass_hash.is_empty() {
                                                        if let Ok(decoded_hash) = hex::decode(&*pass_hash) {
                                                            let len = std::cmp::min(decoded_hash.len(), 32);
                                                            session_key[..len].copy_from_slice(&decoded_hash[..len]);
                                                            derived_ok = true;
                                                        }
                                                    }
                                                }
                                                
                                                if derived_ok {
                                                    // Decrypt with session_key
                                                    match decrypt_aes_gcm(&session_key, &enc_text).map_err(|e| e.to_string()) {
                                                        Ok(decrypted_text) => {
                                                            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&decrypted_text) {
                                                                let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");
                                                                if action == "authenticate" {
                                                                    is_authenticated = true;
                                                                    let (w, h) = server.capture.dimensions();
                                                                    let auth_ok_msg = format!("AUTH_OK {} {}", w, h);
                                                                    if let Some(enc_auth_ok) = encrypt_aes_gcm(&session_key, &auth_ok_msg).ok() {
                                                                        let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(enc_auth_ok)).await;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        Err(err_str) => {
                                                            warn!("E2EE Decryption/Password verification failed for {}: {}", peer_addr, err_str);
                                                            let _ = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text("AUTH_FAILED".into())).await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if !is_authenticated {
                                        let _ = ws_stream.close(None).await;
                                        return;
                                    }

                                    // Add to active connections list
                                    {
                                        let mut active = server.active_connections.lock().unwrap();
                                        active.push(format!("Mobile Client ({})", peer_addr));
                                    }
                                    
                                    // Split stream for concurrent reading/writing
                                    let (mut ws_write, mut ws_read) = ws_stream.split();
                                    let (tx, mut rx) = tokio::sync::mpsc::channel::<tokio_tungstenite::tungstenite::Message>(16);
                                    let screen_stream_active = Arc::new(AtomicBool::new(false));
                                    let camera_stream_active = Arc::new(AtomicBool::new(false));
                                    
                                    // Spawn write worker task
                                    let session_key_write = session_key.clone();
                                    tokio::spawn(async move {
                                        while let Some(msg) = rx.recv().await {
                                            let enc_msg = match msg {
                                                tokio_tungstenite::tungstenite::Message::Text(text) => {
                                                    if let Ok(encrypted_text) = encrypt_aes_gcm(&session_key_write, &text) {
                                                        tokio_tungstenite::tungstenite::Message::Text(encrypted_text)
                                                    } else {
                                                        continue;
                                                    }
                                                }
                                                tokio_tungstenite::tungstenite::Message::Binary(bin) => {
                                                    if let Ok(encrypted_bin) = encrypt_aes_gcm_bin(&session_key_write, &bin) {
                                                        tokio_tungstenite::tungstenite::Message::Binary(encrypted_bin)
                                                    } else {
                                                        continue;
                                                    }
                                                }
                                                other => other,
                                            };
                                            if let Err(_) = ws_write.send(enc_msg).await {
                                                break;
                                            }
                                        }
                                        let _ = ws_write.close().await;
                                    });
                                    
                                    let tx_clone = tx.clone();
                                    let server_clone = Arc::clone(&server);
                                    let session_key_read = session_key.clone();
                                    
                                    // 2. Process incoming messages (SDP offer or Chat/Stream commands)
                                    while let Some(Ok(msg)) = ws_read.next().await {
                                        if msg.is_text() {
                                            let enc_text = msg.into_text().unwrap();
                                            // Decrypt using session_key
                                            if let Ok(msg_text) = decrypt_aes_gcm(&session_key_read, &enc_text) {
                                                if msg_text.trim().starts_with('{') {
                                                    // Handle command (screen capture trigger or other JSON commands)
                                                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&msg_text) {
                                                        let action = json_val.get("action").and_then(|v| v.as_str()).unwrap_or("");
                                                        if action == "start_screen" {
                                                            camera_stream_active.store(false, Ordering::SeqCst);
                                                            server_clone.camera.stop();

                                                            if screen_stream_active
                                                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                                                .is_err()
                                                            {
                                                                info!("Screen stream already active for this connection");
                                                                continue;
                                                            }

                                                            let capture = Arc::clone(&server_clone.capture);
                                                            let tx_capture = tx_clone.clone();
                                                            let stream_flag = Arc::clone(&screen_stream_active);
                                                            let (width, height) = capture.dimensions();

                                                            let screen_info = serde_json::json!({
                                                                "type": "screen_info",
                                                                "width": width,
                                                                "height": height,
                                                            });
                                                            let _ = tx_capture.try_send(
                                                                tokio_tungstenite::tungstenite::Message::Text(screen_info.to_string())
                                                            );

                                                            tokio::spawn(async move {
                                                                info!("Starting low-latency JPEG screen capture stream (20fps, 0.5x scale)");
                                                                loop {
                                                                    if tx_capture.is_closed() || !stream_flag.load(Ordering::SeqCst) { break; }

                                                                    let capture_clone2 = Arc::clone(&capture);
                                                                    let result = tokio::task::spawn_blocking(move || {
                                                                        capture_clone2.capture_frame_jpeg_scaled(0.5, 55)
                                                                    }).await;

                                                                    match result {
                                                                        Ok(Ok(jpeg_bytes)) if !jpeg_bytes.is_empty() => {
                                                                            let bin_msg = tokio_tungstenite::tungstenite::Message::Binary(jpeg_bytes);
                                                                            match tx_capture.try_send(bin_msg) {
                                                                                Ok(_) => {}
                                                                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                                                                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                                                            }
                                                                        }
                                                                        Ok(Ok(_)) => {
                                                                            warn!("Screen capture returned empty JPEG frame");
                                                                            tokio::time::sleep(Duration::from_millis(100)).await;
                                                                        }
                                                                        Ok(Err(e)) => {
                                                                            error!("Screen capture failed: {}", e);
                                                                            tokio::time::sleep(Duration::from_millis(200)).await;
                                                                        }
                                                                        Err(e) => {
                                                                            error!("Blocking capture task panicked: {}", e);
                                                                            tokio::time::sleep(Duration::from_millis(200)).await;
                                                                        }
                                                                    }

                                                                    tokio::time::sleep(Duration::from_millis(50)).await;
                                                                }
                                                                stream_flag.store(false, Ordering::SeqCst);
                                                                info!("JPEG stream loop exited");
                                                            });
                                                        } else if action == "start_camera" {
                                                            screen_stream_active.store(false, Ordering::SeqCst);
                                                            if camera_stream_active
                                                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                                                .is_ok()
                                                            {
                                                                let camera = Arc::clone(&server_clone.camera);
                                                                let tx_camera = tx_clone.clone();
                                                                let stream_flag = Arc::clone(&camera_stream_active);
                                                                
                                                                if let Err(e) = camera.start() {
                                                                    error!("Failed to start camera capture: {}", e);
                                                                    stream_flag.store(false, Ordering::SeqCst);
                                                                    continue;
                                                                }

                                                                let camera_info = serde_json::json!({
                                                                    "type": "camera_info",
                                                                    "width": 640,
                                                                    "height": 480,
                                                                });
                                                                let _ = tx_camera.try_send(
                                                                    tokio_tungstenite::tungstenite::Message::Text(camera_info.to_string())
                                                                );

                                                                tokio::spawn(async move {
                                                                    info!("Starting low-latency JPEG camera capture stream");
                                                                    loop {
                                                                        if tx_camera.is_closed() || !stream_flag.load(Ordering::SeqCst) {
                                                                            break;
                                                                        }

                                                                        let camera_clone2 = Arc::clone(&camera);
                                                                        let result = tokio::task::spawn_blocking(move || {
                                                                            camera_clone2.read_frame()
                                                                        }).await;

                                                                        match result {
                                                                            Ok(Ok(jpeg_bytes)) if !jpeg_bytes.is_empty() => {
                                                                                let bin_msg = tokio_tungstenite::tungstenite::Message::Binary(jpeg_bytes);
                                                                                match tx_camera.try_send(bin_msg) {
                                                                                    Ok(_) => {}
                                                                                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                                                                                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                                                                }
                                                                            }
                                                                            Ok(Ok(_)) => {
                                                                                tokio::time::sleep(Duration::from_millis(50)).await;
                                                                            }
                                                                            Ok(Err(e)) => {
                                                                                error!("Camera capture read failed: {}", e);
                                                                                tokio::time::sleep(Duration::from_millis(200)).await;
                                                                            }
                                                                            Err(e) => {
                                                                                error!("Blocking camera read task panicked: {}", e);
                                                                                tokio::time::sleep(Duration::from_millis(200)).await;
                                                                            }
                                                                        }
                                                                    }
                                                                    camera.stop();
                                                                    stream_flag.store(false, Ordering::SeqCst);
                                                                    info!("JPEG camera stream loop exited");
                                                                });
                                                            }
                                                        } else {
                                                            // Process general chat/telemetry command
                                                            let tx_resp = tx_clone.clone();
                                                            let server_cmd = Arc::clone(&server_clone);
                                                            let cmd_text = msg_text.clone();
                                                            tokio::spawn(async move {
                                                                match server_cmd.handle_chat_command(&cmd_text).await {
                                                                    Ok(response_text) => {
                                                                        let _ = tx_resp.send(tokio_tungstenite::tungstenite::Message::Text(response_text)).await;
                                                                    }
                                                                    Err(e) => {
                                                                        error!("Chat command processing failed: {}", e);
                                                                        let err_payload = serde_json::to_string(&ExecutionResult::Failed(e.to_string())).unwrap_or_default();
                                                                        let _ = tx_resp.send(tokio_tungstenite::tungstenite::Message::Text(err_payload)).await;
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    }
                                                } else {
                                                    // Handle WebRTC SDP Offer
                                                    let tx_resp = tx_clone.clone();
                                                    let server_cmd = Arc::clone(&server_clone);
                                                    let offer_text = msg_text.clone();
                                                    tokio::spawn(async move {
                                                        match server_cmd.handle_remote_offer(&offer_text).await {
                                                            Ok(sdp_answer) => {
                                                                info!("Sending WebRTC SDP Answer back to client");
                                                                let _ = tx_resp.send(tokio_tungstenite::tungstenite::Message::Text(sdp_answer)).await;
                                                            }
                                                            Err(e) => {
                                                                error!("WebRTC setup failure: {}", e);
                                                                let _ = tx_resp.send(tokio_tungstenite::tungstenite::Message::Text(format!("ERROR: {}", e))).await;
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                        }
                                    }
                                    // Cleanup streams on socket exit
                                    screen_stream_active.store(false, Ordering::SeqCst);
                                    camera_stream_active.store(false, Ordering::SeqCst);
                                    server.camera.stop();

                                    // Remove from active connections list
                                    {
                                        let mut active = server.active_connections.lock().unwrap();
                                        active.retain(|x| x != &format!("Mobile Client ({})", peer_addr));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("WebSocket upgrade failure for {}: {}", peer_addr, e);
                        }
                    }
                    info!("Signaling stream closed for peer: {}", peer_addr);
                });
            }
        });

        Ok(())
    }

    /// Input commands that work over a paired connection without vault unlock.
    fn is_remote_input_action(action: &str) -> bool {
        matches!(
            action,
            "move_mouse" | "click" | "scroll" | "type_text" | "press_key" | "press_shortcut" | "unlock_os"
        )
    }

    /// Checks if a session token is active and valid.
    fn validate_token(&self, token: &str) -> bool {
        let db_lock = self.db.lock().unwrap();
        let db = match &*db_lock {
            Some(d) => Arc::clone(d),
            None => {
                // Allow connection when locked to support remote unlocking,
                // but only if the token matches a discovery key in discovery_keys.json
                if let Some(data_dir) = self.db_path.parent() {
                    let keys_file = data_dir.join("discovery_keys.json");
                    if keys_file.exists() {
                        if let Ok(content) = std::fs::read_to_string(&keys_file) {
                            if let Ok(keys) = serde_json::from_str::<Vec<String>>(&content) {
                                let discovery_input = format!("discovery_{}", token);
                                let discovery_hash = ring::digest::digest(&ring::digest::SHA256, discovery_input.as_bytes());
                                let discovery_key = hex::encode(discovery_hash.as_ref());
                                return keys.contains(&discovery_key);
                            }
                        }
                    }
                }
                return false;
            }
        };

        // Hash the input token to match standard token hashing
        let hash = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
        let token_hash = hex::encode(hash.as_ref());
        
        let now = chrono::Utc::now();
        let now_str = now.to_rfc3339();
        
        let conn = db.conn();
        
        // 1. Check if it's already an active session
        if let Ok(mut stmt) = conn.prepare(
            "SELECT count(*) FROM sessions WHERE token_hash = ?1 AND expires_at > ?2"
        ) {
            if let Ok(count) = stmt.query_row(rusqlite::params![token_hash, now_str], |row| row.get::<_, i64>(0)) {
                if count > 0 {
                    return true;
                }
            }
        }

        // 2. If not in active sessions, check if it matches a device (pairing phase)
        let device_exists = if let Ok(mut stmt) = conn.prepare(
            "SELECT count(*) FROM devices WHERE device_id = ?1"
        ) {
            stmt.query_row(rusqlite::params![token_hash], |row| row.get::<_, i64>(0)).unwrap_or(0) > 0
        } else {
            false
        };

        if device_exists {
            // Approve the device if it's not approved yet
            let _ = conn.execute(
                "UPDATE devices SET is_approved = 1 WHERE device_id = ?1",
                rusqlite::params![token_hash],
            );

            // Insert a new session entry
            let session_id = format!("sess_{}", uuid::Uuid::new_v4());
            let expires_at = (now + chrono::Duration::days(30)).to_rfc3339();
            let _ = conn.execute(
                "INSERT INTO sessions (session_id, device_id, token_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id) DO NOTHING",
                rusqlite::params![session_id, token_hash, token_hash, now_str, expires_at],
            );

            return true;
        }

        false
    }

    /// Handles a complete WebRTC connection setup, returning the local SDP answer.
    async fn handle_remote_offer(&self, offer_sdp: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
        info!("Received WebRTC SDP Offer");
        
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        
        let api = APIBuilder::new().with_media_engine(m).build();
        let config = RTCConfiguration::default();
        
        let peer_connection = Arc::new(api.new_peer_connection(config).await?);
        
        // Create WebRTC Video Track for Screen Stream
        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "video/vp8".to_string(),
                ..Default::default()
            },
            "video".to_string(),
            "avatar-screen".to_string(),
        ));
        
        peer_connection.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>).await?;
        
        // Start Video Streaming Loop in Background
        let capture_clone = Arc::clone(&self.capture);
        let video_track_clone = Arc::clone(&video_track);
        let pc_clone = Arc::clone(&peer_connection);
        
        tokio::spawn(async move {
            info!("Starting video frame capture stream loop");
            let frame_duration = Duration::from_millis(40); // ~24fps
            
            while pc_clone.connection_state() != RTCPeerConnectionState::Closed {
                if pc_clone.connection_state() == RTCPeerConnectionState::Connected {
                    let raw_pixels_opt = match capture_clone.capture_frame() {
                        Ok(raw_pixels) => Some(raw_pixels),
                        Err(e) => {
                            error!("Screen capture frame acquisition failed: {}", e);
                            None
                        }
                    };

                    if let Some(raw_pixels) = raw_pixels_opt {
                        let compressed_frame = raw_pixels; // VP8 Frame placeholder bytes
                        let sample = Sample {
                            data: compressed_frame.into(),
                            duration: frame_duration,
                            ..Default::default()
                        };
                        
                        if let Err(e) = video_track_clone.write_sample(&sample).await {
                            error!("Failed to write screen frame sample: {}", e);
                            break;
                        }
                    }
                }
                sleep(frame_duration).await;
            }
            info!("Video stream loop exited");
        });
        
        // Set Remote Offer Session Description
        let desc = RTCSessionDescription::offer(offer_sdp.to_string())?;
        peer_connection.set_remote_description(desc).await?;
        
        // Listen to incoming control commands on WebRTC Data Channels
        let dispatcher_clone = Arc::clone(&self.dispatcher);
        peer_connection.on_data_channel(Box::new(move |dc| {
            let dispatcher = Arc::clone(&dispatcher_clone);
            info!("WebRTC Data Channel established: {}", dc.label());
            
            Box::pin(async move {
                let dc_clone = Arc::clone(&dc);
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    let dispatcher_inner = Arc::clone(&dispatcher);
                    let dc_inner = Arc::clone(&dc_clone);
                    
                    Box::pin(async move {
                        let payload = String::from_utf8_lossy(&msg.data);
                        info!("Received data channel control command: {}", payload);
                        
                        match serde_json::from_str::<SystemIntent>(&payload) {
                            Ok(intent) => {
                                let mut dispatcher_locked = dispatcher_inner.lock().await;
                                let res = dispatcher_locked.dispatch_intent(intent);
                                let resp_payload = serde_json::to_string(&res).unwrap_or_default();
                                
                                if let Err(e) = dc_inner.send_text(resp_payload).await {
                                    error!("Failed to write execution response back to remote data channel: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("Invalid JSON control package received: {}", e);
                            }
                        }
                    })
                }));
            })
        }));
        
        // Create local SDP Answer
        let answer = peer_connection.create_answer(None).await?;
        peer_connection.set_local_description(answer.clone()).await?;
        
        Ok(answer.sdp)
    }

    /// Handles a secure JSON chat command received via WebSocket from a mobile client.
    async fn handle_chat_command(&self, json_str: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
        let payload: MobileCommandPayload = serde_json::from_str(json_str)?;
        
        if payload.action == "unlock_host" {
            let passcode = payload.params.get("passcode").and_then(|v| v.as_str()).ok_or("Missing passcode")?;
            {
                let db_lock = self.db.lock().unwrap();
                if db_lock.is_some() {
                    let res = serde_json::to_string(&ExecutionResult::Success("Already unlocked".to_string()))?;
                    return Ok(res);
                }
            }

            let auth_file = self.db_path.parent()
                .map(|p| p.join("avatar_auth.hash"))
                .ok_or("Invalid database path")?;

            if let Err(e) = crate::security::verify_or_init_passcode(&auth_file, passcode) {
                return Err(format!("Invalid PIN/Passcode: {}", e).into());
            }

            match DbManager::open_encrypted(&self.db_path, passcode, &self.salt) {
                Ok(db_manager) => {
                    let shared_db = Arc::new(db_manager);
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

                    let logger = crate::audit::AuditLogger::new(&*conn, passcode.as_bytes());
                    let _ = logger.log_event("AUTH_SUCCESS", None, "User unlocked system remotely.", "INFO");

                    let mut db_lock = self.db.lock().unwrap();
                    *db_lock = Some(Arc::clone(&shared_db));

                    // Store derived passcode hash for E2EE session verification
                    {
                        let hash = ring::digest::digest(&ring::digest::SHA256, passcode.as_bytes());
                        let mut pass_hash = self.passcode_hash.lock().unwrap();
                        *pass_hash = hex::encode(hash.as_ref());
                    }

                    let res = serde_json::to_string(&ExecutionResult::Success("Remote unlock successful".to_string()))?;
                    Ok(res)
                }
                Err(e) => {
                    Err(format!("Invalid PIN/Passcode: {}", e).into())
                }
            }
        } else if Self::is_remote_input_action(&payload.action) {
            // Remote mouse/keyboard control works with pairing auth alone
            let intent = SystemIntent {
                action: payload.action,
                params: payload.params,
            };
            let mut dispatcher = self.dispatcher.lock().await;
            let exec_res = dispatcher.dispatch_intent(intent);
            let response_payload = serde_json::to_string(&exec_res)?;
            Ok(response_payload)
        } else {
            // AI, metrics, and vault commands require database to be unlocked
            let db = {
                let db_lock = self.db.lock().unwrap();
                db_lock.as_ref().map(|d| Arc::clone(d))
            }.ok_or("Database is locked")?;

            if payload.action == "send_command" {
                let text = payload.params.get("text").and_then(|v| v.as_str()).ok_or("Missing text param")?;
                let session_id = payload.params.get("session_id").and_then(|v| v.as_str()).ok_or("Missing session_id param")?;
                
                let memory = MemoryManager::new(&db);
                
                // 1. Save input message in chat history
                memory.save_message(session_id, "user", text).map_err(|e| format!("Save user msg failed: {}", e))?;
                
                // 2. Fetch context history (last 10 messages)
                let mut history = memory.get_conversation_history(session_id, 10).map_err(|e| format!("Get history failed: {}", e))?;
                
                // 3. Query system metrics to inject current status
                let metrics = {
                    let mut disp = self.dispatcher.lock().await;
                    disp.get_system_metrics()
                };

                let gpu_text = if metrics.gpus.is_empty() {
                    "None".to_string()
                } else {
                    metrics.gpus.iter()
                        .map(|g| format!("{} ({:.1} GB Dedicated VRAM)", g.name, g.vram_dedicated_bytes as f64 / 1_073_741_824.0))
                        .collect::<Vec<_>>()
                        .join(", ")
                };

                let browser_tabs_text = if metrics.open_browser_tabs.is_empty() {
                    "None".to_string()
                } else {
                    metrics.open_browser_tabs.iter()
                        .enumerate()
                        .map(|(i, t)| format!("   {}. {}", i + 1, t))
                        .collect::<Vec<_>>()
                        .join("\n")
                };

                let processes_text = if metrics.running_processes.is_empty() {
                    "None".to_string()
                } else {
                    metrics.running_processes.iter()
                        .take(10)
                        .map(|p| format!("   - {} (Memory: {:.1} MB, CPU: {:.1}%)", p.name, p.memory_bytes as f64 / 1_048_576.0, p.cpu_usage_pct))
                        .collect::<Vec<_>>()
                        .join("\n")
                };

                let status_context = format!(
                    "[CURRENT SYSTEM TELEMETRY & WORKSPACE CONTEXT]\n\
                     - CPU Usage: {:.1}%\n\
                     - RAM Usage: {:.1} GB / {:.1} GB ({:.1}% used)\n\
                     - Disk Storage: {:.1} GB / {:.1} GB ({:.1}% used)\n\
                     - Battery level: {:.0}% (Charging: {})\n\
                     - GPU Hardware: {}\n\
                     - Active Browser Tabs/Windows Open (Total: {}):\n\
                     {}\n\
                     - Top Processes by Memory Usage:\n\
                     {}",
                    metrics.cpu_usage_pct,
                    metrics.ram_used_bytes as f64 / 1_073_741_824.0,
                    metrics.ram_total_bytes as f64 / 1_073_741_824.0,
                    if metrics.ram_total_bytes > 0 { (metrics.ram_used_bytes as f64 / metrics.ram_total_bytes as f64) * 100.0 } else { 0.0 },
                    metrics.disk_used_bytes as f64 / 1_073_741_824.0,
                    metrics.disk_total_bytes as f64 / 1_073_741_824.0,
                    if metrics.disk_total_bytes > 0 { (metrics.disk_used_bytes as f64 / metrics.disk_total_bytes as f64) * 100.0 } else { 0.0 },
                    metrics.battery_pct,
                    if metrics.is_charging { "Yes" } else { "No" },
                    gpu_text,
                    metrics.open_browser_tabs.len(),
                    browser_tabs_text,
                    processes_text
                );

                // Insert the system telemetry context as the first message in the LLM query context window.
                history.insert(0, crate::ai::ChatMessage {
                    role: "system".to_string(),
                    content: status_context,
                });

                // 4. Process with Local AI Coordinator
                let query_res = self.ai.query_llm(None, &history).await.map_err(|e| e.to_string());
                match query_res {
                    Ok(reply) => {
                        // 4. Try parsing reply as intent JSON (allowing markdown wrapping/conversational text)
                        if let Some(intent) = extract_json_intent(&reply) {
                            let mut dispatcher = self.dispatcher.lock().await;
                            let exec_res = dispatcher.dispatch_intent(intent);
                            
                            let result_str = match &exec_res {
                                ExecutionResult::Success(msg) => format!("Executed: {}", msg),
                                ExecutionResult::PermissionRequired { action, details, token } => {
                                    format!("WARNING: Safe execution block. Approval required for action [{}] ({}) Token: {}", action, details, token)
                                }
                                ExecutionResult::Failed(e) => format!("Execution failed: {}", e),
                            };
                            
                            memory.save_message(session_id, "assistant", &result_str).map_err(|e| format!("Save assistant msg failed: {}", e))?;
                            
                            let response_payload = serde_json::to_string(&exec_res)?;
                            return Ok(response_payload);
                        }
                        
                        // Default plain text response
                        memory.save_message(session_id, "assistant", &reply).map_err(|e| format!("Save assistant msg failed: {}", e))?;
                        
                        // Speak text using native Windows TTS
                        let voice = Arc::clone(&self.voice);
                        let speech_text = reply.clone();
                        tokio::spawn(async move {
                            let _ = voice.speak(&speech_text);
                        });
                        
                        // Wrap reply in Success ExecutionResult
                        let response_payload = serde_json::to_string(&ExecutionResult::Success(reply))?;
                        Ok(response_payload)
                    }
                    Err(e) => {
                        let err_msg = format!("AI query failure: {}", e);
                        let response_payload = serde_json::to_string(&ExecutionResult::Failed(err_msg))?;
                        Ok(response_payload)
                    }
                }
            } else if payload.action == "get_metrics" {
                let mut dispatcher = self.dispatcher.lock().await;
                let intent = SystemIntent {
                    action: "get_metrics".to_string(),
                    params: serde_json::Value::Null,
                };
                if let ExecutionResult::Success(metrics_json) = dispatcher.dispatch_intent(intent) {
                    Ok(metrics_json)
                } else {
                    Err("Failed to retrieve metrics".into())
                }
            } else if payload.action == "emergency_kill" {
                let _ = db.conn().execute("DELETE FROM sessions", []);
                let _ = db.conn().execute("DELETE FROM devices", []);
                
                // Clear and delete discovery_keys.json, and clear on KV server
                if let Some(data_dir) = self.db_path.parent() {
                    let discovery_keys_path = data_dir.join("discovery_keys.json");
                    if discovery_keys_path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&discovery_keys_path) {
                            if let Ok(keys) = serde_json::from_str::<Vec<String>>(&content) {
                                tokio::spawn(async move {
                                    let client = reqwest::Client::new();
                                    for key in keys {
                                        let url = format!("https://keyvalue.immanuel.co/api/KeyVal/UpdateValue/0gcpgxva/{}/offline/", key);
                                        let _ = client.post(&url).header("content-length", "0").send().await;
                                    }
                                });
                            }
                        }
                        let _ = std::fs::remove_file(discovery_keys_path);
                    }
                }

                // Reset passcode hash
                {
                    let mut pass_hash = self.passcode_hash.lock().unwrap();
                    *pass_hash = String::new();
                }

                let response_payload = serde_json::to_string(&ExecutionResult::Success("Emergency kill switch triggered successfully.".to_string()))?;
                Ok(response_payload)
            } else {
                // Treat as general intent dispatch
                let intent = SystemIntent {
                    action: payload.action,
                    params: payload.params,
                };
                let mut dispatcher = self.dispatcher.lock().await;
                let exec_res = dispatcher.dispatch_intent(intent);
                let response_payload = serde_json::to_string(&exec_res)?;
                Ok(response_payload)
            }
        }
    }
}

#[derive(Deserialize)]
struct MobileCommandPayload {
    action: String,
    params: serde_json::Value,
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, &'static str> {
        if s.len() % 2 != 0 {
            return Err("Odd number of hexadecimal digits");
        }
        let mut res = Vec::with_capacity(s.len() / 2);
        for i in (0..s.len()).step_by(2) {
            let byte = u8::from_str_radix(&s[i..i+2], 16).map_err(|_| "Invalid hex character")?;
            res.push(byte);
        }
        Ok(res)
    }
}


