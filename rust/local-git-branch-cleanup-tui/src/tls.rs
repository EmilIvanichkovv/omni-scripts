//! TLS setup shared by every HTTP client in the process.

/// Make ring the process-wide rustls crypto provider. reqwest is built without a provider of its
/// own (`rustls-no-provider`), so a client built before this call fails. Safe to call repeatedly.
pub fn install_crypto_provider() {
    // Err only means a provider is already installed, which is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}
