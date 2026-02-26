use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

pub struct Config {
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self> {
        let proj_dirs =
            ProjectDirs::from("", "", "grub").context("Could not determine home directory")?;

        let data_dir = proj_dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;

        let db_path = data_dir.join("grub.db");

        Ok(Config { db_path, data_dir })
    }

    /// Load the API key from disk, or generate a new one.
    ///
    /// Returns `(key, newly_created)` where `newly_created` is true when a
    /// fresh key was just generated (first run).
    pub fn load_or_create_api_key(&self) -> Result<(String, bool)> {
        use rand::Rng;
        use std::fmt::Write;

        let path = self.data_dir.join("api_key");

        if path.exists() {
            let key = std::fs::read_to_string(&path).context("Failed to read API key file")?;
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Ok((key, false));
            }
        }

        let bytes: [u8; 32] = rand::rng().random();
        let key = bytes
            .iter()
            .fold(String::with_capacity(64), |mut acc: String, b| {
                let _ = write!(acc, "{b:02x}");
                acc
            });
        std::fs::write(&path, &key).context("Failed to write API key file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .context("Failed to set API key file permissions")?;
        }
        eprintln!("Generated new API key: {key}");
        eprintln!("Include in requests: Authorization: Bearer {key}");
        Ok((key, true))
    }
}
