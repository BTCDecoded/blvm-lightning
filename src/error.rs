//! Error types for Lightning module

use thiserror::Error;
use blvm_node::module::traits::ModuleError;

#[derive(Debug, Error)]
pub enum LightningError {
    #[error("Module error: {0}")]
    ModuleError(String),
    
    #[error("Invoice parsing error: {0}")]
    InvoiceParseError(String),
    
    #[error("Invoice error: {0}")]
    InvoiceError(String),
    
    #[error("Processor error: {0}")]
    ProcessorError(String),
    
    #[error("Payment verification failed: {0}")]
    PaymentVerificationFailed(String),
    
    #[error("Lightning node connection error: {0}")]
    NodeConnectionError(String),
    
    #[error("Payment routing failed: {0}")]
    RoutingError(String),
    
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

impl From<ModuleError> for LightningError {
    fn from(err: ModuleError) -> Self {
        LightningError::ModuleError(format!("{:?}", err))
    }
}

