//! blvm-lightning - Lightning Network payment processor module
//!
//! When spawned by the node: reads MODULE_ID, SOCKET_PATH, DATA_DIR from env.
//! For manual testing: blvm-lightning --module-id <id> --socket-path <path> --data-dir <dir>

use anyhow::Result;
use blvm_lightning::storage::up_v1;
use blvm_lightning::{LightningConfig, LightningModule, LightningModuleApi, LightningProcessor};
use blvm_sdk::migrations;
use blvm_sdk::module::{ModuleBootstrap, ModuleDb};
use std::sync::Arc;
use tracing::warn;

const MODULE_NAME: &str = "blvm-lightning";

#[tokio::main]
async fn main() -> Result<()> {
    let bootstrap = ModuleBootstrap::init_module(MODULE_NAME);
    let db = ModuleDb::open_or_temp_with_migrations(
        &bootstrap.data_dir,
        MODULE_NAME,
        migrations!(1 => up_v1),
    )?;

    let setup = |node_api: Arc<dyn blvm_node::module::traits::NodeAPI>,
                 db: Arc<dyn blvm_node::storage::database::Database>,
                 data_dir: &std::path::Path| {
        let bootstrap = bootstrap.clone();
        let data_dir = data_dir.to_path_buf();
        async move {
            let (ctx, _config) = bootstrap.context_with_config::<LightningConfig>(&data_dir);
            let processor = LightningProcessor::new(&ctx, Arc::clone(&node_api))
                .await
                .map_err(|e| {
                    blvm_node::module::traits::ModuleError::Other(format!(
                        "Failed to create processor: {e}"
                    ))
                })?;
            let processor = Arc::new(processor);
            let lightning_api = Arc::new(LightningModuleApi::new(Arc::clone(&processor)));
            if let Err(e) = node_api.register_module_api(lightning_api).await {
                warn!("Failed to register lightning module API: {}", e);
            }
            let module = LightningModule {
                processor: Arc::clone(&processor),
                db: Some(Arc::clone(&db)),
            };
            Ok((module.clone(), module))
        }
    };

    blvm_sdk::run_module! {
        bootstrap: &bootstrap,
        module_name: MODULE_NAME,
        module_type: LightningModule,
        cli_type: LightningModule,
        db: db.as_db(),
        setup: setup,
        event_types: LightningModule::event_types(),
    }?;

    Ok(())
}
