use serde::{Serialize, Deserialize};
use reqwest::Client;
use std::error::Error;
use log::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "system", "user", "assistant"
    pub content: String,
}

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelInfo>,
}

#[derive(Deserialize)]
struct OllamaModelInfo {
    name: String,
}

pub struct AiManager {
    client: Client,
    ollama_url: String,
    default_model: String,
    system_prompt: String,
}

impl AiManager {
    pub fn new(ollama_url: Option<String>, default_model: Option<String>) -> Self {
        let system_prompt = "You are Avatar, a private and secure personal desktop AI assistant. \
        You have direct integration with the user's host operating system. You can execute system automation commands \
        by returning a single raw valid JSON object matching the SystemIntent schema. DO NOT wrap JSON in markdown blocks. \
        Do not output any introductory or conversational text when executing a system command—only return the raw JSON object. \
        If the user's request is purely conversational or a question, reply with plain conversational text. \
        \
        SystemIntent JSON schema: \
        { \
          \"action\": \"<action_name>\", \
          \"params\": { ... } \
        } \
        \
        Supported actions and parameter schemas: \
        1. \"open_app\": Open an application or system process. Parameter: \"path\" (string - e.g., \"notepad\", \"calc\", \"chrome\", \"explorer\"). \
           Example: {\"action\": \"open_app\", \"params\": {\"path\": \"notepad\"}} \
        2. \"press_shortcut\": Execute key combinations (useful for switching tabs/apps, closing windows). Parameter: \"shortcut\" (string - e.g., \"ctrl+tab\", \"alt+tab\", \"ctrl+t\", \"alt+f4\", \"win+d\"). \
           Example: {\"action\": \"press_shortcut\", \"params\": {\"shortcut\": \"ctrl+tab\"}} \
        3. \"adjust_volume\": Control volume. Parameter: \"action\" (string - \"up\", \"down\", or \"mute\"). \
           Example: {\"action\": \"adjust_volume\", \"params\": {\"action\": \"up\"}} \
        4. \"adjust_brightness\": Control display brightness. Parameter: \"level\" (integer from 0 to 100). \
           Example: {\"action\": \"adjust_brightness\", \"params\": {\"level\": 80}} \
        5. \"power_action\": Power controls. Parameter: \"action\" (string - \"sleep\", \"lock\", \"shutdown\", or \"restart\"). \
           Example: {\"action\": \"power_action\", \"params\": {\"action\": \"sleep\"}} \
        6. \"open_url\": Open a URL or search Google. Parameter: \"url\" (string). Construct google search queries like https://www.google.com/search?q=query if user asks to search or look up anything. \
           Example: {\"action\": \"open_url\", \"params\": {\"url\": \"https://www.google.com/search?q=how+to+bake+a+cake\"}} \
        7. \"type_text\": Type text at cursor. Parameter: \"text\" (string). \
           Example: {\"action\": \"type_text\", \"params\": {\"text\": \"hello\"}} \
        8. \"press_key\": Click a special keyboard key. Parameter: \"key\" (string - e.g., \"enter\", \"space\", \"backspace\", \"tab\", \"escape\"). \
           Example: {\"action\": \"press_key\", \"params\": {\"key\": \"enter\"}} \
        9. \"get_metrics\": Retrieve system telemetry resource logs. Parameter: none. \
           Example: {\"action\": \"get_metrics\", \"params\": {}} \
        \
        Always prioritize safety. Refuse destructive requests like system formats. Be concise.".to_string();

        AiManager {
            client: Client::new(),
            ollama_url: ollama_url.unwrap_or_else(|| "http://127.0.0.1:11434".to_string()),
            default_model: default_model.unwrap_or_else(|| "llama3".to_string()),
            system_prompt,
        }
    }

    /// Fetches all models currently pulled on the local Ollama instance.
    pub async fn get_available_models(&self) -> Result<Vec<String>, Box<dyn Error>> {
        let endpoint = format!("{}/api/tags", self.ollama_url);
        let response = self.client.get(&endpoint).send().await?;
        if response.status().is_success() {
            let tags: OllamaTagsResponse = response.json().await?;
            let names = tags.models.into_iter().map(|m| m.name).collect();
            Ok(names)
        } else {
            Err(format!("Failed to retrieve models from tags API: {}", response.status()).into())
        }
    }

    /// Queries the local Ollama instance with conversational history.
    pub async fn query_llm(&self, model: Option<&str>, history: &[ChatMessage]) -> Result<String, Box<dyn Error>> {
        let mut model_name = model.unwrap_or(&self.default_model).to_string();

        // Dynamically resolve model name from tags if direct matching fails
        if let Ok(available) = self.get_available_models().await {
            if !available.contains(&model_name) {
                let latest_tag = format!("{}:latest", model_name);
                if available.contains(&latest_tag) {
                    model_name = latest_tag;
                } else if let Some(matched) = available.iter().find(|m| m.starts_with(&format!("{}:", model_name))) {
                    model_name = matched.clone();
                } else if !available.is_empty() {
                    // Fallback to first available model to prevent hard crash
                    model_name = available[0].clone();
                }
            }
        }
        
        // Prep message context, injecting our core identity system prompt
        let mut messages = vec![ChatMessage {
            role: "system".to_string(),
            content: self.system_prompt.clone(),
        }];
        messages.extend_from_slice(history);

        // Sanitize the inputs before sending them to the LLM
        for msg in &messages {
            if self.detect_prompt_injection(&msg.content) {
                warn!("Potential prompt injection detected: {}", msg.content);
                return Err("Security Violation: Request contains disallowed instruction overrides.".into());
            }
        }

        let request_payload = OllamaChatRequest {
            model: model_name.clone(),
            messages,
            stream: false,
        };

        let endpoint = format!("{}/api/chat", self.ollama_url);
        info!("Sending request to Ollama endpoint: {} for model: {}", endpoint, model_name);

        let response = self.client
            .post(&endpoint)
            .json(&request_payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Ollama server error: Code {}. Body: {}", status, body).into());
        }

        let chat_response: OllamaChatResponse = response.json().await?;
        Ok(chat_response.message.content)
    }

    /// Basic defensive input validation looking for command override sequences.
    fn detect_prompt_injection(&self, text: &str) -> bool {
        let lowercase = text.to_lowercase();
        
        // Signatures of prompt redirection attacks
        let indicators = [
            "ignore previous instructions",
            "override system prompt",
            "you are no longer avatar",
            "developer mode",
            "bypass system check",
            "system shutdown", // simple keyword guards
        ];

        for indicator in &indicators {
            if lowercase.contains(indicator) {
                return true;
            }
        }
        false
    }
}

