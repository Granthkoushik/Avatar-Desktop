pub mod db;
pub mod security;
pub mod audit;
pub mod ai;
pub mod memory;
pub mod voice;
pub mod system_monitor;
pub mod automation;
pub mod intent;
pub mod capture;
pub mod camera;
pub mod streaming;

pub fn init_logger() {
    let _ = env_logger::builder().filter_level(log::LevelFilter::Info).try_init();
}
