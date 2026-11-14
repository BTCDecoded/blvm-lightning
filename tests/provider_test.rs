//! Unit tests for Lightning providers

use blvm_lightning::provider::{ProviderType, create_provider};
use blvm_node::module::traits::ModuleContext;
use std::collections::HashMap;

#[tokio::test]
async fn test_stub_provider() {
    let mut config = HashMap::new();
    config.insert("lightning.provider".to_string(), "stub".to_string());

    let ctx = ModuleContext {
        module_id: "test".to_string(),
        config,
        data_dir: "/tmp".to_string(),
        socket_path: "/tmp/test.sock".to_string(),
    };

    let provider = create_provider(ProviderType::Stub, &ctx).unwrap();
    assert_eq!(provider.provider_type(), ProviderType::Stub);

    // Test invoice creation
    let invoice = provider.create_invoice(1000, "test", 3600).await.unwrap();
    assert!(invoice.starts_with("lnbc"));

    let payment_hash = [0u8; 32];
    let result = provider
        .verify_payment(&invoice, &payment_hash, "test_id")
        .await
        .unwrap();
    assert!(!result.verified);
    assert!(result.amount_msats.is_none());

    let confirmed = provider.is_payment_confirmed(&payment_hash).await.unwrap();
    assert!(!confirmed);
}

#[tokio::test]
async fn test_ldk_provider_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let data_dir = temp_dir.path().to_string_lossy().to_string();

    let mut config = HashMap::new();
    config.insert("lightning.provider".to_string(), "ldk".to_string());
    config.insert("lightning.ldk.network".to_string(), "regtest".to_string());

    let ctx = ModuleContext {
        module_id: "test".to_string(),
        config,
        data_dir: data_dir.clone(),
        socket_path: "/tmp/test.sock".to_string(),
    };

    let provider = create_provider(ProviderType::LDK, &ctx)
        .unwrap_or_else(|e| panic!("LDK provider creation failed: {e}"));
    assert_eq!(provider.provider_type(), ProviderType::LDK);

    let invoice = provider
        .create_invoice(1000, "test", 3600)
        .await
        .expect("create_invoice");
    assert!(
        invoice.starts_with("lnbcrt"),
        "regtest invoice prefix, got {invoice}"
    );

    let parsed = blvm_lightning::invoice::InvoiceParser::parse(&invoice).expect("parse invoice");
    let payment_hash = parsed.payment_hash();
    let unpaid = provider
        .verify_payment(&invoice, &payment_hash, "test_id")
        .await
        .expect("verify");
    assert!(!unpaid.verified, "a newly created invoice is not a payment");

    let confirmed = provider
        .is_payment_confirmed(&payment_hash)
        .await
        .expect("confirm");
    assert!(!confirmed);
    assert!(provider.list_channels().await.expect("channels").is_empty());
    assert_eq!(provider.get_balance().await.expect("balance"), Some(0));
    assert!(provider.pay_invoice(&invoice).await.is_err());

    let reopened = create_provider(ProviderType::LDK, &ctx)
        .unwrap_or_else(|e| panic!("LDK reload failed: {e}"));
    let again = reopened
        .create_invoice(2000, "reload", 3600)
        .await
        .expect("invoice after reload");
    let again_parsed =
        blvm_lightning::invoice::InvoiceParser::parse(&again).expect("parse reloaded invoice");
    assert_eq!(
        parsed.invoice.recover_payee_pub_key(),
        again_parsed.invoice.recover_payee_pub_key(),
        "reloading the data directory keeps the LDK node id"
    );
}
