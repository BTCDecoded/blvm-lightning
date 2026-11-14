//! Lightning module configuration.
//!
//! Loaded from config.toml in module data dir. Node overrides via [modules.blvm-lightning]
//! and MODULE_CONFIG_* env vars.

use blvm_sdk_macros::config;
use serde::{Deserialize, Serialize};

/// LNBits provider configuration.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LnbitsConfig {
    #[serde(default)]
    pub api_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub wallet_id: Option<String>,
}

/// LDK provider configuration.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LdkConfig {
    #[serde(default = "default_network")]
    pub network: String,
    /// Hex-encoded 32-byte LDK seed. LDK derives the node key from this seed.
    /// When unset, the module generates a seed and stores it under the data directory.
    #[serde(default)]
    pub node_private_key: Option<String>,
    /// TCP port for inbound Lightning peers. Unset means the module does not listen.
    #[serde(default)]
    pub listen_port: Option<u16>,
    /// Outbound peers, each `node_pubkey@host:port`.
    #[serde(default)]
    pub peers: Vec<String>,
    /// Largest inbound channel, in satoshis. Absent rejects every inbound open.
    #[serde(default)]
    pub max_inbound_sats: Option<u64>,
}

fn default_network() -> String {
    "testnet".to_string()
}

/// Lightning module configuration.
///
/// Config file: `config.toml` in module data dir with [lightning] and [lightning.lnbits]/[lightning.ldk] sections.
/// Node override: `[modules.blvm-lightning]` in node config.
#[config(name = "lightning")]
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LightningConfig {
    /// Provider type: "lnbits", "ldk", or "stub".
    /// An omitted key is LNBits. The stub does not report payments.
    #[serde(default = "default_provider")]
    #[config_env]
    pub provider: String,

    #[serde(default)]
    pub lnbits: LnbitsConfig,

    #[serde(default)]
    pub ldk: LdkConfig,

    /// Min payment in sats. Absent means `create_invoice` has no lower bound.
    #[serde(default)]
    #[config_env]
    pub min_payment_sats: Option<u64>,
    /// Max payment in sats. Absent means `create_invoice` has no upper bound.
    #[serde(default)]
    #[config_env]
    pub max_payment_sats: Option<u64>,
}

fn default_provider() -> String {
    "lnbits".to_string()
}

blvm_sdk::impl_module_config!(LightningConfig);

impl LightningConfig {
    /// Convert to ModuleContext config map for provider compatibility.
    pub fn to_context_map(&self) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("lightning.provider".to_string(), self.provider.clone());
        if !self.lnbits.api_url.is_empty() {
            m.insert(
                "lightning.lnbits.api_url".to_string(),
                self.lnbits.api_url.clone(),
            );
        }
        if !self.lnbits.api_key.is_empty() {
            m.insert(
                "lightning.lnbits.api_key".to_string(),
                self.lnbits.api_key.clone(),
            );
        }
        if let Some(ref w) = self.lnbits.wallet_id {
            m.insert("lightning.lnbits.wallet_id".to_string(), w.clone());
        }
        if !self.ldk.network.is_empty() {
            m.insert(
                "lightning.ldk.network".to_string(),
                self.ldk.network.clone(),
            );
        }
        if let Some(ref k) = self.ldk.node_private_key {
            m.insert("lightning.ldk.node_private_key".to_string(), k.clone());
        }
        if let Some(port) = self.ldk.listen_port {
            m.insert("lightning.ldk.listen_port".to_string(), port.to_string());
        }
        if !self.ldk.peers.is_empty() {
            m.insert("lightning.ldk.peers".to_string(), self.ldk.peers.join(","));
        }
        if let Some(max) = self.ldk.max_inbound_sats {
            m.insert(
                "lightning.ldk.max_inbound_sats".to_string(),
                max.to_string(),
            );
        }
        if let Some(min) = self.min_payment_sats {
            m.insert("lightning.min_payment_sats".to_string(), min.to_string());
        }
        if let Some(max) = self.max_payment_sats {
            m.insert("lightning.max_payment_sats".to_string(), max.to_string());
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::LightningConfig;

    #[test]
    fn an_omitted_provider_is_lnbits() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "min_payment_sats = 1\n").expect("write config");
        let config = LightningConfig::load(&path).expect("load");
        assert_eq!(config.provider, "lnbits");
    }
}
