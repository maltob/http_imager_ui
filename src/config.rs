use configparser::ini::Ini;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployConfig {
    pub format_script: PathBuf,
    pub temp_wim_path: PathBuf,
    pub temp_file_path: PathBuf,
    pub download_url: String,
    pub image_index: u32,
    pub auto_install: bool,
    pub auto_reboot: bool,
    pub target_disk: Option<u32>,
    pub target_serial: Option<String>,
    pub expected_sha256: Option<String>,
    pub expected_size: Option<u64>,
    pub check_url: String,
    pub continue_on_optional_error: bool,
    pub stage_folder: Option<String>,
    pub stage_download_zip: Option<String>,
    pub driver_download_zip: Option<String>,
    pub unattend_download_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn optional_value(config: &Ini, section: &str, key: &str) -> Option<String> {
    config
        .get(section, key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool(config: &Ini, section: &str, key: &str, default: bool) -> bool {
    config
        .getbool(section, key)
        .ok()
        .flatten()
        .unwrap_or(default)
}

fn resolve_path(base: &Path, value: Option<String>, default: &str) -> PathBuf {
    let path = PathBuf::from(value.unwrap_or_else(|| default.to_string()));
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

impl DeployConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut ini = Ini::new();
        if path.exists() {
            ini.load(path)
                .map_err(|error| ConfigError(error.to_string()))?;
        }

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let download_url = optional_value(&ini, "os", "download_url")
            .ok_or_else(|| ConfigError("[os] download_url is required".to_string()))?;
        let image_index = optional_value(&ini, "os", "index")
            .unwrap_or_else(|| "1".to_string())
            .parse::<u32>()
            .map_err(|_| ConfigError("[os] index must be a positive integer".to_string()))?;
        if image_index == 0 {
            return Err(ConfigError(
                "[os] index must be greater than zero".to_string(),
            ));
        }

        let target_disk = optional_value(&ini, "target", "disk")
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|_| ConfigError("[target] disk must be a physical disk number".to_string()))?;
        let expected_size = optional_value(&ini, "os", "size")
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| ConfigError("[os] size must be a byte count".to_string()))?;
        let expected_sha256 =
            optional_value(&ini, "os", "sha256").map(|value| value.to_ascii_lowercase());
        if let Some(hash) = &expected_sha256
            && (hash.len() != 64 || !hash.chars().all(|character| character.is_ascii_hexdigit()))
        {
            return Err(ConfigError(
                "[os] sha256 must be 64 hexadecimal characters".to_string(),
            ));
        }

        let check_url =
            optional_value(&ini, "network", "check_url").unwrap_or_else(|| download_url.clone());
        let driver_download_zip = optional_value(&ini, "deploy", "driver_download_zip")
            .or_else(|| optional_value(&ini, "deploy", "drivers_download_zip"));

        Ok(Self {
            format_script: resolve_path(
                base,
                optional_value(&ini, "storage", "format_script"),
                "DiskPart.txt",
            ),
            temp_wim_path: resolve_path(
                base,
                optional_value(&ini, "storage", "temp_wim_path"),
                r"W:\install.wim",
            ),
            temp_file_path: resolve_path(
                base,
                optional_value(&ini, "storage", "temp_file_path"),
                r"X:\temp",
            ),
            download_url,
            image_index,
            auto_install: parse_bool(&ini, "deploy", "auto_install", false),
            auto_reboot: parse_bool(&ini, "deploy", "auto_reboot", false),
            target_disk,
            target_serial: optional_value(&ini, "target", "serial"),
            expected_sha256,
            expected_size,
            check_url,
            continue_on_optional_error: parse_bool(
                &ini,
                "network",
                "download_continue_on_error",
                false,
            ),
            stage_folder: optional_value(&ini, "deploy", "stage_folder"),
            stage_download_zip: optional_value(&ini, "deploy", "stage_download_zip"),
            driver_download_zip,
            unattend_download_path: optional_value(&ini, "deploy", "unattend_download_path"),
        })
    }

    pub fn load_from_executable() -> Result<Self, ConfigError> {
        let executable = std::env::current_exe().map_err(|error| ConfigError(error.to_string()))?;
        let path = executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("Settings.ini");
        Self::load(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn blank_check_url_uses_download_url_and_relative_paths_use_config_directory() {
        let directory =
            std::env::temp_dir().join(format!("http-imager-config-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Settings.ini");
        fs::write(
            &path,
            "[os]\ndownload_url=http://server/image.wim\n[network]\ncheck_url=\n",
        )
        .unwrap();
        let config = DeployConfig::load(&path).unwrap();
        assert_eq!(config.check_url, "http://server/image.wim");
        assert_eq!(config.format_script, directory.join("DiskPart.txt"));
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn invalid_hash_is_rejected() {
        let directory =
            std::env::temp_dir().join(format!("http-imager-config-hash-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Settings.ini");
        fs::write(
            &path,
            "[os]\ndownload_url=http://server/image.wim\nsha256=bad\n",
        )
        .unwrap();
        assert!(DeployConfig::load(&path).is_err());
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }
}
