//! # omni-lib
//!
//! Shared utilities for omni-scripts Rust tools.
//!
//! This library provides common functionality that can be reused across
//! multiple CLI/TUI applications in the omni-scripts repository.
//!
//! ## Planned Modules
//!
//! - `git`: Common git operations and helpers
//! - `tui`: Shared TUI components and themes
//! - `cli`: CLI argument parsing helpers
//!
//! ## Usage
//!
//! Add to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! omni-lib = { path = "../omni-lib" }
//! ```

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_exists() {
        assert!(!VERSION.is_empty());
    }
}
