# Contributing to pkgoh

Thanks for considering a contribution.

## Development

```bash
cargo fmt
cargo test --offline
cargo run --bin pkg
```

## Guidelines

- keep the TUI responsive during scanning and long-running operations
- prefer clear, reversible interactions over aggressive automation
- keep localization in mind when adding user-facing copy
- avoid shipping misleading delete or cleanup behavior
- document any new package-manager adapter in both `README.md` and `README.zh-CN.md`

## Pull requests

Please include:

- what changed
- why it changed
- how you tested it
- any user-visible behavior changes
