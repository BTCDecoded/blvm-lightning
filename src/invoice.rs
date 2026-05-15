//! Lightning invoice handling (BOLT11)

use crate::error::LightningError;
use bitcoin::hashes::Hash;
use lightning_invoice::Bolt11Invoice;
use tracing::debug;

/// Invoice parser for BOLT11 invoices
pub struct InvoiceParser;

impl InvoiceParser {
    /// Parse a BOLT11 Lightning invoice
    pub fn parse(invoice_str: &str) -> Result<InvoiceData, LightningError> {
        // Parse BOLT11 invoice (`lightning-invoice` 0.32).
        let invoice: Bolt11Invoice = invoice_str.parse().map_err(|e| {
            LightningError::InvoiceError(format!("Failed to parse invoice: {:?}", e))
        })?;

        let amount_msats = invoice.amount_milli_satoshis().unwrap_or(0);

        // Absolute UNIX expiry: creation time + relative expiry (BOLT11 default if tag missing).
        let expiry = invoice
            .duration_since_epoch()
            .as_secs()
            .saturating_add(invoice.expiry_time().as_secs());

        debug!(
            "Parsed Lightning invoice: amount={} msats, expiry={}s",
            amount_msats, expiry
        );

        let payment_hash_bytes = invoice.payment_hash().to_byte_array().to_vec();

        Ok(InvoiceData {
            amount_msats,
            payment_hash: payment_hash_bytes,
            expiry,
            invoice: invoice.clone(),
        })
    }

    /// Verify invoice signature
    pub fn verify_signature(invoice: &Bolt11Invoice) -> Result<bool, LightningError> {
        invoice.check_signature().map(|()| true).map_err(|e| {
            LightningError::InvoiceError(format!("Invalid invoice signature: {:?}", e))
        })
    }
}

/// Parsed invoice data
pub struct InvoiceData {
    pub amount_msats: u64,
    pub payment_hash: Vec<u8>,
    pub expiry: u64,
    pub invoice: Bolt11Invoice,
}

impl InvoiceData {
    /// Check if invoice is expired
    pub fn is_expired(&self) -> bool {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now > self.expiry
    }

    /// Get payment hash as hex string
    pub fn payment_hash_hex(&self) -> String {
        hex::encode(&self.payment_hash)
    }

    /// Get payment hash as [u8; 32] array
    pub fn payment_hash(&self) -> [u8; 32] {
        let mut hash = [0u8; 32];
        let len = self.payment_hash.len().min(32);
        hash[..len].copy_from_slice(&self.payment_hash[..len]);
        hash
    }
}
