# blvm-lightning

Lightning Network payment processor module for blvm-node.

## Overview

This module provides Lightning Network payment processing capabilities for blvm-node. It supports **multiple Lightning providers**:
- **LNBits** (REST API) - Simple, wallet/accounting built-in
- **LDK** (Lightning Development Kit) - Rust-native, full control
- **Stub** (for testing) - Mock implementation

## Installation

```bash
# Install via cargo
cargo install blvm-lightning

# Or install via cargo-blvm-module
cargo install cargo-blvm-module
cargo blvm-module install blvm-lightning
```

## Configuration

Create a `config.toml` in the module directory:

### LNBits Provider (Recommended)

```toml
[lightning]
provider = "lnbits"  # or "ldk" or "stub"

[lightning.lnbits]
api_url = "https://lnbits.example.com"
api_key = "your_lnbits_api_key"
wallet_id = "optional_wallet_id"  # Optional, for specific wallet
```

### LDK Provider

```toml
[lightning]
provider = "ldk"

[lightning.ldk]
data_dir = "data/ldk"
network = "testnet"  # or "mainnet" or "regtest"
node_private_key = "hex_encoded_private_key"  # Optional, will generate if not provided
```

### Stub Provider (Testing)

```toml
[lightning]
provider = "stub"
```

## Module Manifest

The module includes a `module.toml` manifest:

```toml
name = "blvm-lightning"
version = "0.1.0"
description = "Lightning Network payment processor"
author = "Bitcoin Commons Team"
entry_point = "blvm-lightning"

capabilities = [
    "read_blockchain",
    "subscribe_events",
]
```

## Dependencies

### Version Notes

This module uses `bitcoin_hashes = "0.3"` to match `lightning-invoice 0.2` requirements. This version differs from other BLVM crates (which use `0.11.0` or `0.12.x`) but is isolated to this module only and does not affect other components.

**Isolation:** The version conflict is managed by:
- Keeping `bitcoin_hashes` types internal to this module
- Using type conversion when interfacing with other BLVM modules
- Not exposing `bitcoin_hashes` types in the public API

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

## Provider Comparison

| Feature | LNBits | LDK | Stub |
|---------|--------|-----|------|
| **Status** | ✅ Production-ready | ✅ Fully implemented | ✅ Testing |
| **API Type** | REST (HTTP) | Rust-native (lightning-invoice) | None |
| **Real Lightning** | ✅ Yes | ✅ Yes | ❌ No |
| **External Service** | ✅ Yes | ❌ No | ❌ No |
| **Invoice Creation** | ✅ Via API | ✅ Native | ✅ Mock |
| **Payment Verification** | ✅ Via API | ✅ Native | ✅ Mock |
| **Best For** | Payment processing | Full control, Rust-native | Testing |

## Usage

The module automatically selects the provider based on configuration. All providers implement the same interface, so switching providers is just a config change:

```toml
# Switch from LNBits to LDK
[lightning]
provider = "ldk"  # Just change this!
```

## License

MIT License - see LICENSE file for details.
