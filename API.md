# bllvm-lightning API Documentation

## Overview

The `bllvm-lightning` module provides Lightning Network payment processing with support for multiple providers (LNBits, LDK, Stub).

## Core Components

### `processor`

Main Lightning payment processor.

#### `LightningProcessor`

Processes Lightning payments and manages provider.

**Methods:**

- `new(ctx: &ModuleContext, node_api: Arc<dyn NodeAPI>) -> Result<Self, LightningError>`
  - Creates a new Lightning processor
  - Initializes provider based on configuration (`lightning.provider`)

- `handle_event(event: &ModuleMessage, node_api: &dyn NodeAPI) -> Result<(), LightningError>`
  - Handles payment events:
    - `PaymentRequestCreated` - Processes new payment request
    - `PaymentSettled` - Payment confirmed
    - `PaymentFailed` - Payment failed

- `process_payment(invoice_str: &str, payment_id: &str) -> Result<(), LightningError>`
  - Processes a Lightning payment:
    - Parses invoice
    - Verifies payment via provider
    - Updates payment state

### `provider`

Lightning provider abstraction supporting multiple backends.

#### `LightningProvider` Trait

Trait implemented by all Lightning providers.

**Methods:**

- `verify_payment(invoice: &str, payment_hash: &[u8; 32], payment_id: &str) -> Result<PaymentVerificationResult, LightningError>`
  - Verifies a Lightning payment
  - Returns verification result with amount and status

- `create_invoice(amount_msats: u64, description: &str, expiry_seconds: u64) -> Result<String, LightningError>`
  - Creates a BOLT11 Lightning invoice
  - Returns invoice string

- `is_payment_confirmed(payment_hash: &[u8; 32]) -> Result<bool, LightningError>`
  - Checks if a payment is confirmed
  - Returns true if payment is confirmed

- `provider_type() -> ProviderType`
  - Returns the provider type (LNBits, LDK, or Stub)

#### Provider Types

**LNBits Provider**
- REST API-based Lightning wallet
- Configuration: `lightning.lnbits.api_url`, `lightning.lnbits.api_key`

**LDK Provider**
- Rust-native Lightning implementation (bare minimum)
- Configuration: `lightning.ldk.data_dir`, `lightning.ldk.network`, `lightning.ldk.node_private_key`

**Stub Provider**
- Mock implementation for testing
- Always succeeds verification

#### `create_provider(provider_type: ProviderType, ctx: &ModuleContext) -> Result<Box<dyn LightningProvider>, LightningError>`

Factory function to create a provider from configuration.

## Events

### Subscribed Events
- `PaymentRequestCreated` - New payment request
- `PaymentSettled` - Payment confirmed on-chain
- `PaymentFailed` - Payment failed

### Published Events
- `PaymentVerified` - Lightning payment verified
- `PaymentRouteFound` - Payment route discovered
- `PaymentRouteFailed` - Payment routing failed
- `ChannelOpened` - Lightning channel opened
- `ChannelClosed` - Lightning channel closed

## Configuration

### LNBits Provider

```toml
[lightning]
provider = "lnbits"

[lightning.lnbits]
api_url = "https://lnbits.example.com"
api_key = "your_lnbits_api_key"
wallet_id = "optional_wallet_id"
```

### LDK Provider

```toml
[lightning]
provider = "ldk"

[lightning.ldk]
data_dir = "data/ldk"
network = "testnet"  # "mainnet", "testnet", "regtest", "signet"
node_private_key = "hex_encoded_private_key"  # Optional
```

### Stub Provider

```toml
[lightning]
provider = "stub"
```

## Error Handling

All methods return `Result<T, LightningError>` where `LightningError` can be:
- `ConfigError(String)` - Configuration error
- `InvoiceError(String)` - Invoice parsing/validation error
- `ProcessorError(String)` - Payment processing error
- `NodeConnectionError(String)` - Connection to Lightning node failed

## Examples

### Creating an Invoice

```rust
let processor = LightningProcessor::new(&ctx, node_api).await?;
// Invoice creation happens via provider
let provider = create_provider(ProviderType::LNBits, &ctx)?;
let invoice = provider.create_invoice(1000, "test payment", 3600).await?;
```

### Verifying a Payment

```rust
let provider = create_provider(ProviderType::LDK, &ctx)?;
let payment_hash = [0u8; 32]; // Actual payment hash
let result = provider.verify_payment(&invoice, &payment_hash, "payment_id").await?;
if result.verified {
    println!("Payment verified: {} msats", result.amount_msats.unwrap_or(0));
}
```

### Switching Providers

Simply change the configuration:

```toml
# Switch from LNBits to LDK
[lightning]
provider = "ldk"  # Just change this!
```

The module automatically uses the new provider on next initialization.

