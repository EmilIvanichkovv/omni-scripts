# Default recipe - show available commands
default:
    @just --list

# Format and lint everything (main workflow)
format-and-lint:
    pre-commit run --all-files

# Build Rust project
build:
    cd rust && cargo build

# Run Rust tests
test-rust:
    cd rust && cargo test

# Quick validation (format check only, no modifications)
check:
    cd rust && cargo fmt --all -- --check
    prettier --check --prose-wrap always --print-width 100 "**/*.md"
