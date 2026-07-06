use serde::{Serialize, Deserialize};
use crate::automation::AutomationController;
use crate::system_monitor::{SystemMonitor, SystemMetrics};
use log::{warn, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemIntent {
    pub action: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ExecutionResult {
    Success(String),
    PermissionRequired {
        action: String,
        details: String,
        token: String, // Unique token to approve this specific execution later
    },
    Failed(String),
}

pub struct IntentDispatcher {
    automation: AutomationController,
    monitor: SystemMonitor,
}

impl IntentDispatcher {
    pub fn new() -> Self {
        IntentDispatcher {
            automation: AutomationController::new(),
            monitor: SystemMonitor::new(),
        }
    }

    pub fn get_system_metrics(&mut self) -> SystemMetrics {
        self.monitor.get_metrics()
    }

    /// Validates safety class and executes the intent.
    /// Halts execution if permission confirmation is required.
    pub fn dispatch_intent(&mut self, intent: SystemIntent) -> ExecutionResult {
        info!("Dispatching intent action: {}", intent.action);

        // Security check: classify the operation
        if self.is_dangerous(&intent) {
            let details = format!("Action: {}, Params: {}", intent.action, intent.params);
            warn!("Dangerous operation halted for approval: {}", details);
            let approval_token = uuid::Uuid::new_v4().to_string();
            return ExecutionResult::PermissionRequired {
                action: intent.action,
                details,
                token: approval_token,
            };
        }

        // Safe operations: execute immediately
        match self.execute_safe_action(intent) {
            Ok(msg) => ExecutionResult::Success(msg),
            Err(e) => ExecutionResult::Failed(e),
        }
    }

    /// Executes dangerous actions after the user has manual-approved them.
    pub fn execute_approved_action(&mut self, intent: SystemIntent) -> ExecutionResult {
        info!("Executing approved action: {}", intent.action);
        match self.execute_action_force(intent) {
            Ok(msg) => ExecutionResult::Success(msg),
            Err(e) => ExecutionResult::Failed(e),
        }
    }

    /// Evaluates if the intent corresponds to a dangerous system command.
    fn is_dangerous(&self, intent: &SystemIntent) -> bool {
        match intent.action.as_str() {
            "power_action" => {
                // Sleep and lock are safe, but restart and shutdown are dangerous
                if let Some(act) = intent.params.get("action").and_then(|v| v.as_str()) {
                    return act == "shutdown" || act == "restart";
                }
                true
            }
            "run_script" | "terminal_command" | "delete_file" | "format_drive" => true,
            "open_app" => {
                // Some applications are blacklisted from direct opening without confirm
                if let Some(path) = intent.params.get("path").and_then(|v| v.as_str()) {
                    let p_lower = path.to_lowercase();
                    return p_lower.contains("cmd.exe") 
                        || p_lower.contains("powershell.exe")
                        || p_lower.contains("regedit.exe")
                        || p_lower.contains("format");
                }
                false
            }
            _ => false,
        }
    }

    /// Dispatcher function for safe/validated actions.
    fn execute_safe_action(&mut self, intent: SystemIntent) -> Result<String, String> {
        match intent.action.as_str() {
            "move_mouse" => {
                let x = intent.params.get("x").and_then(|v| v.as_i64()).ok_or("Missing x coordinate")? as i32;
                let y = intent.params.get("y").and_then(|v| v.as_i64()).ok_or("Missing y coordinate")? as i32;
                self.automation.move_mouse(x, y);
                Ok("Mouse moved".to_string())
            }
            "click" => {
                let button = intent.params.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                self.automation.click(button);
                Ok(format!("Clicked {}", button))
            }
            "mouse_down" => {
                let button = intent.params.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                self.automation.mouse_down(button);
                Ok(format!("Mouse button down: {}", button))
            }
            "mouse_up" => {
                let button = intent.params.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                self.automation.mouse_up(button);
                Ok(format!("Mouse button up: {}", button))
            }
            "scroll" => {
                let clicks = intent.params.get("clicks").and_then(|v| v.as_i64()).unwrap_or(1) as i32;
                let direction = intent.params.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
                self.automation.scroll(clicks, direction);
                Ok(format!("Scrolled {} {}", clicks, direction))
            }
            "type_text" => {
                let text = intent.params.get("text").and_then(|v| v.as_str()).ok_or("Missing text payload")?;
                self.automation.type_text(text);
                Ok("Text typed".to_string())
            }
            "press_key" => {
                let key = intent.params.get("key").and_then(|v| v.as_str()).ok_or("Missing key code")?;
                self.automation.press_key(key);
                Ok(format!("Pressed key: {}", key))
            }
            "press_shortcut" => {
                let shortcut = intent.params.get("shortcut").and_then(|v| v.as_str()).ok_or("Missing shortcut parameter")?;
                self.automation.press_shortcut(shortcut)?;
                Ok(format!("Shortcut executed: {}", shortcut))
            }
            "unlock_os" => {
                let password = intent.params.get("password").and_then(|v| v.as_str()).ok_or("Missing password parameter")?;
                self.automation.unlock_os(password)?;
                Ok("OS unlocked".to_string())
            }
            "open_url" => {
                let url = intent.params.get("url").and_then(|v| v.as_str()).ok_or("Missing url parameter")?;
                self.automation.launch_url(url)?;
                Ok(format!("Opened URL: {}", url))
            }
            "open_app" => {
                // If it bypassed is_dangerous, it's a safe app
                let path = intent.params.get("path").and_then(|v| v.as_str()).ok_or("Missing application path")?;
                self.automation.launch_app(path)?;
                Ok(format!("Launched: {}", path))
            }
            "adjust_volume" => {
                let level = intent.params.get("action").and_then(|v| v.as_str()).ok_or("Missing action parameter (up/down/mute)")?;
                self.automation.adjust_volume(level);
                Ok(format!("Volume adjusted: {}", level))
            }
            "adjust_brightness" => {
                let level = intent.params.get("level").and_then(|v| v.as_i64()).ok_or("Missing brightness level")? as u8;
                self.automation.adjust_brightness(level)?;
                Ok(format!("Brightness adjusted to: {}%", level))
            }
            "power_action" => {
                // Safe actions are sleep and lock
                let action = intent.params.get("action").and_then(|v| v.as_str()).ok_or("Missing action parameter")?;
                if action == "sleep" || action == "lock" {
                    self.automation.execute_power_action(action)?;
                    Ok(format!("System action executed: {}", action))
                } else {
                    Err("Action requires manual override authorization".to_string())
                }
            }
            "get_metrics" => {
                let metrics = self.monitor.get_metrics();
                let metrics_json = serde_json::to_string(&metrics).map_err(|e| e.to_string())?;
                Ok(metrics_json)
            }
            _ => Err(format!("Unsupported or unknown action: {}", intent.action)),
        }
    }

    /// Dispatcher function bypassing the dangerous check (runs after manual override).
    fn execute_action_force(&mut self, intent: SystemIntent) -> Result<String, String> {
        match intent.action.as_str() {
            "power_action" => {
                let action = intent.params.get("action").and_then(|v| v.as_str()).ok_or("Missing action parameter")?;
                self.automation.execute_power_action(action)?;
                Ok(format!("Power operation executed: {}", action))
            }
            "open_app" => {
                let path = intent.params.get("path").and_then(|v| v.as_str()).ok_or("Missing app path")?;
                self.automation.launch_app(path)?;
                Ok(format!("Launched: {}", path))
            }
            // Add custom scripts/deletions force handlers here...
            _ => self.execute_safe_action(intent),
        }
    }
}

pub fn extract_json_intent(input: &str) -> Option<SystemIntent> {
    let trimmed = input.trim();
    
    let parse_val = |s: &str| -> Option<SystemIntent> {
        let val: serde_json::Value = serde_json::from_str(s).ok()?;
        let action = val.get("action")?.as_str()?.to_string();
        
        let params = if let Some(p) = val.get("params") {
            p.clone()
        } else {
            // Flat fallback: treat everything except action as params
            if let serde_json::Value::Object(mut map) = val {
                map.remove("action");
                serde_json::Value::Object(map)
            } else {
                serde_json::Value::Object(serde_json::Map::new())
            }
        };
        
        Some(SystemIntent { action, params })
    };

    if let Some(intent) = parse_val(trimmed) {
        return Some(intent);
    }
    
    if let (Some(start_idx), Some(end_idx)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start_idx < end_idx {
            let candidate = &trimmed[start_idx..=end_idx];
            if let Some(intent) = parse_val(candidate) {
                return Some(intent);
            }
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_intent() {
        // Direct parse
        let raw_json = r#"{"action":"open_app","params":{"path":"notepad"}}"#;
        let res = extract_json_intent(raw_json).unwrap();
        assert_eq!(res.action, "open_app");
        assert_eq!(res.params["path"], "notepad");

        // Markdown code blocks
        let md_wrapped = r#"```json
        {"action":"open_app","params":{"path":"chrome"}}
        ```"#;
        let res = extract_json_intent(md_wrapped).unwrap();
        assert_eq!(res.action, "open_app");
        assert_eq!(res.params["path"], "chrome");

        // Conversational wrapper
        let conversational = r#"Sure! I will open calculator for you: {"action": "open_app", "params": {"path": "calc"}} Hope that helps!"#;
        let res = extract_json_intent(conversational).unwrap();
        assert_eq!(res.action, "open_app");
        assert_eq!(res.params["path"], "calc");

        // Flat format direct parse
        let flat_json = r#"{"action":"open_app","path":"thonny"}"#;
        let res = extract_json_intent(flat_json).unwrap();
        assert_eq!(res.action, "open_app");
        assert_eq!(res.params["path"], "thonny");

        // Flat format url redirect check
        let flat_url = r#"{"action":"open_url","url":"youtube.com"}"#;
        let res = extract_json_intent(flat_url).unwrap();
        assert_eq!(res.action, "open_url");
        assert_eq!(res.params["url"], "youtube.com");

        // Invalid JSON
        assert!(extract_json_intent("hello world").is_none());
        assert!(extract_json_intent(r#"{"action": "open_app", "#).is_none());
    }
}
