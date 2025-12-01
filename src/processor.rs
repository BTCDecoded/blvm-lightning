//! Lightning payment processor

use crate::provider::{ProviderType, LightningProvider, create_provider};
use crate::error::LightningError;
use crate::invoice::{InvoiceData, InvoiceParser};
use bllvm_node::module::ipc::protocol::ModuleMessage;
use bllvm_node::module::traits::{EventPayload, EventType, NodeAPI};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Lightning payment processor
pub struct LightningProcessor {
    /// Lightning provider (LNBits, LDK, or Stub)
    provider: Box<dyn LightningProvider>,
    /// Node API for storage and queries
    node_api: Arc<dyn NodeAPI>,
}

impl LightningProcessor {
    /// Create a new Lightning processor
    pub async fn new(
        ctx: &bllvm_node::module::traits::ModuleContext,
        node_api: Arc<dyn NodeAPI>,
    ) -> Result<Self, LightningError> {
        // Determine provider type from config
        let provider_type_str = ctx.get_config_or("lightning.provider", "lnbits");
        let provider_type = ProviderType::from_str(&provider_type_str)
            .map_err(|e| LightningError::ConfigError(format!("Invalid provider type: {}", e)))?;
        
        info!("Initializing Lightning processor with provider: {:?}", provider_type);
        
        // Create provider
        let provider = create_provider(provider_type, ctx)?;
        
        // Store provider info in module storage
        let tree_id = node_api.storage_open_tree("lightning_config".to_string()).await
            .map_err(|e| LightningError::ProcessorError(format!("Failed to open storage tree: {}", e)))?;
        
        // Store provider type
        let provider_type_str = match provider.provider_type() {
            ProviderType::LNBits => "lnbits",
            ProviderType::LDK => "ldk",
            ProviderType::Stub => "stub",
        };
        node_api.storage_insert(tree_id.clone(), b"provider_type".to_vec(), provider_type_str.as_bytes().to_vec()).await
            .map_err(|e| LightningError::ProcessorError(format!("Failed to store provider_type: {}", e)))?;
        
        // Initialize channel stats (will be updated as channels are opened/closed)
        node_api.storage_insert(tree_id.clone(), b"channel_count".to_vec(), 0u64.to_be_bytes().to_vec()).await
            .map_err(|e| LightningError::ProcessorError(format!("Failed to store channel_count: {}", e)))?;
        
        node_api.storage_insert(tree_id, b"total_capacity_sats".to_vec(), 0u64.to_be_bytes().to_vec()).await
            .map_err(|e| LightningError::ProcessorError(format!("Failed to store total_capacity_sats: {}", e)))?;
        
        Ok(Self {
            provider,
            node_api,
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
                        if let EventPayload::PaymentRequestCreated { payment_id, invoice, .. } = &event_msg.payload {
                            debug!("Processing payment request: {}", payment_id);
                            if let Some(invoice_str) = invoice {
                                self.process_payment(invoice_str, payment_id, node_api).await?;
                            }
                        }
                    }
                    EventType::PaymentSettled => {
                        debug!("Payment settled event received");
                    }
                    EventType::PaymentFailed => {
                        debug!("Payment failed event received");
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
            return Err(LightningError::ProcessorError("Empty payment_id".to_string()));
        }
        
        // Early exit: Check if node_url is configured before HTTP call
        if self.node_url.is_none() {
            // Try to get from NodeAPI, but check first
            if node_api.get_lightning_node_url().await.is_err() {
                return Err(LightningError::ProcessorError("Lightning node URL not configured".to_string()));
            }
        }
        
        info!("Processing Lightning payment: {} for payment_id: {}", invoice, payment_id);
        
        // Parse invoice
        let invoice_data = self.parse_invoice(invoice)?;
        
        // Check if invoice is expired
        if invoice_data.is_expired() {
            warn!("Invoice expired for payment_id: {}", payment_id);
            return Err(LightningError::InvoiceError("Invoice expired".to_string()));
        }
        
        // Get payment hash from invoice
        let payment_hash = invoice_data.payment_hash();
        
        // Verify payment via provider
        let verification_result = self.provider.verify_payment(invoice, &payment_hash, payment_id).await?;
        
        if verification_result.verified {
            info!(
                "Lightning payment verified via {:?}: payment_id={}, amount={:?} msats",
                self.provider.provider_type(),
                payment_id,
                verification_result.amount_msats
            );
            
            // Check payment state via NodeAPI
            if let Ok(Some(state)) = node_api.get_payment_state(payment_id).await {
                debug!("Payment state for {}: {:?}", payment_id, state);
            }
        } else {
            warn!("Lightning payment verification failed: payment_id={}", payment_id);
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
        payments: &[(&str, &str)],  // (invoice, payment_id)
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
                self.provider.verify_payment(invoice, &payment_hash, payment_id)
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
}

