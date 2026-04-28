use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    ZhHans,
    En,
}

impl Language {
    pub fn is_zh(self) -> bool {
        matches!(self, Language::ZhHans)
    }
}

pub fn detect_system_language() -> Language {
    if let Some(lang) = apple_languages() {
        return lang;
    }

    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if value.to_ascii_lowercase().contains("zh") {
                return Language::ZhHans;
            }
        }
    }

    Language::En
}

fn apple_languages() -> Option<Language> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    if text.contains("zh-hans") || text.contains("zh_cn") || text.contains("zh-cn") {
        Some(Language::ZhHans)
    } else {
        Some(Language::En)
    }
}
