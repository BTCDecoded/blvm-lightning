//! Lightning invoice handling (BOLT11)

use crate::error::LightningError;
use lightning_invoice::Invoice;
use tracing::debug;

/// Invoice parser for BOLT11 invoices
pub struct InvoiceParser;

impl InvoiceParser {
    /// Parse a BOLT11 Lightning invoice
    pub fn parse(invoice_str: &str) -> Result<InvoiceData, LightningError> {
        // Parse BOLT11 invoice using lightning-invoice crate
        let invoice: Invoice = invoice_str.parse()
            .map_err(|e| LightningError::InvoiceError(format!("Failed to parse invoice: {}", e)))?;
        
        debug!("Parsed Lightning invoice: amount={} msats, expiry={}s",
            invoice.amount_milli_satoshis().unwrap_or(0),
            invoice.expiry_time().as_secs()
        );
        
        // Extract payment hash
        let payment_hash = invoice.payment_hash().to_vec();
        
        Ok(InvoiceData {
            amount_msats: invoice.amount_milli_satoshis().unwrap_or(0),
            payment_hash,
            expiry: invoice.expiry_time().as_secs(),
            invoice: invoice.clone(),
        })
    }
    
    /// Verify invoice signature
    pub fn verify_signature(invoice: &Invoice) -> Result<bool, LightningError> {
        // lightning-invoice crate handles signature verification during parsing
        // If we got here, the signature is valid
        Ok(true)
    }
}

/// Parsed invoice data
pub struct InvoiceData {
    pub amount_msats: u64,
    pub payment_hash: Vec<u8>,
    pub expiry: u64,
    pub invoice: Invoice,
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
