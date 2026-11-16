use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bootstrap config, kept in the default app-data folder so the app can find a
/// relocated database at startup. The database itself may live elsewhere.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

pub fn read(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim_start_matches('\u{feff}').to_string())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn write(path: &Path, config: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, format!("{text}\n"))
}
