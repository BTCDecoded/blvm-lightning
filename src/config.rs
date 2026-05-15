//! Lightning module configuration.
//!
//! Loaded from config.toml in module data dir. Node overrides via [modules.lightning] and
//! MODULE_CONFIG_* env vars.

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
    #[serde(default)]
    pub node_private_key: Option<String>,
}

fn default_network() -> String {
    "testnet".to_string()
}

/// Lightning module configuration.
///
/// Config file: `config.toml` in module data dir with [lightning] and [lightning.lnbits]/[lightning.ldk] sections.
/// Node override: `[modules.lightning]` or `[modules.blvm-lightning]` in node config.
#[config(name = "lightning")]
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct LightningConfig {
    /// Provider type: "lnbits", "ldk", or "stub"
    #[serde(default = "default_provider")]
    #[config_env]
    pub provider: String,

    #[serde(default)]
    pub lnbits: LnbitsConfig,

    #[serde(default)]
    pub ldk: LdkConfig,

    /// Min payment in sats (enforced in create_invoice).
    #[serde(default)]
    pub min_payment_sats: Option<u64>,
    /// Max payment in sats (enforced in create_invoice).
    #[serde(default)]
    pub max_payment_sats: Option<u64>,
    /// Channel reserve in sats (LDK).
    #[serde(default)]
    pub channel_reserve: Option<u64>,
}

fn default_provider() -> String {
    "stub".to_string()
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
        m
    }
}
