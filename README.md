# blvm-lightning

Lightning Network payment processor module for blvm-node.

## Overview

This module provides Lightning Network payment processing capabilities for blvm-node. It supports **multiple Lightning providers**:

- **LNBits** (REST API) — simple wallet/accounting integration
- **LDK** (Lightning Development Kit 0.0.125) — a `ChannelManager` in this module process. It follows the node chain, listens for peers when configured, and tracks claims.
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
network = "testnet"  # mainnet, testnet, signet, or regtest
node_private_key = "hex_encoded_32_byte_seed"  # optional; generated and stored when unset
listen_port = 9735   # optional; unset means do not listen
peers = ["02pubkeyhex@127.0.0.1:9735"]  # optional outbound peers
```

`node_private_key` is the LDK seed. LDK derives the node id from it. It is not the node secret itself. The seed is written to `<data_dir>/ldk/seed` (mode `0600` on Unix). A configured seed that differs from the stored seed refuses to start.

The provider builds a `ChannelManager` and a `ChainMonitor`. `create_invoice` registers the payment with that manager, adds a route hint for each usable private channel, and signs the BOLT11 invoice with the node key. Received HTLCs are forwarded when LDK reports them ready, and a claimable invoice payment is claimed. `verify_payment` stays false until LDK emits `PaymentClaimable` or `PaymentClaimed`. A parsed invoice is not treated as a payment.

When the module starts, a manager that is still at genesis and has no channels is anchored at the node tip. A manager that already has a tip, or that has channels, is advanced one block at a time from `NewBlock`. A missing block stops the sync. `BlockDisconnected` is applied only when it is the LDK tip.

Transactions the manager asks to broadcast are submitted to the node mempool and appended to `<data_dir>/ldk/broadcasts.log`. If the node does not accept a transaction, it stays queued.

`listen_port` binds `0.0.0.0` for inbound peers. `peers` entries are `node_pubkey@host:port`. `max_inbound_sats` is the largest inbound channel. When it is unset, inbound opens are rejected. `pay_invoice` succeeds when LDK queues the send. `PaymentRouteFound` is published from the path LDK used. `PaymentVerified` follows `PaymentSent` or `PaymentClaimed`, with the invoice amount. No route is an error.

The provider stores `ldk/channel_manager`, `ldk/network_graph`, `ldk/scorer`, `ldk/payments.json`, and `ldk/funding.json`.

`deposit-address` prints the P2WPKH address for the LDK seed's on-chain deposit key. `open-channel` spends `txid:vout` outputs that pay that address. The peer must already be connected. The reserved coins are written to `<data_dir>/ldk/funding.json`. The funding transaction is signed when the peer accepts, including after a restart, then submitted to the node mempool. Every input must be SegWit. Change returns to the deposit address. Outputs LDK reports as spendable, other than coins already at that address, are swept there.

LNBits remains the provider that pays through an external wallet.

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
    "read_chain_state",
    "read_utxo",
    "read_mempool",
    "send_transactions",
    "subscribe_events",
    "publish_events",
]
```

## Dependencies

`lightning` `0.0.125` and `lightning-invoice` `0.32` share `bitcoin` 0.32. The LDK crate is the upstream release. This module does not vendor a fork.

## Events

### Subscribed

- `PaymentRequestCreated`
- `PaymentSettled`
- `PaymentFailed`
- `NewBlock`
- `BlockDisconnected`

### Published

- `PaymentRequestCreated`
- `PaymentVerified`
- `PaymentSettled`
- `PaymentFailed`
- `PaymentRouteFound` / `PaymentRouteFailed`
- `ChannelClosed`
- `ChannelOpened` when LDK reports the channel ready

## Provider Comparison

| Feature | LNBits | LDK | Stub |
|---------|--------|-----|------|
| **API Type** | REST (HTTP) | LDK `ChannelManager` | None |
| **Pays and receives on the network** | Yes | When a peer and a route exist. The chain source is the node. | No |
| **External Service** | Yes | No | No |
| **Best For** | Payment processing | A channel manager inside the module | Testing |

Switch providers by changing `provider` and the matching `[lnbits]` / `[ldk]` table — no code changes.

## License

MIT License — see LICENSE file for details.
