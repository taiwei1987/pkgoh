use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::model::Asset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Delete,
    CleanCache,
}

impl OperationKind {
    pub fn label(self) -> &'static str {
        match self {
            OperationKind::Delete => "删除",
            OperationKind::CleanCache => "清缓存",
        }
    }

    pub fn progress_label(self) -> &'static str {
        match self {
            OperationKind::Delete => "正在删除",
            OperationKind::CleanCache => "正在清理缓存",
        }
    }
    pub fn success_label(self) -> &'static str {
        match self {
            OperationKind::Delete => "删除完成",
            OperationKind::CleanCache => "缓存清理完成",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ActionEvent {
    Progress {
        operation: OperationKind,
        label: String,
        completed: usize,
        total: usize,
    },
    AdminPrompt {
        operation: OperationKind,
        retry: bool,
    },
    Finished(ActionReport),
}

#[derive(Debug, Clone)]
pub struct ActionReport {
    pub operation: OperationKind,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub outputs: Vec<ActionOutput>,
}

#[derive(Debug, Clone)]
pub struct ActionOutput {
    pub asset_id: Option<String>,
    pub label: String,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
struct ActionStep {
    asset_id: Option<String>,
    label: String,
    command: ActionCommand,
}

#[derive(Debug, Clone)]
enum ActionCommand {
    Process {
        program: String,
        args: Vec<String>,
        use_sudo: bool,
        brew_internal_sudo: bool,
    },
    RemoveDirs {
        paths: Vec<PathBuf>,
    },
}

pub fn spawn_operation(
    operation: OperationKind,
    assets: Vec<Asset>,
    admin_password: Option<String>,
) -> Receiver<ActionEvent> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let steps = match operation {
            OperationKind::Delete => delete_steps(&assets),
            OperationKind::CleanCache => cache_steps(&assets),
        };

        if steps.iter().any(ActionStep::requires_admin) && admin_password.is_none() && !has_admin_ticket() {
            let _ = tx.send(ActionEvent::AdminPrompt {
                operation,
                retry: false,
            });
            return;
        }

        let total = steps.len();
        let mut outputs = Vec::new();
        let mut succeeded = 0usize;

        for (index, step) in steps.into_iter().enumerate() {
            let _ = tx.send(ActionEvent::Progress {
                operation,
                label: step.label.clone(),
                completed: index,
                total,
            });

            match run_step(&step, admin_password.as_deref()) {
                Ok(detail) => {
                    succeeded += 1;
                    outputs.push(ActionOutput {
                        asset_id: step.asset_id.clone(),
                        label: step.label,
                        success: true,
                        detail,
                    });
                }
                Err(ActionError::AdminAuthFailed) => {
                    let _ = tx.send(ActionEvent::AdminPrompt {
                        operation,
                        retry: true,
                    });
                    return;
                }
                Err(ActionError::Other(error)) => {
                    outputs.push(ActionOutput {
                        asset_id: step.asset_id.clone(),
                        label: step.label,
                        success: false,
                        detail: error,
                    });
                }
            }
        }

        let report = ActionReport {
            operation,
            attempted: total,
            succeeded,
            failed: total.saturating_sub(succeeded),
            outputs,
        };
        let _ = tx.send(ActionEvent::Finished(report));
    });

    rx
}

pub fn has_supported_steps(operation: OperationKind, assets: &[Asset]) -> bool {
    match operation {
        OperationKind::Delete => !delete_steps(assets).is_empty(),
        OperationKind::CleanCache => !cache_steps(assets).is_empty(),
    }
}

pub fn estimate_reclaim_bytes(operation: OperationKind, assets: &[Asset]) -> u64 {
    match operation {
        OperationKind::Delete => assets.iter().map(|asset| asset.size_bytes).sum(),
        OperationKind::CleanCache => estimate_cache_reclaim(assets),
    }
}

fn delete_steps(assets: &[Asset]) -> Vec<ActionStep> {
    let mut steps = Vec::new();

    for asset in assets {
        if let Some(step) = delete_step(asset) {
            steps.push(step);
        }
    }

    steps
}

fn delete_step(asset: &Asset) -> Option<ActionStep> {
    match parse_asset_id(&asset.id) {
        AssetId::BrewFormula(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("Homebrew 删除 {}", asset.name),
            "brew",
            ["uninstall", name],
        )),
        AssetId::BrewCask(token) => Some(process_step_brew_cask(
            Some(asset.id.clone()),
            format!("Homebrew Cask 删除 {}", asset.name),
            ["uninstall", "--cask", token],
        )),
        AssetId::Npm(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("npm 删除 {}", asset.name),
            "npm",
            ["uninstall", "-g", name],
        )),
        AssetId::Pnpm(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("pnpm 删除 {}", asset.name),
            "pnpm",
            ["remove", "-g", name],
        )),
        AssetId::Cargo(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("cargo 删除 {}", asset.name),
            "cargo",
            ["uninstall", "--offline", "--package", name],
        )),
        AssetId::Pip(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("pip 删除 {}", asset.name),
            "python3",
            ["-m", "pip", "uninstall", "-y", name],
        )),
        AssetId::Uv(runtime) => Some(process_step(
            Some(asset.id.clone()),
            format!("uv 删除 Python {}", asset.version),
            "uv",
            ["python", "uninstall", runtime],
        )),
        AssetId::UvTool(name) => Some(process_step(
            Some(asset.id.clone()),
            format!("uv tool 删除 {}", asset.name),
            "uv",
            ["tool", "uninstall", name],
        )),
        AssetId::Mas(app_id) => Some(process_step_sudo(
            Some(asset.id.clone()),
            format!("mas 删除 {}", asset.name),
            "mas",
            ["uninstall", app_id],
        )),
        AssetId::Unknown => None,
    }
}

fn cache_steps(assets: &[Asset]) -> Vec<ActionStep> {
    let mut steps = Vec::new();
    let mut seen_shared = BTreeSet::new();

    for asset in assets {
        match parse_asset_id(&asset.id) {
            AssetId::BrewFormula(name) | AssetId::BrewCask(name) => {
                steps.push(process_step(
                    None,
                    format!("Homebrew 清理 {}", asset.name),
                    "brew",
                    ["cleanup", name],
                ));
            }
            AssetId::Npm(_) => {
                if seen_shared.insert("npm") {
                    steps.push(process_step(
                        None,
                        "npm 清理共享缓存".to_string(),
                        "npm",
                        ["cache", "clean", "--force"],
                    ));
                }
            }
            AssetId::Pnpm(_) => {
                if seen_shared.insert("pnpm") {
                    steps.push(process_step(
                        None,
                        "pnpm 清理共享 store".to_string(),
                        "pnpm",
                        ["store", "prune"],
                    ));
                }
            }
            AssetId::Cargo(_) => {
                if seen_shared.insert("cargo") {
                    let paths = cargo_cache_paths();
                    if !paths.is_empty() {
                        steps.push(ActionStep {
                            asset_id: None,
                            label: "cargo 清理 registry/git 缓存".to_string(),
                            command: ActionCommand::RemoveDirs { paths },
                        });
                    }
                }
            }
            AssetId::Pip(name) => {
                steps.push(process_step(
                    None,
                    format!("pip 清理 {} 缓存", asset.name),
                    "python3",
                    ["-m", "pip", "cache", "remove", name],
                ));
            }
            AssetId::Uv(_) => {
                if seen_shared.insert("uv") {
                    steps.push(process_step(
                        None,
                        "uv 清理共享缓存".to_string(),
                        "uv",
                        ["cache", "clean", "--force"],
                    ));
                }
            }
            AssetId::UvTool(_) => {
                if seen_shared.insert("uv") {
                    steps.push(process_step(
                        None,
                        "uv 清理共享缓存".to_string(),
                        "uv",
                        ["cache", "clean", "--force"],
                    ));
                }
            }
            AssetId::Mas(_) => {}
            AssetId::Unknown => {}
        }
    }

    steps
}

fn estimate_cache_reclaim(assets: &[Asset]) -> u64 {
    let mut total = 0u64;
    let mut seen_shared = BTreeSet::new();

    for asset in assets {
        match parse_asset_id(&asset.id) {
            AssetId::BrewFormula(_) | AssetId::BrewCask(_) => {
                if seen_shared.insert("brew") {
                    total += size_of_homebrew_cache();
                }
            }
            AssetId::Npm(_) => {
                if seen_shared.insert("npm") {
                    total += size_of_npm_cache();
                }
            }
            AssetId::Pnpm(_) => {
                if seen_shared.insert("pnpm") {
                    total += size_of_pnpm_store();
                }
            }
            AssetId::Cargo(_) => {
                if seen_shared.insert("cargo") {
                    total += cargo_cache_paths().iter().map(|path| size_of_path(path)).sum::<u64>();
                }
            }
            AssetId::Pip(_) => {
                if seen_shared.insert("pip") {
                    total += size_of_path(&PathBuf::from(default_pip_cache_dir()));
                }
            }
            AssetId::Uv(_) => {
                if seen_shared.insert("uv") {
                    total += size_of_uv_cache();
                }
            }
            AssetId::UvTool(_) => {
                if seen_shared.insert("uv") {
                    total += size_of_uv_cache();
                }
            }
            AssetId::Mas(_) | AssetId::Unknown => {}
        }
    }

    total
}

fn run_step(step: &ActionStep, admin_password: Option<&str>) -> Result<String, ActionError> {
    match &step.command {
        ActionCommand::Process {
            program,
            args,
            use_sudo,
            brew_internal_sudo,
        } => run_process(program, args, *use_sudo, *brew_internal_sudo, admin_password),
        ActionCommand::RemoveDirs { paths } => {
            remove_dirs(paths).map_err(|error| ActionError::Other(error.to_string()))
        }
    }
}

fn run_process(
    program: &str,
    args: &[String],
    use_sudo: bool,
    brew_internal_sudo: bool,
    admin_password: Option<&str>,
) -> Result<String, ActionError> {
    let output = if use_sudo {
        if let Some(password) = admin_password {
            use std::io::Write;

            let mut child = Command::new("sudo")
                .args(["-S", "-p", "", program])
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|error| ActionError::Other(format!("failed to launch sudo for {program}: {error}")))?;

            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(password.as_bytes());
                let _ = stdin.write_all(b"\n");
            }

            child
                .wait_with_output()
                .map_err(|error| ActionError::Other(error.to_string()))?
        } else {
            Command::new("sudo")
                .args(["-n", program])
                .args(args)
                .output()
                .map_err(|error| ActionError::Other(format!("failed to launch sudo for {program}: {error}")))?
        }
    } else if brew_internal_sudo {
        run_brew_with_askpass(program, args, admin_password)?
    } else {
        Command::new(program)
            .args(args)
            .output()
            .map_err(|error| ActionError::Other(format!("failed to launch {program}: {error}")))?
    };

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            format!("{} {} succeeded", program, args.join(" "))
        };
        Ok(truncate_detail(&detail))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        if (use_sudo || brew_internal_sudo) && looks_like_bad_password(&detail) {
            Err(ActionError::AdminAuthFailed)
        } else {
            Err(ActionError::Other(truncate_detail(&detail)))
        }
    }
}

fn run_brew_with_askpass(
    program: &str,
    args: &[String],
    admin_password: Option<&str>,
) -> Result<std::process::Output, ActionError> {
    let mut command = Command::new(program);
    command.args(args);

    if let Some(password) = admin_password {
        let askpass = AskpassScript::new(password)?;
        command.env("SUDO_ASKPASS", &askpass.script_path);
        command.env("PKGOH_ADMIN_PASSWORD", password);
        command
            .output()
            .map_err(|error| ActionError::Other(format!("failed to launch {program}: {error}")))
    } else {
        command
            .output()
            .map_err(|error| ActionError::Other(format!("failed to launch {program}: {error}")))
    }
}

fn remove_dirs(paths: &[PathBuf]) -> Result<String> {
    let mut removed = Vec::new();

    for path in paths {
        if !path.exists() {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        } else {
            fs::remove_file(path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
        removed.push(path.display().to_string());
    }

    if removed.is_empty() {
        Ok("no cache files found".to_string())
    } else {
        Ok(truncate_detail(&format!("removed {}", removed.join(", "))))
    }
}

fn process_step<const N: usize>(
    asset_id: Option<String>,
    label: String,
    program: &str,
    args: [&str; N],
) -> ActionStep {
    ActionStep {
        asset_id,
        label,
        command: ActionCommand::Process {
            program: program.to_string(),
            args: args.into_iter().map(|value| value.to_string()).collect(),
            use_sudo: false,
            brew_internal_sudo: false,
        },
    }
}

fn process_step_brew_cask<const N: usize>(
    asset_id: Option<String>,
    label: String,
    args: [&str; N],
) -> ActionStep {
    ActionStep {
        asset_id,
        label,
        command: ActionCommand::Process {
            program: "brew".to_string(),
            args: args.into_iter().map(|value| value.to_string()).collect(),
            use_sudo: false,
            brew_internal_sudo: true,
        },
    }
}

fn process_step_sudo<const N: usize>(
    asset_id: Option<String>,
    label: String,
    program: &str,
    args: [&str; N],
) -> ActionStep {
    ActionStep {
        asset_id,
        label,
        command: ActionCommand::Process {
            program: program.to_string(),
            args: args.into_iter().map(|value| value.to_string()).collect(),
            use_sudo: true,
            brew_internal_sudo: false,
        },
    }
}

impl ActionStep {
    fn requires_admin(&self) -> bool {
        matches!(
            self.command,
            ActionCommand::Process {
                use_sudo: true,
                ..
            }
                | ActionCommand::Process {
                    brew_internal_sudo: true,
                    ..
                }
        )
    }
}

#[derive(Debug)]
enum ActionError {
    AdminAuthFailed,
    Other(String),
}

fn has_admin_ticket() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn looks_like_bad_password(detail: &str) -> bool {
    let lowered = detail.to_ascii_lowercase();
    lowered.contains("try again")
        || lowered.contains("incorrect password")
        || lowered.contains("authentication failed")
        || lowered.contains("password is required")
}

struct AskpassScript {
    dir_path: PathBuf,
    script_path: PathBuf,
}

impl AskpassScript {
    fn new(_password: &str) -> Result<Self, ActionError> {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir_path = std::env::temp_dir().join(format!(
            "pkgoh-askpass-{}-{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&dir_path)
            .map_err(|error| ActionError::Other(format!("failed to prepare askpass script: {error}")))?;

        let script_path = dir_path.join("askpass.sh");
        fs::write(
            &script_path,
            "#!/bin/sh\nprintf '%s\\n' \"$PKGOH_ADMIN_PASSWORD\"\n",
        )
        .map_err(|error| ActionError::Other(format!("failed to write askpass script: {error}")))?;

        #[cfg(unix)]
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
            .map_err(|error| ActionError::Other(format!("failed to chmod askpass script: {error}")))?;

        Ok(Self {
            dir_path,
            script_path,
        })
    }
}

impl Drop for AskpassScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.script_path);
        let _ = fs::remove_dir_all(&self.dir_path);
    }
}

fn size_of_homebrew_cache() -> u64 {
    command_path("brew", &["--cache"]).map(|path| size_of_path(&path)).unwrap_or(0)
}

fn size_of_npm_cache() -> u64 {
    command_path("npm", &["config", "get", "cache"]).map(|path| size_of_path(&path)).unwrap_or(0)
}

fn size_of_pnpm_store() -> u64 {
    command_path("pnpm", &["store", "path"]).map(|path| size_of_path(&path)).unwrap_or(0)
}

fn size_of_uv_cache() -> u64 {
    command_path("uv", &["cache", "dir"]).map(|path| size_of_path(&path)).unwrap_or_else(|_| {
        size_of_path(&PathBuf::from(default_uv_cache_dir()))
    })
}

fn cargo_cache_paths() -> Vec<PathBuf> {
    let base = default_home_subdir(&[".cargo"]);
    vec![
        base.join("registry").join("cache"),
        base.join("registry").join("index"),
        base.join("git").join("checkouts"),
        base.join("git").join("db"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn command_path(program: &str, args: &[&str]) -> Result<PathBuf> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to launch {program}"))?;
    if !output.status.success() {
        anyhow::bail!("{} {} failed", program, args.join(" "));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

fn size_of_path(path: &PathBuf) -> u64 {
    if !path.exists() {
        return 0;
    }

    let Some(path_str) = path.to_str() else {
        return 0;
    };

    let output = Command::new("du").args(["-sk", path_str]).output();
    let Ok(output) = output else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

fn truncate_detail(detail: &str) -> String {
    const LIMIT: usize = 240;
    if detail.chars().count() <= LIMIT {
        detail.to_string()
    } else {
        let mut truncated = detail.chars().take(LIMIT).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}

fn default_home_subdir(parts: &[&str]) -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    for part in parts {
        path.push(part);
    }
    path
}

fn default_pip_cache_dir() -> String {
    default_home_subdir(&["Library", "Caches", "pip"]) 
        .display()
        .to_string()
}

fn default_uv_cache_dir() -> String {
    default_home_subdir(&[".cache", "uv"]).display().to_string()
}

enum AssetId<'a> {
    BrewFormula(&'a str),
    BrewCask(&'a str),
    Npm(&'a str),
    Pnpm(&'a str),
    Cargo(&'a str),
    Pip(&'a str),
    Uv(&'a str),
    UvTool(&'a str),
    Mas(&'a str),
    Unknown,
}

fn parse_asset_id(id: &str) -> AssetId<'_> {
    if let Some(rest) = id.strip_prefix("brew-cask:") {
        AssetId::BrewCask(rest)
    } else if let Some(rest) = id.strip_prefix("brew:") {
        AssetId::BrewFormula(rest)
    } else if let Some(rest) = id.strip_prefix("npm:") {
        AssetId::Npm(rest)
    } else if let Some(rest) = id.strip_prefix("pnpm:") {
        AssetId::Pnpm(rest)
    } else if let Some(rest) = id.strip_prefix("cargo:") {
        AssetId::Cargo(rest)
    } else if let Some(rest) = id.strip_prefix("pip:") {
        AssetId::Pip(rest)
    } else if let Some(rest) = id.strip_prefix("uv:") {
        AssetId::Uv(rest)
    } else if let Some(rest) = id.strip_prefix("uv-tool:") {
        AssetId::UvTool(rest)
    } else if let Some(rest) = id.strip_prefix("mas:") {
        AssetId::Mas(rest)
    } else {
        AssetId::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn asset(id: &str, name: &str) -> Asset {
        Asset {
            id: id.to_string(),
            name: name.to_string(),
            source: crate::model::SourceKind::Npm,
            version: "1.0.0".to_string(),
            size_bytes: 123,
            last_used: Local::now(),
            summary: String::new(),
            detail: String::new(),
            removal_advice: crate::model::RemovalAdvice::Removable,
            advice_reason: String::new(),
            cache_cleanable: true,
        }
    }

    #[test]
    fn delete_steps_follow_asset_id_prefixes() {
        let assets = vec![
            asset("brew:ffmpeg", "ffmpeg"),
            asset("brew-cask:claude-code", "claude-code"),
            asset("npm:playwright", "playwright"),
            asset("cargo:rg", "rg"),
        ];

        let steps = delete_steps(&assets);
        assert_eq!(steps.len(), 4);
        assert!(matches!(
            &steps[0].command,
            ActionCommand::Process { program, args, use_sudo, brew_internal_sudo }
                if !use_sudo
                    && !brew_internal_sudo
                    && program == "brew"
                    && args == &vec!["uninstall".to_string(), "ffmpeg".to_string()]
        ));
        assert!(matches!(
            &steps[1].command,
            ActionCommand::Process { program, args, use_sudo, brew_internal_sudo }
                if !use_sudo
                    && *brew_internal_sudo
                    && program == "brew"
                    && args == &vec!["uninstall".to_string(), "--cask".to_string(), "claude-code".to_string()]
        ));
    }

    #[test]
    fn clean_cache_dedupes_shared_sources() {
        let assets = vec![asset("npm:a", "a"), asset("npm:b", "b"), asset("uv:cpython-3.11.15", "python")];
        let steps = cache_steps(&assets);
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn remove_dirs_deletes_existing_paths() {
        let path = std::env::temp_dir().join(format!("pkgoh-test-{}", std::process::id()));
        let nested = path.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("file.txt"), "hello").unwrap();

        let result = remove_dirs(&[path.clone()]).unwrap();
        assert!(result.contains("removed"));
        assert!(!path.exists());
    }
}
