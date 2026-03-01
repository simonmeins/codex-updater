# Repository Guidelines

## Project Structure & Module Organization

This repository contains a small Rust CLI for updating the Codex binary on Linux.

- `src/main.rs`: main application logic, CLI parsing, GitHub release checks, download verification, extraction, and atomic install.
- `systemd/codex-updater.service`: optional boot-time `systemd` unit for running the updater once at startup.
- `Cargo.toml`: package metadata and direct dependencies.
- `Cargo.lock`: locked dependency graph for reproducible builds; commit changes to this file.
- `README.md`: usage, installation, and `systemd` setup instructions.

Keep new Rust modules under `src/` and keep system integration files under `systemd/` unless there is a strong reason to change the layout.

## Build, Test, and Development Commands

- `cargo build --release`: builds the optimized updater binary.
- `cargo test`: runs unit tests in `src/main.rs`.
- `cargo fmt --all`: formats the codebase using Rustfmt.
- `./target/release/codex-updater --check-only`: checks the installed Codex version without modifying the system.
- `sudo ./target/release/codex-updater`: performs an update if a newer release is available.

Run `cargo fmt --all` and `cargo test` before opening a PR.

## Coding Style & Naming Conventions

Use standard Rust style with 4-space indentation and let `cargo fmt` enforce formatting. Prefer small, focused functions with explicit error handling via `anyhow::Result`. Use `snake_case` for functions and variables, `SCREAMING_SNAKE_CASE` for constants, and clear, descriptive names for filesystem and network operations.

Security-sensitive code should stay conservative: validate archive contents, verify digests, and avoid non-atomic replacement logic.

## Testing Guidelines

Unit tests live alongside the implementation in `src/main.rs` under `#[cfg(test)]`. Add tests for version parsing, archive validation, path safety, and error cases when changing update logic. Name tests by behavior, for example `parses_prerelease_version` or `rejects_non_flat_archive_paths`.

## Commit & Pull Request Guidelines

Git history is not available in this workspace, so use a simple, consistent convention: imperative, concise commit subjects such as `Add boot-time systemd service` or `Harden archive validation`.

Pull requests should include:

- a short summary of the change
- any security or system-level impact
- commands run for verification (for example `cargo test`)
- updated documentation when behavior or setup changes
