//! Module API for inter-module communication
//!
//! Exposes create_invoice, verify_payment, get_balance for dashboards and automation.

use blvm_node::module::inter_module::api::ModuleAPI;
use blvm_node::module::traits::ModuleError;
use std::sync::Arc;

/// Lightning module API for other modules
pub struct LightningModuleApi {
    processor: Arc<crate::processor::LightningProcessor>,
}

impl LightningModuleApi {
    /// Create a new Lightning module API
    pub fn new(processor: Arc<crate::processor::LightningProcessor>) -> Self {
        Self { processor }
    }
}

#[async_trait::async_trait]
impl ModuleAPI for LightningModuleApi {
    async fn handle_request(
        &self,
        method: &str,
        params: &[u8],
        _caller_module_id: &str,
    ) -> Result<Vec<u8>, ModuleError> {
        match method {
            "create_invoice" => {
                let params_json: serde_json::Value =
                    serde_json::from_slice(params).unwrap_or(serde_json::json!({}));
                let amount_msats = params_json
                    .get("amount_msats")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        ModuleError::OperationError(
                            "create_invoice requires amount_msats (u64)".to_string(),
                        )
                    })?;
                let description = params_json
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let expiry_seconds = params_json
                    .get("expiry_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3600);
                let invoice = self
                    .processor
                    .create_invoice(amount_msats, description, expiry_seconds)
                    .await
                    .map_err(|e| {
                        ModuleError::OperationError(format!("create_invoice failed: {}", e))
                    })?;
                serde_json::to_vec(&serde_json::json!({ "invoice": invoice }))
                    .map_err(|e| ModuleError::OperationError(format!("Serialization error: {}", e)))
            }
            "verify_payment" => {
                let params_json: serde_json::Value =
                    serde_json::from_slice(params).unwrap_or(serde_json::json!({}));
                let invoice = params_json
                    .get("invoice")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ModuleError::OperationError(
                            "verify_payment requires invoice (string)".to_string(),
                        )
                    })?;
                let payment_hash = params_json
                    .get("payment_hash")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ModuleError::OperationError(
                            "verify_payment requires payment_hash (hex string)".to_string(),
                        )
                    })?;
                let payment_id = params_json
                    .get("payment_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let result = self
                    .processor
                    .verify_payment_api(invoice, payment_hash, payment_id)
                    .await
                    .map_err(|e| {
                        ModuleError::OperationError(format!("verify_payment failed: {}", e))
                    })?;
                serde_json::to_vec(&serde_json::json!({
                    "verified": result.verified,
                    "amount_msats": result.amount_msats,
                    "timestamp": result.timestamp,
                    "metadata": result.metadata
                }))
                .map_err(|e| ModuleError::OperationError(format!("Serialization error: {}", e)))
            }
            "get_balance" => {
                let balance = self.processor.get_balance().await.map_err(|e| {
                    ModuleError::OperationError(format!("get_balance failed: {}", e))
                })?;
                serde_json::to_vec(&serde_json::json!({
                    "balance_sats": balance,
                    "supported": balance.is_some()
                }))
                .map_err(|e| ModuleError::OperationError(format!("Serialization error: {}", e)))
            }
            _ => Err(ModuleError::OperationError(format!(
                "Unknown method: {}",
                method
            ))),
        }
    }

    fn list_methods(&self) -> Vec<String> {
        vec![
            "create_invoice".to_string(),
            "verify_payment".to_string(),
            "get_balance".to_string(),
        ]
    }

    fn api_version(&self) -> u32 {
        1
    }
}
