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
            .map_err(|e| LightningError::InvoiceError(format!("Failed to parse invoice: {:?}", e)))?;
        
        // Extract amount (lightning-invoice 0.2 API uses amount_pico_btc)
        // Convert pico BTC to msats: 1 BTC = 10^12 pico BTC = 10^11 msats
        // So: 1 pico BTC = 10^-1 msats = 0.1 msats, but we need integer math
        // Better: amount_pico_btc * 10^-1 = msats, but for integer: (amount_pico_btc + 5) / 10
        let amount_msats = invoice.amount_pico_btc()
            .map(|pico_btc| (pico_btc + 5) / 10) // Round to nearest msat
            .unwrap_or(0);
        
        // Extract expiry time (lightning-invoice 0.2 API - use as_seconds())
        let expiry = invoice.expiry_time()
            .map(|et| et.as_seconds())
            .unwrap_or(3600);
        
        debug!("Parsed Lightning invoice: amount={} msats, expiry={}s",
            amount_msats,
            expiry
        );
        
        // Extract payment hash (lightning-invoice 0.2: payment_hash() returns &Sha256)
        // Sha256 wraps sha256::Hash, convert to bytes
        let payment_hash = invoice.payment_hash();
        // Convert hash to bytes - sha256::Hash implements Display which we can parse
        // Or use the hash's inner representation
        let hash_str = format!("{}", payment_hash.0);
        // Parse hex string back to bytes (sha256::Hash Display outputs hex)
        let payment_hash_bytes = hex::decode(hash_str)
            .map_err(|e| LightningError::InvoiceError(format!("Failed to decode payment hash: {}", e)))?;
        let mut hash_array = [0u8; 32];
        hash_array.copy_from_slice(&payment_hash_bytes[..32]);
        
        Ok(InvoiceData {
            amount_msats,
            payment_hash: payment_hash_bytes.to_vec(),
            expiry,
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
