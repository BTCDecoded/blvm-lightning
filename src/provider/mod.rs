//! Lightning provider implementations
//!
//! Supports multiple providers:
//! - LNBits (REST API)
//! - LDK (Lightning Development Kit)
//! - Stub (for testing)

use crate::error::LightningError;
use async_trait::async_trait;
use blvm_node::module::traits::ModuleContext;
use serde_json::Value;
use std::str::FromStr;

// Define types first, then submodules can import them
pub mod lnbits;
pub mod ldk;
pub mod stub;

/// Lightning provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    LNBits,
    LDK,
    Stub,
}

impl FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "lnbits" => Ok(ProviderType::LNBits),
            "ldk" => Ok(ProviderType::LDK),
            "stub" => Ok(ProviderType::Stub),
            _ => Err(format!("Unknown provider type: {}", s)),
        }
    }
}

/// Payment verification result
#[derive(Debug, Clone)]
pub struct PaymentVerificationResult {
    pub verified: bool,
    pub amount_msats: Option<u64>,
    pub timestamp: Option<u64>,
    pub metadata: Value,
}

/// Lightning provider trait
#[async_trait]
pub trait LightningProvider: Send + Sync {
    /// Verify a Lightning payment
    async fn verify_payment(
        &self,
        invoice: &str,
        payment_hash: &[u8; 32],
        payment_id: &str,
    ) -> Result<PaymentVerificationResult, LightningError>;

    /// Create a Lightning invoice
    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        expiry_seconds: u64,
    ) -> Result<String, LightningError>;

    /// Check if a payment is confirmed
    async fn is_payment_confirmed(&self, payment_hash: &[u8; 32]) -> Result<bool, LightningError>;

    /// Get the provider type
    fn provider_type(&self) -> ProviderType;
}

/// Create a Lightning provider based on type and context
pub fn create_provider(
    provider_type: ProviderType,
    ctx: &ModuleContext,
) -> Result<Box<dyn LightningProvider>, LightningError> {
    match provider_type {
        ProviderType::LNBits => {
            let api_url = ctx.get_config_or("lightning.lnbits.api_url", "");
            let api_key = ctx.get_config_or("lightning.lnbits.api_key", "");
            let wallet_id = ctx.get_config("lightning.lnbits.wallet_id").map(|s| s.to_string());
            
            let config = lnbits::LNBitsConfig {
                api_url: api_url.to_string(),
                api_key: api_key.to_string(),
                wallet_id,
            };
            
            Ok(Box::new(lnbits::LNBitsProvider::new(config)?))
        }
        ProviderType::LDK => {
            let data_dir = ctx.data_dir.clone();
            let network = ctx.get_config_or("lightning.ldk.network", "testnet");
            let node_private_key = ctx.get_config("lightning.ldk.node_private_key")
                .and_then(|s| hex::decode(s).ok())
                .map(|v| v.into_iter().collect());
            
            let config = ldk::LDKConfig {
                data_dir: std::path::PathBuf::from(data_dir),
                network: network.to_string(),
                node_private_key,
            };
            
            Ok(Box::new(ldk::LDKProvider::new(config)?))
        }
        ProviderType::Stub => {
            Ok(Box::new(stub::StubProvider::new()))
        }
    }
}

