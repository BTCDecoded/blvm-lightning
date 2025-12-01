//! Stub provider implementation
//!
//! For testing and development. Always succeeds verification.

use crate::provider::{ProviderType, LightningProvider, PaymentVerificationResult};
use crate::error::LightningError;
use async_trait::async_trait;
use tracing::debug;

/// Stub provider implementation
pub struct StubProvider;

impl StubProvider {
    /// Create a new stub provider
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LightningProvider for StubProvider {
    async fn verify_payment(
        &self,
        _invoice: &str,
        _payment_hash: &[u8; 32],
        payment_id: &str,
    ) -> Result<PaymentVerificationResult, LightningError> {
        debug!("Stub provider: verifying payment (always succeeds): payment_id={}", payment_id);
        
        // Stub: Always return verified
        Ok(PaymentVerificationResult {
            verified: true,
            amount_msats: Some(1000), // Stub amount
            timestamp: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            metadata: serde_json::json!({
                "provider": "stub",
                "note": "This is a stub implementation for testing",
            }),
        })
    }

    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        _expiry_seconds: u64,
    ) -> Result<String, LightningError> {
        debug!("Stub provider: creating invoice: amount={} msats, description={}", amount_msats, description);
        
        // Stub: Return a fake invoice
        // In production, this would be a real BOLT11 invoice
        Ok(format!("lnbc{}u1pstub_invoice", amount_msats))
    }

    async fn is_payment_confirmed(&self, _payment_hash: &[u8; 32]) -> Result<bool, LightningError> {
        // Stub: Always return true
        Ok(true)
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Stub
    }
}

