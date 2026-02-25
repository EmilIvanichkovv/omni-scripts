# Formatting Tools Specification

This document outlines the tools and configuration for code formatting and linting in this project,
including pre-commit hooks for automated checks.

All tools are provided via Nix devShell for reproducibility.

## Overview

| Tool           | Purpose              | Target Files     |
| -------------- | -------------------- | ---------------- |
| `rustfmt`      | Rust code formatting | `*.rs`           |
| `clippy`       | Rust linting         | `*.rs`, `*.toml` |
| `prettier`     | Markdown formatting  | `*.md`           |
| `markdownlint` | Markdown linting     | `*.md`           |
| `pre-commit`   | Git hook management  | All configured   |

---

## Decisions

| Question             | Decision                                          |
| -------------------- | ------------------------------------------------- |
| **Scope**            | Entire `omni-scripts` repository                  |
| **Markdown linting** | Include `markdownlint` in addition to `prettier`  |
| **Nix integration**  | Yes - all tools provided via `flake.nix` devShell |
| **Strictness**       | Pre-commit hooks will block commits on failure    |
| **Line width**       | 100 characters for Markdown                       |

---

## Rust Tools

### rustfmt

**Purpose:** Automatically formats Rust code according to style guidelines.

**Usage:**

```bash
# Check formatting (dry-run)
cargo fmt --all -- --check

# Apply formatting
cargo fmt --all
```

**Configuration:** `rust/rustfmt.toml`

```toml
edition = "2021"
max_width = 100
tab_spaces = 4
use_small_heuristics = "Default"
```

---

### Clippy

**Purpose:** Rust linter that catches common mistakes and suggests improvements.

**Usage:**

```bash
# Run clippy with warnings as errors
cargo clippy --all-targets --all-features -- -D warnings

# Run clippy (warnings only)
cargo clippy --all-targets --all-features
```

---

## Markdown Tools

### Prettier

**Purpose:** Opinionated code formatter supporting Markdown.

**Usage:**

```bash
# Check formatting
prettier --check "**/*.md"

# Apply formatting
prettier --write "**/*.md"
```

**Configuration:** `.prettierrc`

```json
{
  "proseWrap": "always",
  "printWidth": 100,
  "tabWidth": 2,
  "useTabs": false
}
```

**Ignore file:** `.prettierignore`

```text
target/
node_modules/
*.lock
```

---

### markdownlint

**Purpose:** Lints Markdown files for style and syntax issues.

**Usage:**

```bash
# Lint markdown files
markdownlint "**/*.md"

# Fix auto-fixable issues
markdownlint --fix "**/*.md"
```

**Configuration:** `.markdownlint.json`

```json
{
  "MD013": false,
  "MD033": false,
  "MD041": false
}
```

---

## Pre-commit Hooks

**Purpose:** Manages and runs git pre-commit hooks automatically.

**Setup:**

```bash
# Install the hooks (run once after cloning, or auto-installed via shellHook)
pre-commit install

# Run manually on all files
pre-commit run --all-files
```

**Configuration:** `.pre-commit-config.yaml`

```yaml
repos:
  # General hooks
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v4.5.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-yaml
      - id: check-toml
      - id: check-merge-conflict

  # Rust formatting
  - repo: local
    hooks:
      - id: rustfmt
        name: cargo fmt
        entry: >-
          bash -lc 'cargo fmt --all -- --check || { echo "Rust formatting issues found. Run: cargo
          fmt --all"; exit 1; }'
        language: system
        pass_filenames: false
        files: '\.rs$'

      - id: clippy
        name: cargo clippy
        entry: >-
          bash -lc 'cargo clippy --all-targets --all-features -- -D warnings'
        language: system
        pass_filenames: false
        files: '\.(rs|toml)$'

  # Markdown formatting
  - repo: https://github.com/pre-commit/mirrors-prettier
    rev: v3.1.0
    hooks:
      - id: prettier
        types: [markdown]
        args: ["--prose-wrap", "always", "--print-width", "100"]

  # Markdown linting
  - repo: https://github.com/igorshubovych/markdownlint-cli
    rev: v0.39.0
    hooks:
      - id: markdownlint
        args: ["--fix"]
```

---

## Nix Integration

Add the following tools to the `devShell` in `flake.nix`:

```nix
devShells.default = pkgs.mkShell {
  buildInputs = with pkgs; [
    # Rust tools
    rustfmt
    clippy

    # Markdown tools
    nodePackages.prettier
    nodePackages.markdownlint-cli

    # Pre-commit
    pre-commit
  ];

  shellHook = ''
    # Install pre-commit hooks if not already installed
    if [ -f .pre-commit-config.yaml ] && [ ! -f .git/hooks/pre-commit ]; then
      pre-commit install
    fi
  '';
};
```

---

## Directory Structure (Files to Create)

```text
omni-scripts/
├── .pre-commit-config.yaml      # Pre-commit hook configuration
├── .prettierrc                  # Prettier configuration
├── .prettierignore              # Files to ignore for Prettier
├── .markdownlint.json           # Markdownlint rules
├── flake.nix                    # Nix flake with devShell (updated)
└── rust/
    └── rustfmt.toml             # Rust formatting configuration
```

---

## Next Steps

1. Create configuration files:
   - `.pre-commit-config.yaml`
   - `.prettierrc`
   - `.prettierignore`
   - `.markdownlint.json`
   - `rust/rustfmt.toml`
2. Update `flake.nix` with devShell tools
3. Test the complete setup with `pre-commit run --all-files`
