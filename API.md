# blvm-lightning API Documentation

## Overview

The `blvm-lightning` module provides Lightning Network payment processing with support for multiple providers (LNBits, LDK, Stub).

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
- Upstream LDK 0.0.125 `ChannelManager` and `ChainMonitor` in the module process
- `create_invoice` registers an inbound payment, adds a route hint for each usable private channel, and signs a BOLT11 invoice with the LDK node key. Received HTLCs are forwarded when LDK reports them ready, and a claimable invoice payment is claimed.
- `verify_payment` and `is_payment_confirmed` are true only after `PaymentClaimable` or `PaymentClaimed`
- Chain source is the node. A genesis manager with no channels is anchored at the node tip on startup. Otherwise each missing block is connected in order. Broadcasts go to the node mempool and to `ldk/broadcasts.log`.
- `lightning.ldk.listen_port` binds inbound peers. `lightning.ldk.peers` is a comma-separated list of `pubkey@host:port`.
- `pay_invoice` returns when LDK queues the send. It does not claim a route. `PaymentRouteFound` is published from `PaymentPathSuccessful`. `PaymentVerified` is published from `PaymentSent` or `PaymentClaimed` with the payment amount. No route is an error.
- `deposit_address` is the P2WPKH address for the LDK seed's on-chain key. `open_channel` spends `txid:vout` coins that pay it, after the peer accepts. Those coins are stored in `ldk/funding.json`. `ChannelOpened` is published when the channel is ready.
- Restart files under the data directory: `ldk/network_graph`, `ldk/scorer`, `ldk/payments.json`, and `ldk/funding.json`.
- `lightning.ldk.max_inbound_sats` caps an inbound channel. Absent rejects inbound opens. Zero-confirmation channels are not accepted.
- Configuration: `lightning.ldk.network` (`mainnet`, `testnet`, `signet`, `regtest`). `lightning.ldk.node_private_key` is an optional hex 32-byte LDK seed, not the node secret. The data directory comes from the module context.
- Manifest capabilities: `read_blockchain`, `read_chain_state`, `read_utxo`, `read_mempool`, `send_transactions`, `subscribe_events`, `publish_events`.

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
- `NewBlock` - Advance the LDK chain tip
- `BlockDisconnected` - Disconnect the LDK tip when it matches

### Published Events
- `PaymentVerified` - Lightning payment verified
- `PaymentRouteFound` - Payment route discovered
- `PaymentRouteFailed` - Payment routing failed
- `ChannelClosed` - Lightning channel closed
- `ChannelOpened` - LDK reported the channel ready

## Configuration

Use **flat top-level keys** in `config.toml` (no `[lightning]` wrapper — invalid tables are silently ignored; module falls back to **`stub`**).

### LNBits Provider

```toml
provider = "lnbits"

[lnbits]
api_url = "https://lnbits.example.com"
api_key = "your_lnbits_api_key"
wallet_id = "optional_wallet_id"
```

### LDK Provider

```toml
provider = "ldk"

[ldk]
network = "testnet"  # mainnet, testnet, signet, or regtest
node_private_key = "hex_encoded_32_byte_seed"  # optional LDK seed
```

### Stub Provider

```toml
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
provider = "ldk"  # was "lnbits"
```

The module automatically uses the new provider on next initialization.

