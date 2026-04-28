# pkgoh

[简体中文](README.zh-CN.md) | English

`pkgoh` is an open-source macOS terminal asset manager for developer package ecosystems.

Install it, type `pkgoh` or the short alias `pkg`, and enter a keyboard-first TUI that helps you scan, review, filter, remove, and clean globally installed tools across multiple package managers.

## Preview

```text
┌ pkgoh ───────────────────────────────────────────────────────────────────────────┐
│ Source: All  Sort: Size ↓  Selected: 2  Reclaim: 1.4GB  Total: 38             │
│ Modules: [Homebrew npm pnpm cargo pip uv mas]  Search: hidden                 │
├───────────────────────────────────┬─────────────────────────────────────────────┤
│ #  Name         Source  Ver  Size │ Details                                     │
│ 1. hiddenbar    Homebrew ... 12KB │ Name: hiddenbar                             │
│ 2. uv           uv       ... 84MB │ Summary: Python/package management runtime  │
│ 3. pnpm         pnpm     ... 31MB │ Removal Advice: Keep Recommended            │
│ ...                               │ Reason: other global tools may depend on it │
├───────────────────────────────────┴─────────────────────────────────────────────┤
│ Action                                                                      │
│ Delete Confirmation                                                         │
│ Delete 2 selected item(s), estimated reclaim 1.4GB                          │
│ Press Delete again or Enter to run, Esc to cancel.                          │
├──────────────────────────────────────────────────────────────────────────────┤
│ ↑↓ Move  / Search  Number Jump  Space Select  Delete Remove  C Clean Cache │
│ R Refresh  S Sort Size  Esc Quit                                            │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Why pkgoh

Developer Macs accumulate tools from many places. After a while, it becomes difficult to answer:

- what is installed globally
- which tools take the most space
- what has not been used for months
- what can probably be removed safely
- how much space can be reclaimed before you confirm any action

`pkgoh` puts those answers into one consistent terminal interface.

## What it does

- scans Homebrew, npm, pnpm, cargo, pip, uv, and mas
- shows name, source, version, size, and last-used time in one list
- supports loading feedback while scanning, instead of a frozen screen
- sorts by size descending
- highlights large assets and long-unused assets
- supports multi-select with live reclaim estimation
- supports quick name filtering with `/`
- supports number jump, refresh, and quit confirmation
- executes real delete and cache-clean operations
- keeps a detail pane visible on the right, so there is no separate detail screen
- localizes the TUI to Simplified Chinese automatically when the system language is Chinese

## Supported sources

Current built-in source adapters:

- Homebrew formulas
- Homebrew casks
- npm global packages
- pnpm global packages
- cargo installed binaries
- pip global packages
- uv-managed Python runtimes
- uv tool installs
- Mac App Store apps via `mas`

The scanner architecture is plugin-oriented, so more managers can be added later through the same adapter model.

## Interaction model

- `↑` / `↓`: move
- `Space`: select or unselect current item
- `Delete`: prepare delete for selected items
- `C`: prepare cache cleanup for selected items
- `R`: prepare refresh
- `/`: start live search filtering
- `S`: sort by size descending
- `Esc`: leave search, cancel a pending action, or prepare quit
- `Enter`: confirm the current pending action
- `0-9`: number jump

## Removal advice tiers

Each asset is evaluated into one of three tiers:

- `Removable`: usually safe to remove; mainly affects that tool itself
- `Keep Recommended`: removal may cause manageable follow-up issues
- `Core Dependency`: likely to break other tools or workflows and may be hard to recover for non-technical users

The right-side detail panel also shows the reason behind the recommendation.

## Delete and permission behavior

`pkgoh` runs real delete commands. It does not fake removal.

Important behavior:

- selected items must be confirmed before delete or cache cleanup runs
- some operations do not need admin access and will run directly
- some operations may need admin access, especially certain Homebrew casks or system-level removals
- when admin access is needed, `pkgoh` keeps the password flow inside the TUI action area as much as possible, instead of letting a hidden terminal prompt break the interface
- after a successful delete, the current list is updated directly instead of forcing a full rescan every time

## Installation

### Option 1: Build from source

Make sure Rust is installed, then run:

```bash
cargo install --path . --root ~/.local
```

If `~/.local/bin` is already in your `PATH`, you can launch with:

```bash
pkg
```

or:

```bash
pkgoh
```

### Option 2: Download a GitHub release archive

This repository includes a GitHub Actions release workflow that builds:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Each release archive contains:

- `pkg`
- `pkgoh`
- `README.md`
- `README.zh-CN.md`
- `LICENSE`
- `pkgoh.example.toml`

Unpack the archive and place `pkg` and `pkgoh` into a directory that is already in your `PATH`.

## Config

Default config path:

```text
~/.config/pkgoh/pkgoh.toml
```

You can also point to a custom config file:

```bash
PKGOH_CONFIG=/path/to/pkgoh.toml pkg
```

Example config:

```toml
[sources]
brew = true
npm = true
pnpm = true
cargo = true
pip = true
uv = true
mas = true

[highlight]
large_size_mb = 500
unused_days = 90
```

## Architecture

The codebase is split into a few clear layers:

- `src/model.rs`: shared asset model and display helpers
- `src/plugins.rs`: source adapters and scan pipeline
- `src/actions.rs`: delete and cache-clean execution layer
- `src/app.rs`: TUI layout, interaction state, confirmations, and feedback
- `src/i18n.rs`: system language detection and localization switch
- `src/config.rs`: config loading and defaults

Both `pkg` and `pkgoh` use the same library entrypoint.

## GitHub release automation

`.github/workflows/release.yml` builds Intel and Apple Silicon binaries on macOS and uploads release archives automatically when you push a tag like `v0.1.0`.

## Known limitations

- detection is limited to the currently supported package managers
- tools installed manually, by unsupported managers, or via custom scripts may not appear
- size and last-used timestamps are best-effort values gathered from package-manager metadata and filesystem paths
- dependency analysis is heuristic; it is useful guidance, not a perfect guarantee
- some system-level removals can still be slower than normal because the underlying package manager performs its own cleanup work

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
