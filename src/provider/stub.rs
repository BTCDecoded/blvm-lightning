//! Stub provider.
//!
//! It can mint a placeholder invoice string. It has no payment record, so
//! verification and confirmation stay false.

use crate::error::LightningError;
use crate::provider::{LightningProvider, PaymentVerificationResult, ProviderType};
use async_trait::async_trait;
use tracing::debug;

/// Stub provider implementation
pub struct StubProvider;

impl Default for StubProvider {
    fn default() -> Self {
        Self::new()
    }
}

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
        debug!("Stub provider: no payment record for payment_id={payment_id}");

        Ok(PaymentVerificationResult {
            verified: false,
            amount_msats: None,
            timestamp: None,
            metadata: serde_json::json!({
                "provider": "stub",
                "note": "stub provider does not observe payments",
            }),
        })
    }

    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        _expiry_seconds: u64,
    ) -> Result<String, LightningError> {
        debug!(
            "Stub provider: creating invoice: amount={} msats, description={}",
            amount_msats, description
        );

        // Stub: Return a fake invoice
        // In production, this would be a real BOLT11 invoice
        Ok(format!("lnbc{amount_msats}u1pstub_invoice"))
    }

    async fn is_payment_confirmed(&self, _payment_hash: &[u8; 32]) -> Result<bool, LightningError> {
        Ok(false)
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Stub
    }

    async fn get_balance(&self) -> Result<Option<u64>, LightningError> {
        Ok(Some(0))
    }
}
