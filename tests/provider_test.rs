//! Unit tests for Lightning providers

use bllvm_lightning::provider::{create_provider, ProviderType, LightningProvider};
use bllvm_node::module::traits::ModuleContext;
use std::collections::HashMap;

#[tokio::test]
async fn test_stub_provider() {
    let mut config = HashMap::new();
    config.insert("lightning.provider".to_string(), "stub".to_string());
    
    let ctx = ModuleContext {
        module_id: "test".to_string(),
        config,
        data_dir: std::path::PathBuf::from("/tmp"),
        socket_path: "/tmp/test.sock".to_string(),
    };
    
    let provider = create_provider(ProviderType::Stub, &ctx).unwrap();
    assert_eq!(provider.provider_type(), ProviderType::Stub);
    
    // Test invoice creation
    let invoice = provider.create_invoice(1000, "test", 3600).await.unwrap();
    assert!(invoice.starts_with("lnbc"));
    
    // Test payment verification (stub always succeeds)
    let payment_hash = [0u8; 32];
    let result = provider.verify_payment(&invoice, &payment_hash, "test_id").await.unwrap();
    assert!(result.verified);
    
    // Test payment confirmation
    let confirmed = provider.is_payment_confirmed(&payment_hash).await.unwrap();
    assert!(confirmed);
}

#[tokio::test]
async fn test_ldk_provider_creation() {
    let mut config = HashMap::new();
    config.insert("lightning.provider".to_string(), "ldk".to_string());
    config.insert("lightning.ldk.data_dir".to_string(), "/tmp/ldk_test".to_string());
    config.insert("lightning.ldk.network".to_string(), "testnet".to_string());
    
    let ctx = ModuleContext {
        module_id: "test".to_string(),
        config,
        data_dir: std::path::PathBuf::from("/tmp"),
        socket_path: "/tmp/test.sock".to_string(),
    };
    
    let provider = create_provider(ProviderType::LDK, &ctx).unwrap();
    assert_eq!(provider.provider_type(), ProviderType::LDK);
    
    // Test invoice creation
    let invoice_result = provider.create_invoice(1000, "test", 3600).await;
    assert!(invoice_result.is_ok());
    
    // Test payment verification
    let payment_hash = [0u8; 32];
    let result = provider.verify_payment("lnbc1pstub", &payment_hash, "test_id").await;
    // May fail if invoice is invalid, but should not panic
    assert!(result.is_ok());
}

