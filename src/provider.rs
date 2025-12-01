//! Lightning provider abstraction
//!
//! Supports multiple Lightning providers:
//! - LNBits (REST API)
//! - LDK (Lightning Development Kit, Rust native)
//! - Stub (for testing)

use crate::error::LightningError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Lightning provider type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderType {
    /// LNBits REST API provider
    LNBits,
    /// LDK (Lightning Development Kit) Rust native provider
    LDK,
    /// Stub provider (for testing, always succeeds)
    Stub,
}

impl Default for ProviderType {
    fn default() -> Self {
        ProviderType::LNBits // Default to LNBits for simplicity
    }
}

impl std::str::FromStr for ProviderType {
    type Err = LightningError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "lnbits" => Ok(ProviderType::LNBits),
            "ldk" => Ok(ProviderType::LDK),
            "stub" => Ok(ProviderType::Stub),
            _ => Err(LightningError::ProcessorError(format!(
                "Unknown provider type: {} (supported: lnbits, ldk, stub)",
                s
            ))),
        }
    }
}

/// Payment verification result
#[derive(Debug, Clone)]
pub struct PaymentVerificationResult {
    /// Whether payment is verified
    pub verified: bool,
    /// Payment amount in millisatoshis (if available)
    pub amount_msats: Option<u64>,
    /// Payment timestamp (if available)
    pub timestamp: Option<u64>,
    /// Additional provider-specific data
    pub metadata: serde_json::Value,
}

/// Factory function to create a Lightning provider from configuration
pub fn create_provider(
    provider_type: ProviderType,
    ctx: &bllvm_node::module::traits::ModuleContext,
) -> Result<Box<dyn LightningProvider>, LightningError> {
    match provider_type {
        ProviderType::LNBits => {
            let api_url = ctx
                .get_config("lightning.lnbits.api_url")
                .or_else(|| ctx.get_config("lightning.node_url"))
                .ok_or_else(|| LightningError::ConfigError("LNBits API URL not configured".to_string()))?
                .clone();
            
            let api_key = ctx
                .get_config("lightning.lnbits.api_key")
                .ok_or_else(|| LightningError::ConfigError("LNBits API key not configured".to_string()))?
                .clone();
            
            let wallet_id = ctx.get_config("lightning.lnbits.wallet_id").cloned();
            
            let config = crate::provider::lnbits::LNBitsConfig {
                api_url,
                api_key,
                wallet_id,
            };
            
            Ok(Box::new(crate::provider::lnbits::LNBitsProvider::new(config)?))
        }
        ProviderType::LDK => {
            let data_dir_str = ctx
                .get_config("lightning.ldk.data_dir")
                .map(|s| s.clone())
                .unwrap_or_else(|| "data/ldk".to_string());
            let data_dir = std::path::PathBuf::from(data_dir_str);
            
            let network = ctx
                .get_config_or("lightning.ldk.network", "testnet");
            
            let node_private_key = ctx
                .get_config("lightning.ldk.node_private_key")
                .and_then(|s| hex::decode(s).ok());
            
            let config = crate::provider::ldk::LDKConfig {
                data_dir,
                network,
                node_private_key,
            };
            
            Ok(Box::new(crate::provider::ldk::LDKProvider::new(config)?))
        }
        ProviderType::Stub => {
            Ok(Box::new(crate::provider::stub::StubProvider::new()))
        }
    }
}

/// Lightning provider trait - implemented by all providers
#[async_trait]
pub trait LightningProvider: Send + Sync {
    /// Verify a Lightning payment
    ///
    /// # Arguments
    ///
    /// * `invoice` - Lightning invoice (BOLT11 format)
    /// * `payment_hash` - Payment hash to verify
    /// * `payment_id` - Payment ID for tracking
    ///
    /// # Returns
    ///
    /// Payment verification result
    async fn verify_payment(
        &self,
        invoice: &str,
        payment_hash: &[u8; 32],
        payment_id: &str,
    ) -> Result<PaymentVerificationResult, LightningError>;

    /// Create a Lightning invoice
    ///
    /// # Arguments
    ///
    /// * `amount_msats` - Invoice amount in millisatoshis
    /// * `description` - Invoice description
    /// * `expiry_seconds` - Invoice expiry in seconds
    ///
    /// # Returns
    ///
    /// BOLT11 invoice string
    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        expiry_seconds: u64,
    ) -> Result<String, LightningError>;

    /// Check if a payment is confirmed
    ///
    /// # Arguments
    ///
    /// * `payment_hash` - Payment hash to check
    ///
    /// # Returns
    ///
    /// True if payment is confirmed
    async fn is_payment_confirmed(&self, payment_hash: &[u8; 32]) -> Result<bool, LightningError>;

    /// Get provider type
    fn provider_type(&self) -> ProviderType;
}

