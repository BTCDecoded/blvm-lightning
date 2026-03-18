//! Invoice storage for list-invoices CLI.
//! Stores payment_id -> invoice when PaymentRequestCreated is received.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

const INVOICES_TREE: &str = "invoices";
const KEY: &[u8] = b"lightning:invoices";

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct StoredInvoice {
    pub payment_id: String,
    pub invoice: String,
    pub created_at: u64,
}

/// Load invoices from module DB.
pub fn load_invoices(db: &Arc<dyn blvm_node::storage::database::Database>) -> Vec<StoredInvoice> {
    let tree = match db.open_tree(INVOICES_TREE) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let data = match tree.get(KEY) {
        Ok(Some(d)) => d,
        _ => return Vec::new(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

/// Store an invoice. Appends to existing list.
pub fn store_invoice(
    db: &Arc<dyn blvm_node::storage::database::Database>,
    payment_id: &str,
    invoice: &str,
) {
    let tree = match db.open_tree(INVOICES_TREE) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut invoices: Vec<StoredInvoice> = tree
        .get(KEY)
        .ok()
        .flatten()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    invoices.push(StoredInvoice {
        payment_id: payment_id.to_string(),
        invoice: invoice.to_string(),
        created_at,
    });
    if let Ok(data) = serde_json::to_vec(&invoices) {
        let _ = tree.insert(KEY, &data);
    }
}
