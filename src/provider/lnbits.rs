//! LNBits provider implementation
//!
//! Integrates with LNBits REST API for Lightning payments.

use crate::provider::{ProviderType, LightningProvider, PaymentVerificationResult};
use crate::error::LightningError;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};
use hex;

/// LNBits provider configuration
#[derive(Debug, Clone)]
pub struct LNBitsConfig {
    /// LNBits API URL (e.g., "https://lnbits.example.com")
    pub api_url: String,
    /// LNBits API key (admin or invoice key)
    pub api_key: String,
    /// Wallet ID (optional, for specific wallet operations)
    pub wallet_id: Option<String>,
}

/// LNBits provider implementation
pub struct LNBitsProvider {
    config: LNBitsConfig,
    http_client: Arc<Client>,
}

impl LNBitsProvider {
    /// Create a new LNBits provider
    pub fn new(config: LNBitsConfig) -> Result<Self, LightningError> {
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| LightningError::ProcessorError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            http_client: Arc::new(http_client),
        })
    }

    /// Make an authenticated request to LNBits API
    async fn request<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, LightningError> {
        let url = format!("{}/api/v1{}", self.config.api_url.trim_end_matches('/'), endpoint);
        
        let mut request = self
            .http_client
            .request(method, &url)
            .header("X-Api-Key", &self.config.api_key)
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| LightningError::ProcessorError(format!("LNBits API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LightningError::ProcessorError(format!(
                "LNBits API error: {} - {}",
                status, error_text
            )));
        }

        response
            .json::<T>()
            .await
            .map_err(|e| LightningError::ProcessorError(format!("Failed to parse LNBits response: {}", e)))
    }
}

#[async_trait]
impl LightningProvider for LNBitsProvider {
    async fn verify_payment(
        &self,
        invoice: &str,
        payment_hash: &[u8; 32],
        payment_id: &str,
    ) -> Result<PaymentVerificationResult, LightningError> {
        debug!("Verifying payment via LNBits: payment_id={}", payment_id);

        // LNBits API: Check payment status
        // GET /api/v1/payments/{payment_hash}
        let payment_hash_hex = hex::encode(payment_hash);
        let endpoint = format!("/payments/{}", payment_hash_hex);

        #[derive(Deserialize)]
        struct PaymentResponse {
            paid: bool,
            #[serde(rename = "amount")]
            amount_msats: Option<u64>,
            #[serde(rename = "time")]
            timestamp: Option<u64>,
        }

        match self.request::<PaymentResponse>(reqwest::Method::GET, &endpoint, None).await {
            Ok(payment) => {
                let verified = payment.paid;
                debug!(
                    "LNBits payment check: payment_id={}, verified={}, amount={:?}",
                    payment_id, verified, payment.amount_msats
                );

                Ok(PaymentVerificationResult {
                    verified,
                    amount_msats: payment.amount_msats,
                    timestamp: payment.timestamp,
                    metadata: serde_json::json!({
                        "provider": "lnbits",
                        "payment_hash": payment_hash_hex,
                    }),
                })
            }
            Err(e) => {
                // If payment not found, it might not be paid yet
                warn!("LNBits payment check failed: payment_id={}, error={}", payment_id, e);
                Ok(PaymentVerificationResult {
                    verified: false,
                    amount_msats: None,
                    timestamp: None,
                    metadata: serde_json::json!({
                        "provider": "lnbits",
                        "error": e.to_string(),
                    }),
                })
            }
        }
    }

    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        expiry_seconds: u64,
    ) -> Result<String, LightningError> {
        debug!("Creating invoice via LNBits: amount={} msats", amount_msats);

        // LNBits API: Create invoice
        // POST /api/v1/payments
        let endpoint = if let Some(wallet_id) = &self.config.wallet_id {
            format!("/payments?wallet={}", wallet_id)
        } else {
            "/payments".to_string()
        };

        #[derive(Serialize)]
        struct InvoiceRequest {
            out: bool, // false = invoice (receive payment)
            amount: u64,
            memo: String,
            expiry: u64,
        }

        #[derive(Deserialize)]
        struct InvoiceResponse {
            payment_request: String,
        }

        let request_body = InvoiceRequest {
            out: false,
            amount: amount_msats,
            memo: description.to_string(),
            expiry: expiry_seconds,
        };

        let response: InvoiceResponse = self
            .request(reqwest::Method::POST, &endpoint, Some(serde_json::to_value(request_body)
                .map_err(|e| LightningError::ProcessorError(format!("Failed to serialize request: {}", e)))?))
            .await?;

        debug!("LNBits invoice created: {}", response.payment_request);
        Ok(response.payment_request)
    }

    async fn is_payment_confirmed(&self, payment_hash: &[u8; 32]) -> Result<bool, LightningError> {
        let payment_hash_hex = hex::encode(payment_hash);
        let endpoint = format!("/payments/{}", payment_hash_hex);

        #[derive(Deserialize)]
        struct PaymentResponse {
            paid: bool,
        }

        match self.request::<PaymentResponse>(reqwest::Method::GET, &endpoint, None).await {
            Ok(payment) => Ok(payment.paid),
            Err(_) => Ok(false), // Payment not found = not confirmed
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::LNBits
    }
}

