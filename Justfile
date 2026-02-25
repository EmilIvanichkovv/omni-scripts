# Default recipe - show available commands
default:
    @just --list

# ============================================================================
# Formatting
# ============================================================================

# Format all Rust code
fmt-rust:
    cd rust && cargo fmt --all

# Format all Markdown files
fmt-md:
    prettier --write --prose-wrap always --print-width 100 "**/*.md"

# Format everything (Rust + Markdown)
fmt: fmt-rust fmt-md

# ============================================================================
# Linting
# ============================================================================

# Check Rust formatting (without modifying files)
check-rust-fmt:
    cd rust && cargo fmt --all -- --check

# Run clippy linter on Rust code
lint-rust:
    cd rust && cargo clippy --all-targets --all-features -- -D warnings

# Lint Markdown files (with auto-fix)
lint-md:
    markdownlint --fix "**/*.md"

# Check Markdown formatting (without modifying files)
check-md-fmt:
    prettier --check --prose-wrap always --print-width 100 "**/*.md"

# Lint everything
lint: lint-rust lint-md

# Check all formatting (without modifying files)
check-fmt: check-rust-fmt check-md-fmt

# ============================================================================
# Pre-commit
# ============================================================================

# Run all pre-commit hooks on all files
pre-commit:
    pre-commit run --all-files

# Install pre-commit hooks
pre-commit-install:
    pre-commit install

# Update pre-commit hooks to latest versions
pre-commit-update:
    pre-commit autoupdate

# ============================================================================
# Build & Test
# ============================================================================

# Build Rust project
build:
    cd rust && cargo build

# Build Rust project in release mode
build-release:
    cd rust && cargo build --release

# Run Rust tests
test:
    cd rust && cargo test

# Run Rust tests with output
test-verbose:
    cd rust && cargo test -- --nocapture

# ============================================================================
# Combined Commands
# ============================================================================

# Format and lint everything
fix: fmt lint

# Full check: format check + lint + test
check: check-fmt lint test
