# bllvm-lightning

Lightning Network payment processor module for bllvm-node.

## Overview

This module provides Lightning Network payment processing capabilities for bllvm-node. It supports **multiple Lightning providers**:
- **LNBits** (REST API) - Simple, wallet/accounting built-in
- **LDK** (Lightning Development Kit) - Rust-native, full control
- **Stub** (for testing) - Mock implementation

## Installation

```bash
# Install via cargo
cargo install bllvm-lightning

# Or install via cargo-bllvm-module
cargo install cargo-bllvm-module
cargo bllvm-module install bllvm-lightning
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
name = "bllvm-lightning"
version = "0.1.0"
description = "Lightning Network payment processor"
author = "Bitcoin Commons Team"
entry_point = "bllvm-lightning"

capabilities = [
    "read_blockchain",
    "subscribe_events",
]
```

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
| **Integration Effort** | 8-12h | 20-30h | 4-6h |
| **API Type** | REST (HTTP) | Rust API | None |
| **Real Lightning** | ✅ Yes | ✅ Yes | ❌ No |
| **External Service** | ✅ Yes | ❌ No | ❌ No |
| **Wallet Features** | ✅ Built-in | ❌ Manual | ❌ None |
| **Best For** | Payment processing | Full control | Testing |

## Usage

The module automatically selects the provider based on configuration. All providers implement the same interface, so switching providers is just a config change:

```toml
# Switch from LNBits to LDK
[lightning]
provider = "ldk"  # Just change this!
```

## License

MIT License - see LICENSE file for details.
