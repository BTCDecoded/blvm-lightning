//! Lightning payment processor

use crate::error::LightningError;
use crate::invoice::{InvoiceData, InvoiceParser};
use crate::provider::{LightningProvider, ProviderType, create_provider};
use serde::Serialize;

/// Lightning module status (for CLI)
#[derive(Debug, Clone, Serialize)]
pub struct LightningStatus {
    pub provider_type: String,
}
use blvm_node::module::EventType;
use blvm_node::module::ipc::protocol::EventPayload;
use blvm_node::module::ipc::protocol::ModuleMessage;
use blvm_node::module::traits::NodeAPI;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Lightning payment processor
pub struct LightningProcessor {
    /// Lightning provider (LNBits, LDK, or Stub)
    provider: Box<dyn LightningProvider>,
    /// Node API for storage and queries
    node_api: Arc<dyn NodeAPI>,
    /// Module DB for invoice storage (list-invoices)
    invoice_db: Option<Arc<dyn blvm_node::storage::database::Database>>,
    min_payment_sats: Option<u64>,
    max_payment_sats: Option<u64>,
}

/// `None` when the amount is inside the optional satoshi bounds. Amount is millisatoshis.
pub(crate) fn invoice_amount_rejection(
    amount_msats: u64,
    min_payment_sats: Option<u64>,
    max_payment_sats: Option<u64>,
) -> Option<String> {
    let amount_sats = amount_msats / 1000;
    if let Some(min) = min_payment_sats {
        if amount_sats < min {
            return Some(format!(
                "amount {amount_sats} sats is below min_payment_sats {min}"
            ));
        }
    }
    if let Some(max) = max_payment_sats {
        if amount_sats > max {
            return Some(format!(
                "amount {amount_sats} sats is above max_payment_sats {max}"
            ));
        }
    }
    None
}

/// A missing module, a reply that is not JSON, or a payment with no confirmations yields no event.
fn settled_from_payment_state(payment_id: &str, reply: Option<&[u8]>) -> Option<EventPayload> {
    let value: serde_json::Value = serde_json::from_slice(reply?).ok()?;
    let confirmations = value
        .get("confirmation_count")
        .and_then(|item| item.as_u64())
        .unwrap_or(0);
    if confirmations == 0 {
        return None;
    }
    let tx_hash = value
        .get("tx_hash")
        .and_then(|item| item.as_str())
        .and_then(|hex_str| blvm_node::storage::hashing::hash_from_rpc_hex(hex_str).ok())?;
    Some(EventPayload::PaymentSettled {
        payment_id: payment_id.to_string(),
        tx_hash,
        confirmations: confirmations as u32,
    })
}

impl LightningProcessor {
    /// Get module status for CLI
    pub fn get_status(&self) -> LightningStatus {
        LightningStatus {
            provider_type: format!("{:?}", self.provider.provider_type()),
        }
    }

    /// Create a new Lightning processor
    pub async fn new(
        ctx: &blvm_node::module::traits::ModuleContext,
        node_api: Arc<dyn NodeAPI>,
    ) -> Result<Self, LightningError> {
        // Determine provider type from config
        let provider_type_str = ctx.get_config_or("lightning.provider", "lnbits");
        let provider_type = ProviderType::from_str(&provider_type_str)
            .map_err(|e| LightningError::ConfigError(format!("Invalid provider type: {e}")))?;

        info!(
            "Initializing Lightning processor with provider: {:?}",
            provider_type
        );

        // Create provider, then let it see the node. LDK anchors its chain tip here,
        // before any invoice is registered and before the peer listener starts.
        let provider = create_provider(provider_type, ctx)?;
        provider.attach_node(Arc::clone(&node_api)).await?;
        let min_payment_sats = ctx
            .get_config("lightning.min_payment_sats")
            .and_then(|value| value.parse().ok());
        let max_payment_sats = ctx
            .get_config("lightning.max_payment_sats")
            .and_then(|value| value.parse().ok());

        // Store provider info in module DB
        const CONFIG_TREE: &str = "config";
        let data_dir = std::path::Path::new(&ctx.data_dir);
        let invoice_db = blvm_sdk::module::ModuleDb::open(data_dir)
            .ok()
            .map(|m| m.as_db());
        if let Some(ref db) = invoice_db {
            if let Ok(tree) = db.open_tree(CONFIG_TREE) {
                let provider_type_str = match provider.provider_type() {
                    ProviderType::LNBits => "lnbits",
                    ProviderType::LDK => "ldk",
                    ProviderType::Stub => "stub",
                };
                let _ = tree.insert(
                    b"lightning_config:provider_type",
                    provider_type_str.as_bytes(),
                );
                let _ = tree.insert(b"lightning_config:channel_count", &0u64.to_be_bytes());
                let _ = tree.insert(b"lightning_config:total_capacity_sats", &0u64.to_be_bytes());
            }
        }

        Ok(Self {
            provider,
            node_api,
            invoice_db,
            min_payment_sats,
            max_payment_sats,
        })
    }

    /// Handle an event from the node
    pub async fn handle_event(
        &self,
        event: &ModuleMessage,
        node_api: &dyn NodeAPI,
    ) -> Result<(), LightningError> {
        match event {
            ModuleMessage::Event(event_msg) => {
                match event_msg.event_type {
                    EventType::PaymentRequestCreated => {
                        if let EventPayload::PaymentRequestCreated {
                            payment_id,
                            invoice,
                            ..
                        } = &event_msg.payload
                        {
                            debug!("Processing payment request: {}", payment_id);
                            if let Some(invoice_str) = invoice {
                                if let Some(ref db) = self.invoice_db {
                                    crate::invoice_store::store_invoice(
                                        db,
                                        payment_id,
                                        invoice_str,
                                    );
                                }
                                self.process_payment(invoice_str, payment_id, node_api)
                                    .await?;
                            }
                        }
                    }
                    EventType::PaymentSettled => {
                        debug!("Payment settled event received");
                    }
                    EventType::PaymentFailed => {
                        debug!("Payment failed event received");
                    }
                    EventType::NewBlock => {
                        if let Some((hash, height)) = event_msg.payload.as_new_block() {
                            self.provider
                                .on_block_connected(node_api, hash, height)
                                .await?;
                        }
                    }
                    EventType::BlockDisconnected => {
                        if let Some((hash, height)) = event_msg.payload.as_block_disconnected() {
                            self.provider
                                .on_block_disconnected(node_api, hash, height)
                                .await?;
                        }
                    }
                    _ => {
                        // Ignore other events
                    }
                }
            }
            _ => {
                // Not an event message
            }
        }

        Ok(())
    }

    /// Process a Lightning payment
    pub async fn process_payment(
        &self,
        invoice: &str,
        payment_id: &str,
        node_api: &dyn NodeAPI,
    ) -> Result<(), LightningError> {
        // Early exit: Check if invoice is empty (cheap check before expensive parsing)
        if invoice.is_empty() {
            return Err(LightningError::InvoiceError("Empty invoice".to_string()));
        }

        // Early exit: Check if payment_id is empty (cheap check)
        if payment_id.is_empty() {
            return Err(LightningError::ProcessorError(
                "Empty payment_id".to_string(),
            ));
        }

        // Early exit: Check if node_url is configured before HTTP call
        let node_url = self.node_api.get_lightning_node_url().await?;
        if node_url.is_none() {
            // Try to get from NodeAPI, but check first
            if node_api.get_lightning_node_url().await.is_err() {
                return Err(LightningError::ProcessorError(
                    "Lightning node URL not configured".to_string(),
                ));
            }
        }

        info!(
            "Processing Lightning payment: {} for payment_id: {}",
            invoice, payment_id
        );

        // Parse invoice
        let invoice_data = self.parse_invoice(invoice)?;

        // Check if invoice is expired
        if invoice_data.is_expired() {
            warn!("Invoice expired for payment_id: {}", payment_id);
            let payload = EventPayload::PaymentFailed {
                payment_id: payment_id.to_string(),
                reason: "Invoice expired".to_string(),
            };
            if let Err(e) = node_api
                .publish_event(EventType::PaymentFailed, payload)
                .await
            {
                debug!("Failed to publish PaymentFailed: {}", e);
            }
            return Err(LightningError::InvoiceError("Invoice expired".to_string()));
        }

        // Get payment hash from invoice
        let payment_hash = invoice_data.payment_hash();

        // Verify payment via provider
        let verification_result = match self
            .provider
            .verify_payment(invoice, &payment_hash, payment_id)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let reason = e.to_string();
                let payload = EventPayload::PaymentFailed {
                    payment_id: payment_id.to_string(),
                    reason: reason.clone(),
                };
                if let Err(pub_e) = node_api
                    .publish_event(EventType::PaymentFailed, payload)
                    .await
                {
                    debug!("Failed to publish PaymentFailed: {}", pub_e);
                }
                return Err(e);
            }
        };

        if verification_result.verified {
            info!(
                "Lightning payment verified via {:?}: payment_id={}, amount={:?} msats",
                self.provider.provider_type(),
                payment_id,
                verification_result.amount_msats
            );

            let payload = EventPayload::PaymentVerified {
                payment_id: payment_id.to_string(),
                amount_msats: verification_result.amount_msats.unwrap_or(0),
                invoice: invoice.to_string(),
            };
            if let Err(e) = node_api
                .publish_event(EventType::PaymentVerified, payload)
                .await
            {
                debug!("Failed to publish PaymentVerified: {}", e);
            }

            // Settled on-chain state lives in blvm-payment. A missing module skips PaymentSettled.
            let params = serde_json::to_vec(&serde_json::json!([payment_id])).unwrap_or_default();
            let reply = node_api
                .call_module(Some("blvm-payment"), "getpaymentstate", params)
                .await
                .ok();
            if let Some(payload) = settled_from_payment_state(payment_id, reply.as_deref()) {
                if let Err(e) = node_api
                    .publish_event(EventType::PaymentSettled, payload)
                    .await
                {
                    debug!("Failed to publish PaymentSettled: {}", e);
                }
            }
        } else {
            warn!(
                "Lightning payment verification failed: payment_id={}",
                payment_id
            );
            let reason = "Verification failed".to_string();
            let payload = EventPayload::PaymentFailed {
                payment_id: payment_id.to_string(),
                reason: reason.clone(),
            };
            if let Err(e) = node_api
                .publish_event(EventType::PaymentFailed, payload)
                .await
            {
                debug!("Failed to publish PaymentFailed: {}", e);
            }
        }

        Ok(())
    }

    /// Parse Lightning invoice (BOLT11)
    fn parse_invoice(&self, invoice: &str) -> Result<InvoiceData, LightningError> {
        InvoiceParser::parse(invoice)
    }

    /// Verify multiple payments in parallel (batch operation)
    ///
    /// Processes multiple payment verifications concurrently for better performance.
    /// Returns a vector of verification results in the same order as inputs.
    pub async fn verify_payments_batch(
        &self,
        payments: &[(&str, &str)], // (invoice, payment_id)
    ) -> Result<Vec<bool>, LightningError> {
        if payments.is_empty() {
            return Ok(Vec::new());
        }

        // Parse all invoices first (sequential, but fast)
        let invoice_data: Result<Vec<_>, _> = payments
            .iter()
            .map(|(invoice, _)| self.parse_invoice(invoice))
            .collect();
        let invoice_data = invoice_data?;

        // Verify all payments in parallel via provider
        let futures: Vec<_> = invoice_data
            .iter()
            .zip(payments.iter())
            .map(|(invoice_data, (invoice, payment_id))| {
                let payment_hash = invoice_data.payment_hash();
                // Clone payment_hash to avoid lifetime issues in async closure
                let payment_hash_array = payment_hash;
                let provider = &self.provider;
                async move {
                    provider
                        .verify_payment(invoice, &payment_hash_array, payment_id)
                        .await
                }
            })
            .collect();

        // Wait for all verifications to complete
        let results = futures::future::join_all(futures).await;
        Ok(results
            .into_iter()
            .map(|r| r.map(|v| v.verified).unwrap_or(false))
            .collect())
    }

    /// Get the provider type
    pub fn provider_type(&self) -> ProviderType {
        self.provider.provider_type()
    }

    /// Create a Lightning invoice (for ModuleAPI)
    pub async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        expiry_seconds: u64,
    ) -> Result<String, LightningError> {
        if let Some(reason) =
            invoice_amount_rejection(amount_msats, self.min_payment_sats, self.max_payment_sats)
        {
            return Err(LightningError::ProcessorError(reason));
        }
        let invoice = self
            .provider
            .create_invoice(amount_msats, description, expiry_seconds)
            .await?;
        let payment_id = self
            .parse_invoice(&invoice)
            .map(|d| d.payment_hash_hex())
            .unwrap_or_else(|_| hex::encode(rand::random::<[u8; 16]>()));
        let amount_sats = amount_msats / 1000;
        let payload = EventPayload::PaymentRequestCreated {
            payment_id: payment_id.clone(),
            amount_sats,
            invoice: Some(invoice.clone()),
        };
        if let Err(e) = self
            .node_api
            .publish_event(EventType::PaymentRequestCreated, payload)
            .await
        {
            debug!("Failed to publish PaymentRequestCreated: {}", e);
        }
        Ok(invoice)
    }

    /// Verify a Lightning payment (for ModuleAPI)
    pub async fn verify_payment_api(
        &self,
        invoice: &str,
        payment_hash_hex: &str,
        payment_id: &str,
    ) -> Result<crate::provider::PaymentVerificationResult, LightningError> {
        let hash_bytes = hex::decode(payment_hash_hex).map_err(|e| {
            LightningError::ProcessorError(format!("Invalid payment_hash hex: {e}"))
        })?;
        if hash_bytes.len() != 32 {
            return Err(LightningError::ProcessorError(
                "payment_hash must be 32 bytes (64 hex chars)".to_string(),
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&hash_bytes);
        self.provider
            .verify_payment(invoice, &arr, payment_id)
            .await
    }

    /// Get wallet balance in sats (for ModuleAPI)
    pub async fn get_balance(&self) -> Result<Option<u64>, LightningError> {
        self.provider.get_balance().await
    }

    /// Pay a Lightning invoice. `Ok` means the provider queued the send.
    /// `PaymentRouteFound` arrives later, from the path that succeeded.
    /// `PaymentVerified` arrives from `PaymentSent` or `PaymentClaimed`.
    pub async fn pay_invoice(
        &self,
        invoice: &str,
        node_api: &dyn NodeAPI,
    ) -> Result<(), LightningError> {
        let payment_id = self
            .parse_invoice(invoice)
            .map(|d| d.payment_hash_hex())
            .unwrap_or_else(|_| hex::encode(rand::random::<[u8; 16]>()));

        match self.provider.pay_invoice(invoice).await {
            Ok(_result) => Ok(()),
            Err(e) => {
                let reason = e.to_string();
                let is_route_error = matches!(e, LightningError::RoutingError(_));
                let payload = EventPayload::PaymentRouteFailed {
                    payment_id: payment_id.clone(),
                    reason: reason.clone(),
                };
                if is_route_error {
                    if let Err(pub_e) = node_api
                        .publish_event(EventType::PaymentRouteFailed, payload)
                        .await
                    {
                        debug!("Failed to publish PaymentRouteFailed: {}", pub_e);
                    }
                } else {
                    let payload = EventPayload::PaymentFailed { payment_id, reason };
                    if let Err(pub_e) = node_api
                        .publish_event(EventType::PaymentFailed, payload)
                        .await
                    {
                        debug!("Failed to publish PaymentFailed: {}", pub_e);
                    }
                }
                Err(e)
            }
        }
    }

    /// Close a Lightning channel. Emits ChannelClosed on success.
    pub async fn close_channel(
        &self,
        channel_id: &str,
        node_api: &dyn NodeAPI,
    ) -> Result<(), LightningError> {
        self.provider.close_channel(channel_id).await?;
        let payload = EventPayload::ChannelClosed {
            channel_id: channel_id.to_string(),
            reason: "user_requested".to_string(),
        };
        if let Err(e) = node_api
            .publish_event(EventType::ChannelClosed, payload)
            .await
        {
            debug!("Failed to publish ChannelClosed: {}", e);
        }
        Ok(())
    }

    /// List Lightning channels (delegates to provider).
    pub async fn list_channels(&self) -> Result<Vec<crate::provider::ChannelInfo>, LightningError> {
        self.provider.list_channels().await
    }

    /// Deposit address for the LDK on-chain key. `None` for providers that do not hold one.
    pub async fn deposit_address(&self) -> Result<Option<String>, LightningError> {
        self.provider.deposit_address().await
    }

    /// Open an outbound channel. `outpoints` is `txid:vout` comma-separated.
    pub async fn open_channel(
        &self,
        node_api: &dyn NodeAPI,
        peer_pubkey: &str,
        value_sats: u64,
        outpoints: &str,
    ) -> Result<String, LightningError> {
        self.provider
            .open_channel(node_api, peer_pubkey, value_sats, outpoints)
            .await
    }
}

#[cfg(test)]
mod invoice_bound_tests {
    use super::invoice_amount_rejection;
    use crate::provider::LightningProvider;
    use crate::provider::stub::StubProvider;

    #[tokio::test]
    async fn omitted_min_payment_still_creates_an_invoice() {
        assert!(invoice_amount_rejection(1_000, None, None).is_none());
        let invoice = StubProvider::new()
            .create_invoice(1_000, "test", 3600)
            .await
            .expect("invoice");
        assert!(invoice.starts_with("lnbc"));
    }

    #[test]
    fn set_minimum_rejects_a_smaller_amount() {
        assert!(invoice_amount_rejection(500, Some(1), None).is_some());
    }
}

#[cfg(test)]
mod settled_state_tests {
    use super::settled_from_payment_state;
    use blvm_node::module::ipc::protocol::EventPayload;

    #[test]
    fn a_missing_payment_module_skips_payment_settled() {
        assert!(settled_from_payment_state("pay-1", None).is_none());
    }

    #[test]
    fn an_unconfirmed_payment_skips_payment_settled() {
        let reply = br#"{"confirmation_count":0,"tx_hash":"11"}"#;
        assert!(settled_from_payment_state("pay-1", Some(reply)).is_none());
    }

    #[test]
    fn a_confirmed_payment_publishes_payment_settled() {
        let tx_hash = "ab".repeat(32);
        let reply = format!(r#"{{"confirmation_count":6,"tx_hash":"{tx_hash}"}}"#);
        match settled_from_payment_state("pay-1", Some(reply.as_bytes())) {
            Some(EventPayload::PaymentSettled {
                payment_id,
                confirmations,
                tx_hash: hash,
            }) => {
                assert_eq!(payment_id, "pay-1");
                assert_eq!(confirmations, 6);
                assert_eq!(hash, [0xab; 32]);
            }
            other => panic!("expected PaymentSettled, got {other:?}"),
        }
    }
}
