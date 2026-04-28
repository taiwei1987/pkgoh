use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc::{self, Receiver}, Mutex, OnceLock},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;

use crate::{
    config::Config,
    i18n::Language,
    model::{Asset, RemovalAdvice, SourceKind},
};

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Progress {
        current: SourceKind,
        completed: usize,
        total: usize,
        found: usize,
    },
    Finished(Vec<Asset>),
}

trait SourcePlugin: Send {
    fn kind(&self) -> SourceKind;
    fn scan(&self) -> Result<Vec<Asset>>;
}

pub fn spawn_scanner(config: Config, language: Language) -> Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let plugins = enabled_plugins(&config, language);
        let total = plugins.len();
        let mut assets = Vec::new();

        for (index, plugin) in plugins.into_iter().enumerate() {
            let kind = plugin.kind();
            let source_assets = plugin.scan().unwrap_or_default();
            let found = assets.len() + source_assets.len();
            assets.extend(source_assets);

            let _ = tx.send(ScanEvent::Progress {
                current: kind,
                completed: index + 1,
                total,
                found,
            });

        }

        assets.sort_by(|left, right| right.size_bytes.cmp(&left.size_bytes));
        let _ = tx.send(ScanEvent::Finished(assets));
    });

    rx
}

fn enabled_plugins(config: &Config, language: Language) -> Vec<Box<dyn SourcePlugin>> {
    let mut plugins: Vec<Box<dyn SourcePlugin>> = Vec::new();

    if config.sources.brew {
        plugins.push(Box::new(BrewPlugin { language }));
    }
    if config.sources.npm {
        plugins.push(Box::new(NpmPlugin { language }));
    }
    if config.sources.pnpm {
        plugins.push(Box::new(PnpmPlugin { language }));
    }
    if config.sources.cargo {
        plugins.push(Box::new(CargoPlugin { language }));
    }
    if config.sources.pip {
        plugins.push(Box::new(PipPlugin { language }));
    }
    if config.sources.uv {
        plugins.push(Box::new(UvPlugin { language }));
    }
    if config.sources.mas {
        plugins.push(Box::new(MasPlugin { language }));
    }

    if plugins.is_empty() {
        SourceKind::all()
            .iter()
            .copied()
            .map(|kind| plugin_for_kind(kind, language))
            .collect()
    } else {
        plugins
    }
}

fn plugin_for_kind(kind: SourceKind, language: Language) -> Box<dyn SourcePlugin> {
    match kind {
        SourceKind::Brew => Box::new(BrewPlugin { language }),
        SourceKind::Npm => Box::new(NpmPlugin { language }),
        SourceKind::Pnpm => Box::new(PnpmPlugin { language }),
        SourceKind::Cargo => Box::new(CargoPlugin { language }),
        SourceKind::Pip => Box::new(PipPlugin { language }),
        SourceKind::Uv => Box::new(UvPlugin { language }),
        SourceKind::Mas => Box::new(MasPlugin { language }),
    }
}

struct BrewPlugin {
    language: Language,
}

impl SourcePlugin for BrewPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Brew
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("brew") {
            return Ok(Vec::new());
        }

        let info: BrewInfo = command_json("brew", &["info", "--json=v2", "--installed"])?;
        let prefix = PathBuf::from(command_text("brew", &["--prefix"])?);
        let cellar_root = PathBuf::from(command_text("brew", &["--cellar"])?);
        let cache_root = PathBuf::from(command_text("brew", &["--cache"])?);
        let reverse_dependents = brew_reverse_dependents(&info.formulae);
        let mut assets = Vec::new();

        for formula in info.formulae {
            let installed = formula.installed.first();
            let version = installed
                .map(|item| item.version.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let install_path = resolve_brew_formula_path(&cellar_root, &formula.name, &version);
            let linked_path = prefix.join("opt").join(&formula.name);
            let probe_paths = vec![install_path.clone(), linked_path.clone()];
            let (removal_advice, advice_reason) =
                brew_formula_advice(&formula, &reverse_dependents, self.language);

            assets.push(Asset {
                id: format!("brew:{}", formula.name),
                name: formula.name.clone(),
                source: SourceKind::Brew,
                version,
                size_bytes: size_of_first_existing_path(&probe_paths),
                last_used: best_effort_last_used(
                    &probe_paths,
                    installed.and_then(|item| item.time).map(epoch_local),
                ),
                summary: formula
                    .desc
                    .clone()
                    .unwrap_or_else(|| generic_formula_summary(&formula.name, self.language)),
                detail: format!(
                    "Cellar: {}\nLinked prefix: {}\nCache: {}",
                    install_path.display(),
                    linked_path.display(),
                    cache_root.display(),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        for cask in info.casks {
            let version = cask
                .installed
                .clone()
                .or(cask.version.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let install_path = prefix.join("Caskroom").join(&cask.token);
            let (removal_advice, advice_reason) = brew_cask_advice(&cask.token, self.language);
            assets.push(Asset {
                id: format!("brew-cask:{}", cask.token),
                name: cask.token.clone(),
                source: SourceKind::Brew,
                version,
                size_bytes: size_of_first_existing_path(std::slice::from_ref(&install_path)),
                last_used: best_effort_last_used(
                    std::slice::from_ref(&install_path),
                    cask.installed_time.map(epoch_local),
                ),
                summary: cask
                    .desc
                    .clone()
                    .unwrap_or_else(|| generic_cask_summary(&cask.token, self.language)),
                detail: format!(
                    "Caskroom: {}\nApplications: /Applications\nCache: {}",
                    install_path.display(),
                    cache_root.display(),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct NpmPlugin {
    language: Language,
}

impl SourcePlugin for NpmPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Npm
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("npm") {
            return Ok(Vec::new());
        }

        let root = PathBuf::from(command_text("npm", &["root", "-g"])?);
        let mut roots = vec![root.clone()];
        roots.extend(discover_path_linked_node_global_roots());
        roots.retain(|path| path.exists());
        roots.sort();
        roots.dedup();

        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let cache_path = command_text("npm", &["config", "get", "cache"])
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_home_subdir(&[".npm"]));
        let listing: NpmLsOutput =
            command_json("npm", &["ls", "-g", "--depth=0", "--json"]).unwrap_or(NpmLsOutput {
                dependencies: BTreeMap::new(),
            });
        let mut packages = Vec::new();
        let mut seen_names = BTreeSet::new();

        for (name, dep) in listing.dependencies {
            let install_path = root.join(&name);
            if !install_path.exists() {
                continue;
            }
            let manifest = read_manifest(&install_path).ok();
            let package_summary = manifest
                .as_ref()
                .and_then(|manifest| manifest.description.clone());
            let manifest_refs = manifest
                .as_ref()
                .map(manifest_references)
                .unwrap_or_default();
            packages.push(NodePackageScan {
                name: name.clone(),
                version: dep.version.unwrap_or_else(|| "unknown".to_string()),
                install_path,
                summary: package_summary.or(dep.description.clone()),
                references: manifest_refs,
            });
            seen_names.insert(name);
        }

        for extra_root in roots.iter().filter(|path| **path != root) {
            for install_path in collect_package_dirs(extra_root)? {
                let manifest = read_manifest(&install_path)?;
                let Some(name) = manifest.name.clone() else {
                    continue;
                };
                if !seen_names.insert(name.clone()) {
                    continue;
                }
                let references = manifest_references(&manifest);
                packages.push(NodePackageScan {
                    name,
                    version: manifest.version.unwrap_or_else(|| "unknown".to_string()),
                    summary: manifest.description.clone(),
                    install_path,
                    references,
                });
            }
        }

        let top_level_refs = build_top_level_reference_map(&packages);
        let mut assets = Vec::new();

        for package in packages {
            let (removal_advice, advice_reason) = node_package_advice(
                &package.name,
                top_level_refs.get(&package.name),
                self.language,
            );
            assets.push(Asset {
                id: format!("npm:{}", package.name),
                name: package.name.clone(),
                source: SourceKind::Npm,
                version: package.version,
                size_bytes: size_of_first_existing_path(std::slice::from_ref(&package.install_path)),
                last_used: best_effort_last_used(std::slice::from_ref(&package.install_path), None),
                summary: package
                    .summary
                    .unwrap_or_else(|| generic_npm_summary(&package.name, self.language)),
                detail: format!(
                    "Global root: {}\nPackage path: {}\nCache: {}",
                    package_root_from_install_path(&package.install_path)
                        .unwrap_or(&root)
                        .display(),
                    package.install_path.display(),
                    cache_path.display(),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct PnpmPlugin {
    language: Language,
}

impl SourcePlugin for PnpmPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Pnpm
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("pnpm") {
            return Ok(Vec::new());
        }

        let root = PathBuf::from(command_text("pnpm", &["root", "-g"])?);
        if !root.exists() {
            return Ok(Vec::new());
        }

        let store_path = command_text("pnpm", &["store", "path"])
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join(".pnpm-store"));
        let mut packages = Vec::new();
        for install_path in collect_package_dirs(&root)? {
            let manifest = read_manifest(&install_path)?;
            let name = manifest.name.clone().unwrap_or_else(|| {
                install_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });
            let manifest_refs = manifest_references(&manifest);
            packages.push(NodePackageScan {
                name: name.clone(),
                version: manifest.version.unwrap_or_else(|| "unknown".to_string()),
                summary: manifest
                    .description
                    .clone(),
                install_path,
                references: manifest_refs,
            });
        }

        let top_level_refs = build_top_level_reference_map(&packages);
        let mut assets = Vec::new();
        for package in packages {
            let (removal_advice, advice_reason) = node_package_advice(
                &package.name,
                top_level_refs.get(&package.name),
                self.language,
            );
            assets.push(Asset {
                id: format!("pnpm:{}", package.name),
                name: package.name.clone(),
                source: SourceKind::Pnpm,
                version: package.version,
                size_bytes: size_of_first_existing_path(std::slice::from_ref(&package.install_path)),
                last_used: best_effort_last_used(std::slice::from_ref(&package.install_path), None),
                summary: package
                    .summary
                    .unwrap_or_else(|| generic_npm_summary(&package.name, self.language)),
                detail: format!(
                    "Global root: {}\nPackage path: {}\nStore: {}",
                    root.display(),
                    package.install_path.display(),
                    store_path.display(),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct CargoPlugin {
    language: Language,
}

impl SourcePlugin for CargoPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Cargo
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("cargo") {
            return Ok(Vec::new());
        }

        let raw = command_text("cargo", &["install", "--list"])?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        let bin_root = default_home_subdir(&[".cargo", "bin"]);
        let mut assets = Vec::new();

        for item in parse_cargo_install_list(&raw) {
            let binary_paths: Vec<PathBuf> = item.binaries.iter().map(|bin| bin_root.join(bin)).collect();
            let (removal_advice, advice_reason) = cargo_package_advice(&item.name, self.language);
            assets.push(Asset {
                id: format!("cargo:{}", item.name),
                name: item.name.clone(),
                source: SourceKind::Cargo,
                version: item.version,
                size_bytes: total_size_of_paths(&binary_paths),
                last_used: best_effort_last_used(&binary_paths, None),
                summary: generic_cargo_summary(&item.name, self.language),
                detail: format!(
                    "Cargo bin dir: {}\nBinaries: {}",
                    bin_root.display(),
                    item.binaries.join(", "),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct PipPlugin {
    language: Language,
}

impl SourcePlugin for PipPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Pip
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("python3") {
            return Ok(Vec::new());
        }

        let result: PipInventory = python_json(PIP_SCAN_SCRIPT)?;
        let mut assets = Vec::new();

        for package in result.packages {
            let location = PathBuf::from(&package.location);
            let fallback = package.last_used_epoch.map(epoch_local);
            let package_name = package.name.clone();
            let (removal_advice, advice_reason) = pip_package_advice(&package, self.language);
            assets.push(Asset {
                id: format!("pip:{}", package_name),
                name: package_name.clone(),
                source: SourceKind::Pip,
                version: package.version,
                size_bytes: package.size_bytes,
                last_used: best_effort_last_used(std::slice::from_ref(&location), fallback),
                summary: package
                    .summary
                    .clone()
                    .unwrap_or_else(|| generic_pip_summary(&package_name, self.language)),
                detail: format!(
                    "Location: {}\nScripts dir: {}\nUser site: {}\nCache: {}",
                    location.display(),
                    result.scripts_dir,
                    result.user_site,
                    result.cache_dir,
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct UvPlugin {
    language: Language,
}

impl SourcePlugin for UvPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Uv
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("uv") {
            return Ok(Vec::new());
        }

        let uv_root = default_home_subdir(&[".local", "share", "uv", "python"]);
        let alias_root = default_home_subdir(&[".local", "bin"]);
        let mut assets = Vec::new();

        if uv_root.exists() {
            let managed = collect_uv_runtimes(&uv_root)?;
            for runtime in managed {
                let aliases = uv_aliases(&alias_root, &runtime.path);
                let alias_text = if aliases.is_empty() {
                    "none".to_string()
                } else {
                    aliases.join(", ")
                };
                let runtime_version = runtime.version.clone();
                let (removal_advice, advice_reason) =
                    uv_runtime_advice(&runtime_version, &aliases, self.language);
                assets.push(Asset {
                    id: format!("uv:{}", runtime.key),
                    name: format!("python@{}", runtime_version),
                    source: SourceKind::Uv,
                    version: runtime_version.clone(),
                    size_bytes: size_of_first_existing_path(std::slice::from_ref(&runtime.path)),
                    last_used: best_effort_last_used(std::slice::from_ref(&runtime.path), None),
                    summary: generic_uv_summary(&runtime_version, self.language),
                    detail: format!(
                        "Runtime root: {}\nManaged by: uv python\nAliases: {}",
                        runtime.path.display(),
                        alias_text,
                    ),
                    removal_advice,
                    advice_reason,
                    cache_cleanable: true,
                });
            }
        }

        for tool in collect_uv_tools(self.language)? {
            let (removal_advice, advice_reason) = uv_tool_advice(&tool.name, self.language);
            assets.push(Asset {
                id: format!("uv-tool:{}", tool.name),
                name: tool.name.clone(),
                source: SourceKind::Uv,
                version: tool.version.clone(),
                size_bytes: size_of_first_existing_path(std::slice::from_ref(&tool.path)),
                last_used: best_effort_last_used(std::slice::from_ref(&tool.path), None),
                summary: tool.summary.clone(),
                detail: format!(
                    "Tool env: {}\nManaged by: uv tool\nExecutables: {}",
                    tool.path.display(),
                    if tool.executables.is_empty() {
                        "unknown".to_string()
                    } else {
                        tool.executables.join(", ")
                    }
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: true,
            });
        }

        Ok(assets)
    }
}

struct MasPlugin {
    language: Language,
}

impl SourcePlugin for MasPlugin {
    fn kind(&self) -> SourceKind {
        SourceKind::Mas
    }

    fn scan(&self) -> Result<Vec<Asset>> {
        if !command_exists("mas") {
            return Ok(Vec::new());
        }

        let raw = command_text("mas", &["list"])?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut assets = Vec::new();
        for item in parse_mas_list(&raw) {
            let install_path = PathBuf::from("/Applications").join(format!("{}.app", item.name));
            let (removal_advice, advice_reason) = mas_app_advice(&item.name, self.language);
            assets.push(Asset {
                id: format!("mas:{}", item.id),
                name: item.name.clone(),
                source: SourceKind::Mas,
                version: item.version,
                size_bytes: size_of_first_existing_path(std::slice::from_ref(&install_path)),
                last_used: best_effort_last_used(std::slice::from_ref(&install_path), None),
                summary: generic_mas_summary(&item.name, self.language),
                detail: format!(
                    "App Store id: {}\nApplications path: {}",
                    item.id,
                    install_path.display(),
                ),
                removal_advice,
                advice_reason,
                cache_cleanable: false,
            });
        }

        Ok(assets)
    }
}

fn command_exists(program: &str) -> bool {
    Command::new("sh")
        .args(["-lc", &format!("command -v {program} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn command_text(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to launch {program}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "{} {} failed: {}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_json<T>(program: &str, args: &[&str]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = command_text(program, args)?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse JSON from {program}"))
}

fn python_json<T>(script: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let output = Command::new("python3")
        .args(["-c", script])
        .output()
        .context("failed to launch python3")?;

    if !output.status.success() {
        anyhow::bail!(
            "python3 script failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&raw).context("failed to parse JSON from python3")
}

fn resolve_brew_formula_path(cellar_root: &Path, name: &str, version: &str) -> PathBuf {
    let versioned = cellar_root.join(name).join(version);
    if versioned.exists() {
        versioned
    } else {
        cellar_root.join(name)
    }
}

fn size_of_first_existing_path(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .find(|path| path.exists())
        .map(|path| size_of_path(path))
        .unwrap_or(0)
}

fn total_size_of_paths(paths: &[PathBuf]) -> u64 {
    paths.iter().map(|path| size_of_path(path)).sum()
}

fn size_of_path(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }

    let Some(path_str) = path.to_str() else {
        return 0;
    };

    let modified_epoch = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_epoch_seconds)
        .unwrap_or(0);

    let cache = size_cache();
    if let Ok(cache) = cache.lock() {
        if let Some((cached_mtime, cached_size)) = cache.get(path_str) {
            if *cached_mtime == modified_epoch {
                return *cached_size;
            }
        }
    }

    let output = Command::new("du").args(["-sk", path_str]).output();
    let Ok(output) = output else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let kib = stdout
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let size = kib * 1024;

    if let Ok(mut cache) = size_cache().lock() {
        cache.insert(path_str.to_string(), (modified_epoch, size));
    }

    size
}

fn best_effort_last_used(paths: &[PathBuf], fallback: Option<DateTime<Local>>) -> DateTime<Local> {
    let mut newest = None;

    for path in paths {
        if let Some(candidate) = path_timestamp(path) {
            newest = match newest {
                Some(current) if current >= candidate => Some(current),
                _ => Some(candidate),
            };
        }
    }

    newest.or(fallback).unwrap_or_else(Local::now)
}

fn path_timestamp(path: &Path) -> Option<DateTime<Local>> {
    let metadata = fs::metadata(path).ok()?;
    let system_time = metadata.accessed().or_else(|_| metadata.modified()).ok()?;
    Some(system_time_to_local(system_time))
}

fn system_time_to_local(time: SystemTime) -> DateTime<Local> {
    DateTime::<Local>::from(time)
}

fn system_time_epoch_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|duration| duration.as_secs())
}

fn epoch_local(seconds: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .unwrap_or_else(|| DateTime::<Local>::from(UNIX_EPOCH))
}

fn default_home_subdir(parts: &[&str]) -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    for part in parts {
        path.push(part);
    }
    path
}

fn size_cache() -> &'static Mutex<BTreeMap<String, (u64, u64)>> {
    static SIZE_CACHE: OnceLock<Mutex<BTreeMap<String, (u64, u64)>>> = OnceLock::new();
    SIZE_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn collect_package_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut packages = Vec::new();

    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if !path.is_dir() {
            continue;
        }

        if file_name.starts_with('@') {
            for scoped in fs::read_dir(&path)
                .with_context(|| format!("failed to read {}", path.display()))?
            {
                let scoped = scoped?;
                if scoped.path().join("package.json").exists() {
                    packages.push(scoped.path());
                }
            }
        } else if path.join("package.json").exists() {
            packages.push(path);
        }
    }

    packages.sort();
    Ok(packages)
}

fn discover_path_linked_node_global_roots() -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();

    let Some(path_value) = std::env::var_os("PATH") else {
        return Vec::new();
    };

    for bin_dir in std::env::split_paths(&path_value) {
        let Ok(entries) = fs::read_dir(&bin_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            let Ok(target) = fs::canonicalize(&entry_path) else {
                continue;
            };
            if let Some(root) = node_modules_root_for_path(&target) {
                roots.insert(root);
            }
        }
    }

    roots.into_iter().collect()
}

fn node_modules_root_for_path(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().is_some_and(|name| name == "node_modules") {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn package_root_from_install_path(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "node_modules"))
}

fn read_manifest(path: &Path) -> Result<PackageManifest> {
    let raw = fs::read_to_string(path.join("package.json"))
        .with_context(|| format!("failed to read package.json in {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse package.json in {}", path.display()))
}

fn parse_cargo_install_list(raw: &str) -> Vec<CargoInstallEntry> {
    let mut entries = Vec::new();
    let mut current: Option<CargoInstallEntry> = None;

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if !line.starts_with(' ') && !line.starts_with('\t') {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }

            let header = line.trim().trim_end_matches(':');
            let (name, version) = header
                .rsplit_once(" v")
                .map(|(name, version)| (name.to_string(), version.to_string()))
                .unwrap_or_else(|| (header.to_string(), "unknown".to_string()));
            current = Some(CargoInstallEntry {
                name,
                version,
                binaries: Vec::new(),
            });
            continue;
        }

        if let Some(entry) = current.as_mut() {
            for binary in line.trim().split(',') {
                let binary = binary.trim();
                if !binary.is_empty() {
                    entry.binaries.push(binary.to_string());
                }
            }
        }
    }

    if let Some(entry) = current {
        entries.push(entry);
    }

    entries
}

fn collect_uv_runtimes(root: &Path) -> Result<Vec<UvRuntime>> {
    let mut runtimes = Vec::new();
    let mut seen = BTreeSet::new();

    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }

        let key = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        if !key.starts_with("cpython-") {
            continue;
        }

        let version = key
            .split('-')
            .nth(1)
            .unwrap_or("unknown")
            .to_string();
        runtimes.push(UvRuntime {
            key,
            version,
            path: canonical,
        });
    }

    runtimes.sort_by(|left, right| left.version.cmp(&right.version));
    Ok(runtimes)
}

fn uv_aliases(bin_root: &Path, runtime_root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(bin_root) else {
        return Vec::new();
    };

    let mut aliases = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(target) = fs::read_link(&path) else {
            continue;
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            path.parent().unwrap_or(bin_root).join(target)
        };
        if resolved.starts_with(runtime_root) {
            if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                aliases.insert(name.to_string());
            }
        }
    }

    aliases.into_iter().collect()
}

fn collect_uv_tools(language: Language) -> Result<Vec<UvTool>> {
    let tools_root = default_home_subdir(&[".local", "share", "uv", "tools"]);
    if !tools_root.exists() {
        return Ok(Vec::new());
    }

    let mut tools = Vec::new();
    for entry in fs::read_dir(&tools_root)
        .with_context(|| format!("failed to read {}", tools_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let directory_name = entry.file_name().to_string_lossy().to_string();
        let receipt = read_uv_tool_receipt(&path).unwrap_or_default();
        let name = receipt.name.unwrap_or(directory_name);
        let version = receipt.version.unwrap_or_else(|| "unknown".to_string());
        let executables = collect_uv_tool_bins(&path);
        tools.push(UvTool {
            summary: receipt
                .summary
                .unwrap_or_else(|| generic_uv_tool_summary(&name, language)),
            name,
            version,
            path,
            executables,
        });
    }

    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tools)
}

fn read_uv_tool_receipt(path: &Path) -> Result<UvToolReceipt> {
    let receipt_path = path.join("uv-receipt.toml");
    if !receipt_path.exists() {
        return Ok(UvToolReceipt::default());
    }

    let raw = fs::read_to_string(&receipt_path)
        .with_context(|| format!("failed to read {}", receipt_path.display()))?;
    let value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", receipt_path.display()))?;
    let first_requirement = value
        .get("tool")
        .and_then(|tool| tool.get("requirements"))
        .and_then(toml::Value::as_array)
        .and_then(|requirements| requirements.first());

    Ok(UvToolReceipt {
        name: first_requirement
            .and_then(|requirement| requirement.get("name"))
            .and_then(toml::Value::as_str)
            .map(|value| value.to_string()),
        version: first_requirement
            .and_then(|requirement| requirement.get("specifier"))
            .and_then(toml::Value::as_str)
            .map(|value| value.trim_start_matches("==").to_string()),
        summary: first_requirement
            .and_then(|requirement| requirement.get("name"))
            .and_then(toml::Value::as_str)
            .map(|tool_name| format!("{tool_name} is managed by uv tool.")),
    })
}

fn collect_uv_tool_bins(path: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(path.join("bin")) else {
        return Vec::new();
    };

    let mut bins = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
        .filter(|name| !matches!(name.as_str(), "python" | "python3" | "activate" | "activate.csh" | "activate.fish"))
        .collect::<Vec<_>>();
    bins.sort();
    bins
}

fn parse_mas_list(raw: &str) -> Vec<MasInstallEntry> {
    let mut entries = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((id, rest)) = trimmed.split_once(' ') else {
            continue;
        };
        let Some((name, version)) = rest.rsplit_once(" (") else {
            continue;
        };
        let version = version.trim_end_matches(')').to_string();
        entries.push(MasInstallEntry {
            id: id.to_string(),
            name: name.to_string(),
            version,
        });
    }

    entries
}

fn brew_reverse_dependents(formulae: &[BrewFormula]) -> BTreeMap<String, BTreeSet<String>> {
    let mut reverse = BTreeMap::new();

    for formula in formulae {
        let deps = formula
            .installed
            .first()
            .map(|installed| {
                installed
                    .runtime_dependencies
                    .iter()
                    .map(|dep| dep.full_name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| formula.dependencies.clone());

        for dep in deps {
            reverse
                .entry(dep)
                .or_insert_with(BTreeSet::new)
                .insert(formula.name.clone());
        }
    }

    reverse
}

fn brew_formula_advice(
    formula: &BrewFormula,
    reverse_dependents: &BTreeMap<String, BTreeSet<String>>,
    language: Language,
) -> (RemovalAdvice, String) {
    let dependents = reverse_dependents
        .get(&formula.name)
        .map(|names| names.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let runtime_dependency_count = formula
        .installed
        .first()
        .map(|installed| installed.runtime_dependencies.len())
        .unwrap_or_else(|| formula.dependencies.len());
    let installed_as_dependency = formula
        .installed
        .first()
        .map(|installed| installed.installed_as_dependency)
        .unwrap_or(false);
    let installed_on_request = formula
        .installed
        .first()
        .map(|installed| installed.installed_on_request)
        .unwrap_or(false);

    if !dependents.is_empty() {
        let examples = join_names_limited(&dependents, 3);
        return (
            RemovalAdvice::CoreDependency,
            if language.is_zh() {
                format!(
                    "当前有 {} 个已安装 Homebrew 工具依赖它，例如 {}。",
                    dependents.len(),
                    examples
                )
            } else {
                format!(
                    "{} installed Homebrew package(s) depend on it, including {}.",
                    dependents.len(),
                    examples
                )
            },
        );
    }

    if is_brew_foundational(&formula.name) {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                "它属于常见运行时、编译链或基础库，删除后通常需要额外修复环境。".to_string()
            } else {
                "It is part of a common runtime, toolchain, or base library, so removal often needs follow-up fixes.".to_string()
            },
        );
    }

    if installed_as_dependency && !installed_on_request {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                "它最初是作为其他 Homebrew 工具的依赖装进来的，虽然当前未检测到强依赖，但通常不建议随意手动删除。".to_string()
            } else {
                "It was originally installed as a dependency of another Homebrew tool; even without a strong current dependency signal, it is safer to keep.".to_string()
            },
        );
    }

    if runtime_dependency_count > 0 {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                format!(
                    "它本身依赖 {} 个 Homebrew 运行库，删除后一般只影响当前工具，但可能需要重新安装。",
                    runtime_dependency_count
                )
            } else {
                format!(
                    "It relies on {runtime_dependency_count} Homebrew runtime package(s); removal usually affects only this tool but may require reinstalling later."
                )
            },
        );
    }

    (
        RemovalAdvice::Removable,
        if language.is_zh() {
            "没有发现其他已安装 Homebrew 工具依赖它，删除后通常只会让这个工具本身不可用。".to_string()
        } else {
            "No other installed Homebrew tools were found depending on it, so removal usually only disables this tool itself.".to_string()
        },
    )
}

fn brew_cask_advice(name: &str, language: Language) -> (RemovalAdvice, String) {
    let advice = if is_cask_foundational(name) {
        RemovalAdvice::Keep
    } else {
        RemovalAdvice::Removable
    };

    let reason = match (advice, language.is_zh()) {
        (RemovalAdvice::Keep, true) => {
            "它更像常驻开发环境中的桌面工具，删除后不会牵连依赖，但可能影响日常工作流。".to_string()
        }
        (RemovalAdvice::Keep, false) => {
            "It behaves like a regularly used desktop tool in the development environment; removal should not break dependencies, but may disrupt your workflow.".to_string()
        }
        (_, true) => "未发现 Homebrew 级别的依赖关系，删除后通常只会让这个应用本身不可用。".to_string(),
        (_, false) => {
            "No Homebrew-level dependency relationship was found, so removal usually only makes this app unavailable.".to_string()
        }
    };

    (advice, reason)
}

fn manifest_references(manifest: &PackageManifest) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    refs.extend(manifest.dependencies.keys().cloned());
    refs.extend(manifest.optional_dependencies.keys().cloned());
    refs.extend(manifest.peer_dependencies.keys().cloned());
    refs
}

fn build_top_level_reference_map(packages: &[NodePackageScan]) -> BTreeMap<String, BTreeSet<String>> {
    let names: BTreeSet<&str> = packages.iter().map(|package| package.name.as_str()).collect();
    let mut refs = BTreeMap::new();

    for package in packages {
        for dependency in &package.references {
            if names.contains(dependency.as_str()) {
                refs.entry(dependency.clone())
                    .or_insert_with(BTreeSet::new)
                    .insert(package.name.clone());
            }
        }
    }

    refs
}

fn node_package_advice(
    name: &str,
    top_level_dependents: Option<&BTreeSet<String>>,
    language: Language,
) -> (RemovalAdvice, String) {
    let dependents = top_level_dependents
        .map(|items| items.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    if dependents.len() >= 2 {
        let examples = join_names_limited(&dependents, 3);
        return (
            RemovalAdvice::CoreDependency,
            if language.is_zh() {
                format!(
                    "另外 {} 个全局 Node 工具的清单里引用了它，例如 {}，卸载后可能引发连锁报错。",
                    dependents.len(),
                    examples
                )
            } else {
                format!(
                    "{} other global Node tools reference it in their manifests, including {}, so removal may trigger follow-up errors.",
                    dependents.len(),
                    examples
                )
            },
        );
    }

    if dependents.len() == 1 {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                format!(
                    "另一个全局 Node 工具 {} 的清单里引用了它，删除后可能出现可修复的命令报错。",
                    dependents[0]
                )
            } else {
                format!(
                    "Another global Node tool ({}) references it in its manifest, so removal may cause fixable command errors.",
                    dependents[0]
                )
            },
        );
    }

    if is_node_foundational(name) {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                "它属于 Node.js 常见包管理或脚手架工具链的一部分，删除后通常需要手动恢复命令环境。".to_string()
            } else {
                "It is part of the common Node.js package-management or scaffolding toolchain, so removal often needs manual environment fixes.".to_string()
            },
        );
    }

    (
        RemovalAdvice::Removable,
        if language.is_zh() {
            "它看起来是独立的全局 Node 工具，没有发现其他已安装全局工具直接引用它。".to_string()
        } else {
            "It appears to be a standalone global Node tool, with no direct references from other installed global tools.".to_string()
        },
    )
}

fn cargo_package_advice(name: &str, language: Language) -> (RemovalAdvice, String) {
    if is_cargo_foundational(name) {
        (
            RemovalAdvice::Keep,
            if language.is_zh() {
                "它属于 Rust 日常开发流程里常见的辅助工具，删除后一般不会波及依赖，但工作流可能中断。".to_string()
            } else {
                "It is a common helper in everyday Rust workflows; removal usually will not break dependencies, but may interrupt your tooling flow.".to_string()
            },
        )
    } else {
        (
            RemovalAdvice::Removable,
            if language.is_zh() {
                "cargo install 安装的大多是独立命令，删除后通常只会影响这个命令本身。".to_string()
            } else {
                "Most `cargo install` packages are standalone commands, so removal usually only affects that command itself.".to_string()
            },
        )
    }
}

fn pip_package_advice(package: &PipPackage, language: Language) -> (RemovalAdvice, String) {
    if !package.required_by.is_empty() {
        let examples = join_names_limited(&package.required_by, 3);
        return (
            RemovalAdvice::CoreDependency,
            if language.is_zh() {
                format!(
                    "当前有 {} 个已安装 Python 包依赖它，例如 {}，删除后很容易引发 import 或命令报错。",
                    package.required_by.len(),
                    examples
                )
            } else {
                format!(
                    "{} installed Python package(s) depend on it, including {}, so removal is likely to cause import or command errors.",
                    package.required_by.len(),
                    examples
                )
            },
        );
    }

    if is_pip_foundational(&package.name) {
        return (
            RemovalAdvice::CoreDependency,
            if language.is_zh() {
                "它属于 Python 打包或安装基础设施的一部分，删除后修复成本通常更高。".to_string()
            } else {
                "It belongs to Python packaging or installation infrastructure, so recovery is usually more involved after removal.".to_string()
            },
        );
    }

    if !package.requires.is_empty() {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                format!(
                    "它依赖 {} 个 Python 包，删除后通常只影响当前包，但重新恢复时可能需要一起补装依赖。",
                    package.requires.len()
                )
            } else {
                format!(
                    "It depends on {} Python package(s); removal usually affects only this package, but restoring it may require reinstalling dependencies.",
                    package.requires.len()
                )
            },
        );
    }

    (
        RemovalAdvice::Removable,
        if language.is_zh() {
            "没有发现其他已安装 Python 包依赖它，删除后通常只影响这个包本身。".to_string()
        } else {
            "No other installed Python packages were found depending on it, so removal usually affects only this package itself.".to_string()
        },
    )
}

fn uv_runtime_advice(version: &str, aliases: &[String], language: Language) -> (RemovalAdvice, String) {
    let generic_aliases: Vec<String> = aliases
        .iter()
        .filter(|alias| alias.as_str() == "python" || alias.as_str() == "python3" || alias.starts_with("python3."))
        .cloned()
        .collect();

    if !generic_aliases.is_empty() {
        return (
            RemovalAdvice::CoreDependency,
            if language.is_zh() {
                format!(
                    "这个 uv 运行时当前提供 {} 别名，删除后可能直接影响脚本、虚拟环境或 shebang 调用。",
                    generic_aliases.join(", ")
                )
            } else {
                format!(
                    "This uv runtime currently provides aliases such as {}, so removal may directly affect scripts, virtual environments, or shebang targets.",
                    generic_aliases.join(", ")
                )
            },
        );
    }

    if !aliases.is_empty() {
        return (
            RemovalAdvice::Keep,
            if language.is_zh() {
                format!(
                    "它仍然挂着 {} 这些命令别名，删除后可能需要手动调整调用路径。",
                    aliases.join(", ")
                )
            } else {
                format!(
                    "It still exposes command aliases ({}) and may need manual path updates after removal.",
                    aliases.join(", ")
                )
            },
        );
    }

    (
        RemovalAdvice::Removable,
        if language.is_zh() {
            format!("没有发现 python@{version} 的命令别名正在使用它，删除后通常只会移除这个运行时本身。")
        } else {
            format!("No active command aliases were found for python@{version}, so removal usually only removes this runtime itself.")
        },
    )
}

fn uv_tool_advice(name: &str, language: Language) -> (RemovalAdvice, String) {
    if is_uv_foundational(name) {
        (
            RemovalAdvice::Keep,
            if language.is_zh() {
                "它属于常见 Python 或 AI 命令行工作流里的基础工具，删除后通常不会牵连依赖，但会影响日常命令使用。".to_string()
            } else {
                "It is a common base tool in Python or AI CLI workflows; removal usually will not break dependencies, but it can disrupt daily commands.".to_string()
            },
        )
    } else {
        (
            RemovalAdvice::Removable,
            if language.is_zh() {
                "uv tool 安装的大多是独立命令，删除后通常只会影响这个工具本身。".to_string()
            } else {
                "Most `uv tool` installs are standalone commands, so removal usually affects only that tool itself.".to_string()
            },
        )
    }
}

fn mas_app_advice(_name: &str, language: Language) -> (RemovalAdvice, String) {
    (
        RemovalAdvice::Removable,
        if language.is_zh() {
            "未发现命令行包管理层面的依赖关系，删除后通常只会让这个桌面应用不可用。".to_string()
        } else {
            "No command-line package-manager dependency relationship was found, so removal usually only makes this desktop app unavailable.".to_string()
        },
    )
}

fn join_names_limited(names: &[String], limit: usize) -> String {
    if names.is_empty() {
        return "none".to_string();
    }

    let shown = names.iter().take(limit).cloned().collect::<Vec<_>>();
    if names.len() > limit {
        format!("{} ... (+{} more)", shown.join(", "), names.len() - limit)
    } else {
        shown.join(", ")
    }
}

fn is_brew_foundational(name: &str) -> bool {
    matches!(
        name,
        "openssl@3"
            | "ca-certificates"
            | "sqlite"
            | "readline"
            | "zlib"
            | "pcre2"
            | "python"
            | "python@3.11"
            | "python@3.12"
            | "python@3.13"
            | "node"
            | "rust"
            | "git"
            | "cmake"
            | "pkg-config"
    )
}

fn is_cask_foundational(name: &str) -> bool {
    matches!(
        name,
        "docker"
            | "docker-desktop"
            | "iterm2"
            | "visual-studio-code"
            | "cursor"
            | "warp"
    )
}

fn is_node_foundational(name: &str) -> bool {
    matches!(
        name,
        "npm" | "pnpm" | "yarn" | "corepack" | "typescript" | "ts-node" | "node-gyp"
    )
}

fn is_cargo_foundational(name: &str) -> bool {
    name.starts_with("cargo-") || matches!(name, "rust-analyzer" | "bacon" | "cross")
}

fn is_pip_foundational(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "pip" | "setuptools" | "wheel" | "virtualenv" | "pipenv" | "poetry" | "build" | "installer"
    )
}

fn is_uv_foundational(name: &str) -> bool {
    matches!(
        name,
        "uv" | "pip" | "poetry" | "ruff" | "basedpyright" | "mypy" | "black"
    )
}

fn generic_formula_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 Homebrew 安装的命令行或开发工具。")
    } else {
        format!("{name} is a command-line or developer tool installed via Homebrew.")
    }
}

fn generic_cask_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 Homebrew Cask 安装的 macOS 应用或二进制工具。")
    } else {
        format!("{name} is a macOS app or binary tool installed via Homebrew Cask.")
    }
}

fn generic_npm_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 Node.js 生态全局安装的开发工具或命令行包。")
    } else {
        format!("{name} is a globally installed developer tool or command-line package from the Node.js ecosystem.")
    }
}

fn generic_cargo_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 cargo 安装的 Rust 命令行工具。")
    } else {
        format!("{name} is a Rust command-line tool installed via cargo.")
    }
}

fn generic_pip_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 pip 安装的 Python 包或命令行工具。")
    } else {
        format!("{name} is a Python package or command-line tool installed via pip.")
    }
}

fn generic_uv_summary(version: &str, language: Language) -> String {
    if language.is_zh() {
        format!("这是 uv 管理的 Python 运行时，版本为 {version}，可供项目或工具链复用。")
    } else {
        format!("This is a uv-managed Python runtime, version {version}, available for projects and tools.")
    }
}

fn generic_uv_tool_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 uv tool 安装的命令行工具。")
    } else {
        format!("{name} is a command-line tool installed via uv tool.")
    }
}

fn generic_mas_summary(name: &str, language: Language) -> String {
    if language.is_zh() {
        format!("{name} 是一个通过 Mac App Store 安装的桌面应用。")
    } else {
        format!("{name} is a desktop app installed from the Mac App Store.")
    }
}

const PIP_SCAN_SCRIPT: &str = r#"
import importlib.metadata as md
import json
import pathlib
import re
import site
import sysconfig

def normalize(name: str) -> str:
    return re.sub(r"[-_.]+", "-", (name or "").strip()).lower()

def parse_requirement_name(requirement: str):
    if not requirement:
        return None
    candidate = re.split(r"[ ;(<>=!\\[]", requirement, 1)[0].strip()
    return candidate or None

packages = []
for dist in md.distributions():
    name = dist.metadata.get('Name') or ''
    version = getattr(dist, 'version', '') or ''
    if not name:
        continue

    size_bytes = 0
    latest = 0
    seen = set()
    for rel in dist.files or []:
        try:
            path = pathlib.Path(dist.locate_file(rel))
        except Exception:
            continue
        if not path.exists() or not path.is_file():
            continue
        key = str(path)
        if key in seen:
            continue
        seen.add(key)
        try:
            stat = path.stat()
        except OSError:
            continue
        size_bytes += stat.st_size
        latest = max(latest, int(max(stat.st_atime, stat.st_mtime)))

    dist_path = getattr(dist, '_path', None)
    location = str(pathlib.Path(dist_path).parent) if dist_path else ''
    requires = []
    for requirement in dist.requires or []:
        parsed = parse_requirement_name(requirement)
        if parsed:
            requires.append(parsed)
    packages.append({
        'name': name,
        'normalized_name': normalize(name),
        'version': version,
        'location': location,
        'size_bytes': size_bytes,
        'last_used_epoch': latest or None,
        'summary': dist.metadata.get('Summary'),
        'requires': requires,
    })

required_by = {}
for package in packages:
    required_by.setdefault(package['normalized_name'], [])

for package in packages:
    for requirement in package['requires']:
        required_by.setdefault(normalize(requirement), []).append(package['name'])

results = []
for package in packages:
    results.append({
        'name': package['name'],
        'version': package['version'],
        'location': package['location'],
        'size_bytes': package['size_bytes'],
        'last_used_epoch': package['last_used_epoch'],
        'summary': package['summary'],
        'requires': package['requires'],
        'required_by': sorted(set(required_by.get(package['normalized_name'], []))),
    })

results.sort(key=lambda item: item['size_bytes'], reverse=True)
print(json.dumps({
    'packages': results,
    'cache_dir': str(pathlib.Path.home() / 'Library' / 'Caches' / 'pip'),
    'scripts_dir': sysconfig.get_paths().get('scripts', ''),
    'user_site': site.getusersitepackages(),
}))
"#;

#[derive(Debug, Deserialize)]
struct BrewInfo {
    #[serde(default)]
    formulae: Vec<BrewFormula>,
    #[serde(default)]
    casks: Vec<BrewCask>,
}

#[derive(Debug, Deserialize)]
struct BrewFormula {
    name: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    installed: Vec<BrewInstalled>,
}

#[derive(Debug, Deserialize)]
struct BrewInstalled {
    version: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    runtime_dependencies: Vec<BrewRuntimeDependency>,
    #[serde(default)]
    installed_as_dependency: bool,
    #[serde(default)]
    installed_on_request: bool,
}

#[derive(Debug, Deserialize)]
struct BrewRuntimeDependency {
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct BrewCask {
    token: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    installed: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    installed_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct NpmLsOutput {
    #[serde(default)]
    dependencies: BTreeMap<String, NpmDependency>,
}

#[derive(Debug, Deserialize)]
struct NpmDependency {
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug)]
struct NodePackageScan {
    name: String,
    version: String,
    summary: Option<String>,
    install_path: PathBuf,
    references: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default, rename = "peerDependencies")]
    peer_dependencies: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug)]
struct CargoInstallEntry {
    name: String,
    version: String,
    binaries: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PipInventory {
    packages: Vec<PipPackage>,
    cache_dir: String,
    scripts_dir: String,
    user_site: String,
}

#[derive(Debug, Deserialize)]
struct PipPackage {
    name: String,
    version: String,
    location: String,
    size_bytes: u64,
    last_used_epoch: Option<i64>,
    summary: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    required_by: Vec<String>,
}

#[derive(Debug)]
struct UvRuntime {
    key: String,
    version: String,
    path: PathBuf,
}

#[derive(Debug)]
struct UvTool {
    name: String,
    version: String,
    summary: String,
    path: PathBuf,
    executables: Vec<String>,
}

#[derive(Debug, Default)]
struct UvToolReceipt {
    name: Option<String>,
    version: Option<String>,
    summary: Option<String>,
}

#[derive(Debug)]
struct MasInstallEntry {
    id: String,
    name: String,
    version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brew_formula_with_reverse_dependents_is_core() {
        let formula = BrewFormula {
            name: "openssl@3".to_string(),
            desc: None,
            dependencies: vec!["ca-certificates".to_string()],
            installed: vec![BrewInstalled {
                version: "3.0.0".to_string(),
                time: None,
                runtime_dependencies: vec![],
                installed_as_dependency: true,
                installed_on_request: false,
            }],
        };
        let reverse = BTreeMap::from([(
            "openssl@3".to_string(),
            BTreeSet::from(["wget".to_string(), "ffmpeg".to_string()]),
        )]);

        let (advice, _) = brew_formula_advice(&formula, &reverse, Language::ZhHans);
        assert_eq!(advice, RemovalAdvice::CoreDependency);
    }

    #[test]
    fn top_level_reference_map_links_packages() {
        let packages = vec![
            NodePackageScan {
                name: "typescript".to_string(),
                version: "1.0.0".to_string(),
                summary: None,
                install_path: PathBuf::from("/tmp/typescript"),
                references: BTreeSet::new(),
            },
            NodePackageScan {
                name: "ts-node".to_string(),
                version: "1.0.0".to_string(),
                summary: None,
                install_path: PathBuf::from("/tmp/ts-node"),
                references: BTreeSet::from(["typescript".to_string()]),
            },
        ];

        let refs = build_top_level_reference_map(&packages);
        assert_eq!(
            refs.get("typescript").cloned(),
            Some(BTreeSet::from(["ts-node".to_string()]))
        );
    }

    #[test]
    fn pip_package_with_required_by_is_core() {
        let package = PipPackage {
            name: "urllib3".to_string(),
            version: "1.0.0".to_string(),
            location: "/tmp".to_string(),
            size_bytes: 1,
            last_used_epoch: None,
            summary: None,
            requires: vec![],
            required_by: vec!["requests".to_string()],
        };

        let (advice, _) = pip_package_advice(&package, Language::ZhHans);
        assert_eq!(advice, RemovalAdvice::CoreDependency);
    }

    #[test]
    fn uv_runtime_with_python_alias_is_core() {
        let aliases = vec!["python3".to_string(), "python3.12".to_string()];
        let (advice, _) = uv_runtime_advice("3.12", &aliases, Language::ZhHans);
        assert_eq!(advice, RemovalAdvice::CoreDependency);
    }

    #[test]
    fn node_modules_root_can_be_derived_from_link_target() {
        let path = PathBuf::from(
            "/Users/demo/.hermes/node/lib/node_modules/@google/gemini-cli/bundle/gemini.js",
        );
        assert_eq!(
            node_modules_root_for_path(&path),
            Some(PathBuf::from("/Users/demo/.hermes/node/lib/node_modules"))
        );
    }
}
