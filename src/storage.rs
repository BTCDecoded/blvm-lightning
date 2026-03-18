//! Storage migrations for blvm-lightning.
//!
//! v1: Migrate invoices from legacy "items" tree to "invoices".

use blvm_sdk::module::{MigrationContext, MigrationUp};

const INVOICES_TREE: &str = "invoices";

pub fn up_v1(ctx: &MigrationContext) -> anyhow::Result<()> {
    let items_tree = ctx.open_tree("items")?;
    if let Some(data) = items_tree.get(b"lightning:invoices")? {
        let invoices_tree = ctx.open_tree(INVOICES_TREE)?;
        invoices_tree.insert(b"lightning:invoices", &data)?;
    }
    Ok(())
}

pub const MIGRATIONS: &[(u32, MigrationUp)] = &[(1, up_v1)];
