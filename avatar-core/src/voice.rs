use std::error::Error;
use reqwest::Client;
use serde::Serialize;
use log::{info, error};

use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
use windows::Win32::Media::Speech::{ISpVoice, SpVoice};
use windows::core::PCWSTR;

pub struct VoiceManager {
    http_client: Client,
    whisper_url: String,
}

impl VoiceManager {
    pub fn new(whisper_url: Option<String>) -> Self {
        VoiceManager {
            http_client: Client::new(),
            whisper_url: whisper_url.unwrap_or_else(|| "http://127.0.0.1:8000/transcribe".to_string()),
        }
    }

    /// Invokes the native Windows TTS engine to speak a text phrase synchronously.
    pub fn speak(&self, text: &str) -> Result<(), Box<dyn Error>> {
        info!("Speaking: {}", text);
        unsafe {
            // Initialize COM library for the current thread
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            
            // Create instance of Windows Speech Synthesis SpVoice COM object
            let voice: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL)?;
            
            // Convert rust string to wide string for Win32 API compatibility
            let mut wide_text: Vec<u16> = text.encode_utf16().collect();
            wide_text.push(0); // Null-terminator
            let pcwstr = PCWSTR::from_raw(wide_text.as_ptr());
            
            let mut stream_num = 0u32;
            // Speak standard flags: 0 = Speak Synchronously (SPF_DEFAULT)
            voice.Speak(pcwstr, 0, Some(&mut stream_num))?;
        }
        Ok(())
    }

    /// Transcribes raw PCM audio data (from microphone) using local Whisper STT engine.
    pub async fn transcribe_audio(&self, pcm_data: Vec<f32>, sample_rate: u32) -> Result<String, Box<dyn Error>> {
        info!("Sending audio transcription request ({} samples)", pcm_data.len());
        
        // In production, we send the audio buffer to a local Whisper FastAPI server
        // formatted as a multipart form or binary payload.
        #[derive(Serialize)]
        struct TranscribePayload {
            sample_rate: u32,
            audio_data: Vec<f32>,
        }

        let payload = TranscribePayload {
            sample_rate,
            audio_data: pcm_data,
        };

        match self.http_client
            .post(&self.whisper_url)
            .json(&payload)
            .send()
            .await 
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    let text = resp.text().await?;
                    Ok(text)
                } else {
                    Err(format!("Whisper STT server returned status: {}", resp.status()).into())
                }
            }
            Err(e) => {
                error!("STT connection failed: {}. Falling back to default parser.", e);
                // Return a clear error for signaling or UI fallback
                Err(Box::new(e))
            }
        }
    }

    /// Checks a segment of text for the "Avatar" wake word.
    pub fn detect_wake_word(&self, text: &str) -> bool {
        let normalized = text.to_lowercase();
        normalized.contains("avatar")
    }
}
