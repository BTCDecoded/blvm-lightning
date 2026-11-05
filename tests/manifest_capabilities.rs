use blvm_node::module::security::permissions::parse_permission_string;

#[test]
fn module_capabilities_are_node_permissions() {
    let manifest: Manifest = toml::from_str(include_str!("../module.toml")).expect("module.toml");
    let required = [
        "read_blockchain",
        "read_chain_state",
        "read_utxo",
        "read_mempool",
        "send_transactions",
        "subscribe_events",
        "publish_events",
    ];
    for name in required {
        assert!(
            manifest.capabilities.iter().any(|cap| cap == name),
            "module.toml is missing {name}"
        );
        assert!(
            parse_permission_string(name).is_some(),
            "{name} is not a node permission"
        );
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    capabilities: Vec<String>,
}
