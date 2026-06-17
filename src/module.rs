//! Lightning module: unified CLI via #[module] macro.

use blvm_node::module::ipc::protocol::{EventMessage, ModuleMessage};
use blvm_sdk::module::prelude::*;
use blvm_sdk_macros::module;
use std::sync::Arc;

use crate::processor::LightningProcessor;

/// Lightning module: processor + CLI in one struct.
#[derive(Clone)]
pub struct LightningModule {
    pub processor: Arc<LightningProcessor>,
    pub db: Option<Arc<dyn blvm_node::storage::database::Database>>,
}

#[module]
impl LightningModule {
    #[on_event(PaymentRequestCreated, PaymentSettled, PaymentFailed)]
    async fn on_payment_event(
        &self,
        event: &EventMessage,
        ctx: &InvocationContext,
    ) -> Result<(), ModuleError> {
        let msg = ModuleMessage::Event(event.clone());
        let api = ctx.node_api().expect("node_api required");
        self.processor
            .handle_event(&msg, api.as_ref())
            .await
            .map_err(|e| ModuleError::Other(e.to_string()))
    }

    /// Show lightning module status (provider type).
    #[command]
    fn status(&self, _ctx: &InvocationContext) -> Result<String, ModuleError> {
        let status = self.processor.get_status();
        Ok(format!(
            "Lightning module\n\
             Provider: {}",
            status.provider_type
        ))
    }

    /// List stored invoices (from PaymentRequestCreated events).
    #[command]
    fn list_invoices(&self, _ctx: &InvocationContext) -> Result<String, ModuleError> {
        let invoices = self
            .db
            .as_ref()
            .map(crate::invoice_store::load_invoices)
            .unwrap_or_default();
        let out = if invoices.is_empty() {
            "No invoices stored.\n\
             Invoices appear when PaymentRequestCreated events are received."
                .into()
        } else {
            let mut s = format!("Invoices ({}):\n", invoices.len());
            for (i, inv) in invoices.iter().enumerate() {
                s.push_str(&format!(
                    "  {}. {} | {}...\n",
                    i + 1,
                    inv.payment_id,
                    inv.invoice.chars().take(40).collect::<String>()
                ));
            }
            s
        };
        Ok(out)
    }

    /// Create a Lightning invoice (amount in msats).
    #[command]
    fn create_invoice(
        &self,
        _ctx: &InvocationContext,
        amount_msats: u64,
        description: Option<String>,
        expiry_seconds: Option<u64>,
    ) -> Result<String, ModuleError> {
        let processor = Arc::clone(&self.processor);
        let desc = description.unwrap_or_else(|| "CLI invoice".into());
        let expiry = expiry_seconds.unwrap_or(3600);
        run_async(async move {
            processor
                .create_invoice(amount_msats, &desc, expiry)
                .await
                .map(|inv| format!("Invoice:\n{inv}"))
                .map_err(|e| anyhow::anyhow!("Failed to create invoice: {}", e))
        })
    }

    /// Pay a Lightning invoice (outgoing payment).
    #[command]
    fn pay_invoice(&self, ctx: &InvocationContext, invoice: String) -> Result<String, ModuleError> {
        let inv = invoice.trim();
        if inv.is_empty() {
            return Err(ModuleError::Other("Usage: pay-invoice <invoice>".into()));
        }
        let node_api = ctx.node_api().ok_or_else(|| {
            ModuleError::Other("Node not connected (pay-invoice requires node API)".into())
        })?;
        let processor = Arc::clone(&self.processor);
        run_async(async move {
            processor
                .pay_invoice(inv, node_api.as_ref())
                .await
                .map(|_| {
                    "Payment initiated. Check events for PaymentVerified/PaymentFailed.".into()
                })
                .map_err(|e| anyhow::anyhow!("{}", e))
        })
    }

    /// List Lightning channels (capacity, state). Requires provider support.
    #[command]
    fn list_channels(&self, _ctx: &InvocationContext) -> Result<String, ModuleError> {
        let processor = Arc::clone(&self.processor);
        run_async(async move {
            let channels = processor.list_channels().await.unwrap_or_default();
            if channels.is_empty() {
                Ok::<_, String>(
                    "No channels. LDK provider would list channels when connected.".into(),
                )
            } else {
                let mut out = format!("Channels ({}):\n", channels.len());
                for (i, ch) in channels.iter().enumerate() {
                    let peer_hex = if ch.peer_pubkey.is_empty() {
                        "n/a".into()
                    } else {
                        format!(
                            "{}...",
                            hex::encode(&ch.peer_pubkey[..ch.peer_pubkey.len().min(8)])
                        )
                    };
                    out.push_str(&format!(
                        "  {}. {} | {} sats | {} | peer={}\n",
                        i + 1,
                        ch.channel_id,
                        ch.capacity_sats,
                        ch.state,
                        peer_hex
                    ));
                }
                Ok::<_, String>(out)
            }
        })
    }

    /// Close a Lightning channel. Emits ChannelClosed on success.
    #[command]
    fn close_channel(
        &self,
        ctx: &InvocationContext,
        channel_id: String,
    ) -> Result<String, ModuleError> {
        let cid = channel_id.trim();
        if cid.is_empty() {
            return Err(ModuleError::Other(
                "Usage: close-channel <channel_id>".into(),
            ));
        }
        let node_api = ctx.node_api().ok_or_else(|| {
            ModuleError::Other("Node not connected (close-channel requires node API)".into())
        })?;
        let processor = Arc::clone(&self.processor);
        run_async(async move {
            processor
                .close_channel(cid, node_api.as_ref())
                .await
                .map(|_| format!("Channel {cid} closed."))
                .map_err(|e| anyhow::anyhow!("{}", e))
        })
    }
}
