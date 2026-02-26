# Contributing to Grub

Thanks for your interest in contributing to Grub! This guide covers the open source components: the Rust core library (`core/`) and CLI (`cli/`).

> **Note:** The iOS and Android apps are proprietary and live in a separate private repository. Contributions are accepted for the Rust workspace (`core/`, `cli/`).

## Development Setup

### Rust Toolchain

Install via [rustup](https://rustup.rs/) (stable channel). The workspace uses Rust 2024 edition.

### Pre-commit Hooks

We use pre-commit hooks to enforce formatting, linting, and tests locally:

```sh
pre-commit install
pre-commit install --hook-type pre-push
```

## Building

```sh
cargo build --workspace
```

## Running Tests

```sh
cargo test --workspace
```

Unit tests only — integration tests hit external APIs. Tests use in-memory SQLite (`Database::open_in_memory()`), so no external API calls or database setup is needed.

## Linting Requirements

All lints must pass with zero issues before committing.

```sh
cargo clippy --workspace -- -D warnings
cargo fmt --all --check
```

## Code Style

- Clippy pedantic lints are enabled. `unsafe` code is forbidden via `[lints.rust]` in Cargo.toml.
- Use `anyhow::Result` for error handling.
- Run `cargo fmt` before committing.

## PR Guidelines

- Use descriptive PR titles
- Reference related issues in the PR description
- All CI checks must pass (formatting, linting, tests)
- Pre-commit hooks enforce checks locally before push
