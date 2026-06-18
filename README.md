# blvm-lightning

Lightning Network payment processor module for blvm-node.

## Overview

This module provides Lightning Network payment processing capabilities for blvm-node. It supports **multiple Lightning providers**:

- **LNBits** (REST API) — simple wallet/accounting integration
- **LDK** (Lightning Development Kit) — Rust-native, full control
- **Stub** (for testing) — mock implementation (default)

## Installation

Pin in node `blvm.toml`:

```toml
[modules]
blvm-lightning = "0.1.*"
```

Or build from source and place the binary + `module.toml` under the module search path. See [blvm-docs — Lightning module](https://github.com/BTCDecoded/blvm-docs/blob/main/src/modules/lightning.md).

## Configuration

Create `config.toml` in `<modules.data_dir>/blvm-lightning/` with **flat top-level keys** (no `[lightning]` wrapper — invalid tables are **silently ignored** and the module falls back to **`stub`**):

### LNBits Provider (Recommended)

```toml
provider = "lnbits"

[lnbits]
api_url = "https://lnbits.example.com"
api_key = "your_lnbits_api_key"
wallet_id = "optional_wallet_id"  # Optional
```

### LDK Provider

```toml
provider = "ldk"

[ldk]
network = "testnet"  # or "mainnet" or "regtest"
node_private_key = "hex_encoded_private_key"  # Optional; generated when unset
```

### Stub Provider (Testing)

```toml
provider = "stub"
```

Node overrides: `[modules.blvm-lightning]` with the same flat keys (passed as `MODULE_CONFIG_*` on spawn).

## Module Manifest

See `module.toml` in this repo and **`registry/modules.json`** in the `blvm` release — do not hardcode semver in operator docs.

```toml
name = "blvm-lightning"
description = "Lightning Network payment processor"
author = "Bitcoin Commons Team"
entry_point = "blvm-lightning"

capabilities = [
    "read_blockchain",
    "subscribe_events",
]
```

## Dependencies

This module uses `bitcoin_hashes = "0.3"` to match `lightning-invoice 0.2` requirements. That version differs from other BLVM crates but is isolated to this module.

## Events

### Subscribed

- `PaymentRequestCreated`
- `PaymentSettled`
- `PaymentFailed`

### Published

- `PaymentRequestCreated`
- `PaymentVerified`
- `PaymentSettled`
- `PaymentFailed`
- `PaymentRouteFound` / `PaymentRouteFailed`
- `ChannelClosed`

`ChannelOpened` exists on the shared `EventType` enum but is **not emitted** by this module today.

## Provider Comparison

| Feature | LNBits | LDK | Stub |
|---------|--------|-----|------|
| **API Type** | REST (HTTP) | Rust-native (lightning-invoice) | None |
| **Real Lightning** | Yes | Yes | No (mock) |
| **External Service** | Yes | No | No |
| **Best For** | Payment processing | Full control, Rust-native | Testing |

Switch providers by changing `provider` and the matching `[lnbits]` / `[ldk]` table — no code changes.

## License

MIT License — see LICENSE file for details.
