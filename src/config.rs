use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sources: SourceToggles,
    #[serde(default)]
    pub highlight: HighlightConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceToggles {
    #[serde(default = "enabled")]
    pub brew: bool,
    #[serde(default = "enabled")]
    pub npm: bool,
    #[serde(default = "enabled")]
    pub pnpm: bool,
    #[serde(default = "enabled")]
    pub cargo: bool,
    #[serde(default = "enabled")]
    pub pip: bool,
    #[serde(default = "enabled")]
    pub uv: bool,
    #[serde(default = "enabled")]
    pub mas: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightConfig {
    #[serde(default = "default_large_size_mb")]
    pub large_size_mb: u64,
    #[serde(default = "default_unused_days")]
    pub unused_days: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sources: SourceToggles::default(),
            highlight: HighlightConfig::default(),
        }
    }
}

impl Default for SourceToggles {
    fn default() -> Self {
        Self {
            brew: true,
            npm: true,
            pnpm: true,
            cargo: true,
            pip: true,
            uv: true,
            mas: true,
        }
    }
}

impl Default for HighlightConfig {
    fn default() -> Self {
        Self {
            large_size_mb: default_large_size_mb(),
            unused_days: default_unused_days(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let Some(path) = Self::config_path() else {
            return Ok(Self::default());
        };

        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))
    }

    fn config_path() -> Option<PathBuf> {
        if let Ok(value) = env::var("PKGOH_CONFIG") {
            return Some(PathBuf::from(value));
        }

        dirs::config_dir().map(|dir| dir.join("pkgoh").join("pkgoh.toml"))
    }
}

const fn enabled() -> bool {
    true
}

const fn default_large_size_mb() -> u64 {
    500
}

const fn default_unused_days() -> i64 {
    90
}
