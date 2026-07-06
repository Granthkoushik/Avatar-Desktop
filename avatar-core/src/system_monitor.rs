use sysinfo::{System, Disks};
use serde::Serialize;

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1};
use windows::Win32::Foundation::{HWND, LPARAM, BOOL};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible, GetWindowTextW, GetWindowThreadProcessId};

#[derive(Serialize, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub vram_dedicated_bytes: u64,
}

#[derive(Serialize, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub memory_bytes: u64,
    pub cpu_usage_pct: f32,
}

#[derive(Serialize, Clone)]
pub struct SystemMetrics {
    pub cpu_usage_pct: f32,
    pub ram_total_bytes: u64,
    pub ram_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub battery_pct: f32,
    pub is_charging: bool,
    pub gpus: Vec<GpuInfo>,
    pub running_processes: Vec<ProcessInfo>,
    pub open_browser_tabs: Vec<String>,
}

pub struct SystemMonitor {
    sys: System,
}

impl SystemMonitor {
    pub fn new() -> Self {
        SystemMonitor { sys: System::new() }
    }

    /// Pulls current metrics from the system.
    pub fn get_metrics(&mut self) -> SystemMetrics {
        self.sys.refresh_all();

        // Calculate aggregate CPU usage
        let cpus = self.sys.cpus();
        let cpu_sum: f32 = cpus.iter().map(|cpu| cpu.cpu_usage()).sum();
        let cpu_usage_pct = if !cpus.is_empty() {
            cpu_sum / cpus.len() as f32
        } else {
            0.0
        };

        let ram_total_bytes = self.sys.total_memory();
        let ram_used_bytes = self.sys.used_memory();

        // Calculate aggregated storage
        let mut disk_total_bytes = 0u64;
        let mut disk_used_bytes = 0u64;
        let disks = Disks::new_with_refreshed_list();
        for disk in &disks {
            disk_total_bytes += disk.total_space();
            disk_used_bytes += disk.total_space() - disk.available_space();
        }

        // Get GPU metrics via DXGI
        let gpus = Self::query_gpus_dxgi();

        // Gather process table, sorted by memory consumption (top 15)
        let mut running_processes = Vec::new();
        for (pid, process) in self.sys.processes() {
            running_processes.push(ProcessInfo {
                pid: pid.as_u32(),
                name: process.name().to_string(),
                memory_bytes: process.memory(),
                cpu_usage_pct: process.cpu_usage(),
            });
        }
        running_processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        running_processes.truncate(15);

        // Simple battery query placeholder or stub if API isn't present
        // (Windows battery can be queried via GetSystemPowerStatus Win32 API)
        let (battery_pct, is_charging) = Self::query_battery_status();

        let open_browser_tabs = self.get_browser_windows();

        SystemMetrics {
            cpu_usage_pct,
            ram_total_bytes,
            ram_used_bytes,
            disk_total_bytes,
            disk_used_bytes,
            battery_pct,
            is_charging,
            gpus,
            running_processes,
            open_browser_tabs,
        }
    }

    /// Queries GPU hardware descriptions utilizing the DXGI subsystem.
    fn query_gpus_dxgi() -> Vec<GpuInfo> {
        let mut gpus = Vec::new();
        unsafe {
            // Create the DXGI Factory
            let factory_res: Result<IDXGIFactory1, _> = CreateDXGIFactory1();
            if let Ok(factory) = factory_res {
                let mut index = 0;
                // Enumerate adapters
                while let Ok(adapter) = factory.EnumAdapters1(index) {
                    let mut desc = DXGI_ADAPTER_DESC1::default();
                    if adapter.GetDesc1(&mut desc).is_ok() {
                        let raw_name = String::from_utf16_lossy(&desc.Description);
                        let clean_name = raw_name.trim_matches(char::from(0)).to_string();
                        
                        gpus.push(GpuInfo {
                            name: clean_name,
                            vram_dedicated_bytes: desc.DedicatedVideoMemory as u64,
                        });
                    }
                    index += 1;
                }
            }
        }
        gpus
    }

    /// Direct query of Windows Power Management subsystem for battery statuses.
    fn query_battery_status() -> (f32, bool) {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe {
            if GetSystemPowerStatus(&mut status).is_ok() {
                let pct = if status.BatteryLifePercent == 255 {
                    100.0 // fallback
                } else {
                    status.BatteryLifePercent as f32
                };
                let charging = (status.ACLineStatus == 1) || (status.BatteryFlag & 8 != 0);
                return (pct, charging);
            }
        }
        (100.0, true) // Default fallback if no battery hardware
    }

    /// Fetches all visible browser window/tab titles.
    pub fn get_browser_windows(&self) -> Vec<String> {
        unsafe extern "system" fn enum_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let state = &mut *(lparam.0 as *mut (Vec<String>, &System));
            let titles = &mut state.0;
            let sys = state.1;
            
            if IsWindowVisible(hwnd).as_bool() {
                let mut text = [0u16; 512];
                let len = GetWindowTextW(hwnd, &mut text);
                if len > 0 {
                    let title = String::from_utf16_lossy(&text[..len as usize]);
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    
                    let found_proc = sys.processes().iter().find(|(p, _)| p.as_u32() == pid);
                    if let Some((_, proc)) = found_proc {
                        let proc_name = proc.name().to_lowercase();
                        if proc_name.contains("chrome")
                            || proc_name.contains("msedge")
                            || proc_name.contains("firefox")
                            || proc_name.contains("brave")
                            || proc_name.contains("opera")
                        {
                            let trimmed_title = title.trim();
                            if !trimmed_title.is_empty() && !titles.contains(&trimmed_title.to_string()) {
                                titles.push(trimmed_title.to_string());
                            }
                        }
                    }
                }
            }
            BOOL::from(true)
        }

        let mut state = (Vec::<String>::new(), &self.sys);
        unsafe {
            let lparam = LPARAM(&mut state as *mut _ as isize);
            let _ = EnumWindows(Some(enum_window_callback), lparam);
        }
        state.0
    }
}
