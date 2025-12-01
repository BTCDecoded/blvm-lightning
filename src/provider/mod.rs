//! Lightning provider implementations
//!
//! Supports multiple providers:
//! - LNBits (REST API)
//! - LDK (Lightning Development Kit)
//! - Stub (for testing)

pub mod lnbits;
pub mod ldk;
pub mod stub;

pub use crate::provider::{ProviderType, LightningProvider, PaymentVerificationResult, create_provider};

