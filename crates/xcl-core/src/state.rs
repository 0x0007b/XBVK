//! Lightweight runtime state shared with XCL subsystems.

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct ConfigState {
    pub series_s: bool,
}

impl ConfigState {
    pub fn is_series_s(&self) -> bool {
        self.series_s
    }
}

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static CONFIG: OnceLock<ConfigState> = OnceLock::new();

pub fn init(data_dir: PathBuf, series_s: bool) {
    let _ = DATA_DIR.set(data_dir);
    let _ = CONFIG.set(ConfigState { series_s });
}

pub fn config() -> &'static ConfigState {
    CONFIG.get_or_init(ConfigState::default)
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = DATA_DIR.get() {
        return dir.clone();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("XCLVIC").join("unit_data")
}
