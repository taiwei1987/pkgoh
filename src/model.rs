use chrono::{DateTime, Duration, Local};

use crate::config::HighlightConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    Brew,
    Npm,
    Pnpm,
    Cargo,
    Pip,
    Uv,
    Mas,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Brew => "Homebrew",
            SourceKind::Npm => "npm",
            SourceKind::Pnpm => "pnpm",
            SourceKind::Cargo => "cargo",
            SourceKind::Pip => "pip",
            SourceKind::Uv => "uv",
            SourceKind::Mas => "mas",
        }
    }

    pub fn all() -> &'static [SourceKind] {
        &[
            SourceKind::Brew,
            SourceKind::Npm,
            SourceKind::Pnpm,
            SourceKind::Cargo,
            SourceKind::Pip,
            SourceKind::Uv,
            SourceKind::Mas,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub source: SourceKind,
    pub version: String,
    pub size_bytes: u64,
    pub last_used: DateTime<Local>,
    pub summary: String,
    pub detail: String,
    pub removal_advice: RemovalAdvice,
    pub advice_reason: String,
    pub cache_cleanable: bool,
}

impl Asset {
    pub fn size_label(&self) -> String {
        human_size(self.size_bytes)
    }

    pub fn last_used_label(&self) -> String {
        self.last_used.format("%Y-%m-%d").to_string()
    }

    pub fn is_large(&self, highlight: &HighlightConfig) -> bool {
        self.size_bytes >= highlight.large_size_mb * 1024 * 1024
    }

    pub fn is_stale(&self, highlight: &HighlightConfig) -> bool {
        self.last_used < Local::now() - Duration::days(highlight.unused_days)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemovalAdvice {
    Removable,
    Keep,
    CoreDependency,
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut size = bytes as f64;
    let mut index = 0usize;
    while size >= 1024.0 && index < UNITS.len() - 1 {
        size /= 1024.0;
        index += 1;
    }

    if index == 0 {
        format!("{}{}", size as u64, UNITS[index])
    } else {
        format!("{size:.1}{}", UNITS[index])
    }
}
