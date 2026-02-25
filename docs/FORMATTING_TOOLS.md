# Formatting Tools

This repository uses automated code formatting and linting via pre-commit hooks. All tools are
provided via Nix devShell for reproducibility.

## Quick Start

```bash
# Enter the dev environment (hooks auto-install)
nix develop

# Format and lint everything
just fix
```

## Tools

| Tool           | Purpose              | Target Files     |
| -------------- | -------------------- | ---------------- |
| `rustfmt`      | Rust code formatting | `*.rs`           |
| `clippy`       | Rust linting         | `*.rs`, `*.toml` |
| `prettier`     | Markdown formatting  | `*.md`           |
| `markdownlint` | Markdown linting     | `*.md`           |

## Justfile Commands

| Command      | Description                          |
| ------------ | ------------------------------------ |
| `just fix`   | Format + lint all code (CI-like)     |
| `just fmt`   | Format all code                      |
| `just lint`  | Run all linters                      |
| `just build` | Build Rust projects                  |
| `just test`  | Run tests                            |
| `just check` | Quick validation (format check only) |

## Design Decisions

| Question             | Decision                                           |
| -------------------- | -------------------------------------------------- |
| **Scope**            | Entire `omni-scripts` repository                   |
| **Markdown linting** | Include `markdownlint` in addition to `prettier`   |
| **Nix integration**  | Hooks defined in `flake.nix` using `git-hooks.nix` |
| **Strictness**       | Pre-commit hooks block commits on failure          |
| **Line width**       | 100 characters for Markdown and Rust               |

## How It Works

Pre-commit hooks are defined in `flake.nix` using
[git-hooks.nix](https://github.com/cachix/git-hooks.nix). When you run `nix develop`, the hooks are
automatically installed to `.git/hooks/pre-commit`.

On every commit, the following checks run automatically:

- **Rust**: `cargo fmt` + `clippy` (warnings as errors)
- **Markdown**: `prettier` + `markdownlint`
- **General**: trailing whitespace, end-of-file fixer, YAML/TOML validation, merge conflict check

## Configuration Files

| File                 | Purpose                  |
| -------------------- | ------------------------ |
| `.prettierrc`        | Prettier settings        |
| `.prettierignore`    | Files to skip formatting |
| `.markdownlint.json` | Markdownlint rules       |
| `rust/rustfmt.toml`  | Rust formatting settings |
