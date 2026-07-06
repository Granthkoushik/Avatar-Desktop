use enigo::{Enigo, MouseControllable, KeyboardControllable, MouseButton, Key};
use std::process::Command;
use std::path::Path;
use log::{info, error};

pub struct AutomationController {
    enigo: Enigo,
}

impl AutomationController {
    pub fn new() -> Self {
        AutomationController { enigo: Enigo::new() }
    }

    // --- Mouse Emulation ---

    pub fn move_mouse(&mut self, x: i32, y: i32) {
        info!("Moving mouse to ({}, {})", x, y);
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.mouse_move_to(x, y);
        }));
        if let Err(e) = res {
            error!("Failed to move mouse (headless or permission block): {:?}", e);
        }
    }

    pub fn click(&mut self, button: &str) {
        info!("Clicking mouse button: {}", button);
        let btn = match button {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            "left" | _ => MouseButton::Left,
        };
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.mouse_click(btn);
        }));
        if let Err(e) = res {
            error!("Failed to click mouse: {:?}", e);
        }
    }

    pub fn mouse_down(&mut self, button: &str) {
        let btn = match button {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            "left" | _ => MouseButton::Left,
        };
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.mouse_down(btn);
        }));
        if let Err(e) = res {
            error!("Failed to press mouse down: {:?}", e);
        }
    }

    pub fn mouse_up(&mut self, button: &str) {
        let btn = match button {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            "left" | _ => MouseButton::Left,
        };
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.mouse_up(btn);
        }));
        if let Err(e) = res {
            error!("Failed to release mouse: {:?}", e);
        }
    }

    pub fn scroll(&mut self, clicks: i32, direction: &str) {
        info!("Scrolling mouse: {} clicks {}", clicks, direction);
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match direction {
                "up" => enigo.mouse_scroll_y(clicks),
                "down" => enigo.mouse_scroll_y(-clicks),
                "left" => enigo.mouse_scroll_x(clicks),
                "right" => enigo.mouse_scroll_x(-clicks),
                _ => {}
            }
        }));
        if let Err(e) = res {
            error!("Failed to scroll mouse: {:?}", e);
        }
    }

    // --- Keyboard Emulation ---

    pub fn type_text(&mut self, text: &str) {
        info!("Typing text: {}", text);
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.key_sequence(text);
        }));
        if let Err(e) = res {
            error!("Failed to type text: {:?}", e);
        }
    }

    pub fn press_key(&mut self, key_name: &str) {
        info!("Pressing special key: {}", key_name);
        if let Some(key) = self.parse_key(key_name) {
            let enigo = &mut self.enigo;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                enigo.key_click(key);
            }));
            if let Err(e) = res {
                error!("Failed to press key: {:?}", e);
            }
        }
    }

    pub fn press_shortcut(&mut self, shortcut: &str) -> Result<(), String> {
        info!("Pressing shortcut: {}", shortcut);
        let parts: Vec<&str> = shortcut.split(|c| c == '+' || c == '-' || c == ' ').collect();
        
        let mut parsed_keys = Vec::new();
        for part in parts {
            if let Some(key) = self.parse_key(part) {
                parsed_keys.push(key);
            } else {
                if part.len() == 1 {
                    let c = part.chars().next().unwrap();
                    parsed_keys.push(Key::Layout(c));
                } else {
                    return Err(format!("Unknown key part in shortcut: {}", part));
                }
            }
        }

        if parsed_keys.is_empty() {
            return Err("Empty shortcut".to_string());
        }

        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for i in 0..parsed_keys.len() - 1 {
                enigo.key_down(parsed_keys[i]);
            }
            if let Some(&last_key) = parsed_keys.last() {
                enigo.key_click(last_key);
            }
            for i in (0..parsed_keys.len() - 1).rev() {
                enigo.key_up(parsed_keys[i]);
            }
        }));
        
        if let Err(e) = res {
            error!("Failed to press shortcut: {:?}", e);
            return Err("Headless or permission block executing shortcut".to_string());
        }

        Ok(())
    }

    // --- Application & System Control ---

    pub fn launch_app(&self, app_path: &str) -> Result<(), String> {
        let app_name = app_path.trim().to_lowercase();
        
        // Handle URL inputs or common websites dynamically
        let has_tld_or_dot = app_name.contains('.') && 
            !app_name.ends_with(".exe") && 
            !app_name.ends_with(".bat") && 
            !app_name.ends_with(".cmd") && 
            !app_name.ends_with(".lnk");

        if app_path.starts_with("http://") || app_path.starts_with("https://") {
            return self.launch_url(app_path);
        }
        if has_tld_or_dot {
            return self.launch_url(&format!("https://{}", app_path));
        }

        if app_name == "youtube" || app_name.contains("youtube") {
            return self.launch_url("https://www.youtube.com");
        }
        if app_name == "google" || app_name.contains("google.com") {
            return self.launch_url("https://www.google.com");
        }
        if app_name == "gmail" {
            return self.launch_url("https://mail.google.com");
        }
        if app_name == "github" {
            return self.launch_url("https://github.com");
        }
        
        let target = match app_name.as_str() {
            "chrome" | "google chrome" | "browser" => "chrome".to_string(),
            "edge" | "microsoft edge" => "msedge".to_string(),
            "notepad" | "text editor" => "notepad".to_string(),
            "calculator" | "calc" => "calc".to_string(),
            "paint" | "mspaint" => "mspaint".to_string(),
            "word" | "ms word" | "microsoft word" => "winword".to_string(),
            "excel" | "ms excel" | "microsoft excel" => "excel".to_string(),
            "explorer" | "file explorer" | "files" => "explorer".to_string(),
            "cmd" | "command prompt" => "cmd".to_string(),
            "powershell" => "powershell".to_string(),
            _ => {
                // Try to find in Start Menu / Programs
                if let Some(shortcut) = find_start_menu_shortcut(&app_name) {
                    shortcut
                } else {
                    app_path.to_string()
                }
            }
        };
        info!("Launching application: {} (resolved as: {})", app_path, target);
        // Use cmd shell so paths and environment variables solve correctly
        Command::new("cmd")
            .args(&["/C", "start", "", &target])
            .spawn()
            .map_err(|e| format!("Failed to launch app: {}", e))?;
        Ok(())
    }

    pub fn launch_url(&self, url: &str) -> Result<(), String> {
        let mut final_url = url.trim().to_string();
        if !final_url.starts_with("http://") && !final_url.starts_with("https://") {
            final_url = format!("https://{}", final_url);
        }
        info!("Opening URL: {}", final_url);
        Command::new("cmd")
            .args(&["/C", "start", &final_url])
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
        Ok(())
    }

    pub fn unlock_os(&mut self, password: &str) -> Result<(), String> {
        info!("Initiating OS unlock sequence...");
        let enigo = &mut self.enigo;
        
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Wake up display and dismiss lock screen
            enigo.key_click(Key::Space);
            std::thread::sleep(std::time::Duration::from_millis(800));
            
            // Clear any stray characters
            enigo.key_click(Key::Backspace);
            enigo.key_click(Key::Backspace);
            std::thread::sleep(std::time::Duration::from_millis(150));
            
            // Type password characters
            for c in password.chars() {
                enigo.key_click(Key::Layout(c));
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            
            // Press enter to log in
            enigo.key_click(Key::Return);
        }));
        
        if let Err(e) = res {
            error!("Failed to execute OS unlock: {:?}", e);
            return Err("Simulation panic".to_string());
        }
        Ok(())
    }

    pub fn adjust_volume(&mut self, action: &str) {
        info!("Adjusting system volume: {}", action);
        let key = match action {
            "up" => Key::VolumeUp,
            "down" => Key::VolumeDown,
            "mute" => Key::Raw(0xAD),
            _ => return,
        };
        let enigo = &mut self.enigo;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            enigo.key_click(key);
        }));
        if let Err(e) = res {
            error!("Failed to press volume key: {:?}", e);
        }
    }

    pub fn adjust_brightness(&self, level: u8) -> Result<(), String> {
        info!("Adjusting system screen brightness to: {}%", level);
        let capped = std::cmp::min(level, 100);
        // Execute WMI script via PowerShell to configure brightness
        let script = format!(
            "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(0, {})",
            capped
        );
        Command::new("powershell")
            .args(&["-Command", &script])
            .output()
            .map_err(|e| format!("Failed to adjust brightness: {}", e))?;
        Ok(())
    }

    pub fn execute_power_action(&self, action: &str) -> Result<(), String> {
        info!("Executing system power command: {}", action);
        match action {
            "lock" => {
                Command::new("rundll32.exe")
                    .args(&["user32.dll,LockWorkStation"])
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "sleep" => {
                // SuspendState: sleep (0,1,0)
                Command::new("rundll32.exe")
                    .args(&["powrprof.dll,SetSuspendState", "0,1,0"])
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "shutdown" => {
                Command::new("shutdown")
                    .args(&["/s", "/t", "10"]) // give user 10 second warning
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            "restart" => {
                Command::new("shutdown")
                    .args(&["/r", "/t", "10"])
                    .spawn()
                    .map_err(|e| e.to_string())?;
            }
            _ => return Err("Unknown power action".to_string()),
        }
        Ok(())
    }

    // --- Private Helper to Parse Keys ---

    fn parse_key(&self, key_name: &str) -> Option<Key> {
        match key_name.to_lowercase().as_str() {
            "enter" => Some(Key::Return),
            "space" => Some(Key::Space),
            "backspace" => Some(Key::Backspace),
            "tab" => Some(Key::Tab),
            "escape" | "esc" => Some(Key::Escape),
            "up" => Some(Key::UpArrow),
            "down" => Some(Key::DownArrow),
            "left" => Some(Key::LeftArrow),
            "right" => Some(Key::RightArrow),
            "meta" | "win" | "super" => Some(Key::Meta),
            "control" | "ctrl" => Some(Key::Control),
            "alt" => Some(Key::Alt),
            "shift" => Some(Key::Shift),
            "f1" => Some(Key::F1),
            "f2" => Some(Key::F2),
            "f3" => Some(Key::F3),
            "f4" => Some(Key::F4),
            "f5" => Some(Key::F5),
            "f6" => Some(Key::F6),
            "f7" => Some(Key::F7),
            "f8" => Some(Key::F8),
            "f9" => Some(Key::F9),
            "f10" => Some(Key::F10),
            "f11" => Some(Key::F11),
            _ => None,
        }
    }
}

fn find_start_menu_shortcut(app_name: &str) -> Option<String> {
    let app_name_lower = app_name.to_lowercase();
    
    // Start menu paths
    let mut search_paths = vec![
        std::path::PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs")
    ];
    
    if let Ok(appdata) = std::env::var("APPDATA") {
        let user_start = std::path::Path::new(&appdata).join("Microsoft\\Windows\\Start Menu\\Programs");
        search_paths.push(user_start);
    }
    
    // Walk and find matching shortcut
    for path in search_paths {
        if let Some(matched) = walk_and_find(&path, &app_name_lower) {
            return Some(matched);
        }
    }
    
    // Fallback: Check local appdata/programs
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let programs_dir = std::path::Path::new(&local_appdata).join("Programs");
        if programs_dir.exists() {
            if let Some(matched) = walk_and_find(&programs_dir, &app_name_lower) {
                return Some(matched);
            }
        }
    }
    
    None
}

fn walk_and_find(dir: &std::path::Path, target_lower: &str) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    
    let mut entries_to_visit = vec![dir.to_path_buf()];
    let mut count = 0;
    
    while let Some(current_dir) = entries_to_visit.pop() {
        count += 1;
        if count > 2000 {
            break; // prevent infinite loops or performance hits
        }
        if let Ok(read_dir) = std::fs::read_dir(current_dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    entries_to_visit.push(path);
                } else if path.is_file() {
                    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                        let f_lower = file_name.to_lowercase();
                        if (f_lower.ends_with(".lnk") || f_lower.ends_with(".exe")) && f_lower.contains(target_lower) {
                            return Some(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_launch_app_url_redirection() {
        let controller = AutomationController::new();
        // Verify launch_app runs without errors for URLs and common keywords
        let res = controller.launch_app("https://www.youtube.com");
        assert!(res.is_ok());

        let res = controller.launch_app("youtube");
        assert!(res.is_ok());

        let res = controller.launch_app("youtube.com");
        assert!(res.is_ok());
    }
}
