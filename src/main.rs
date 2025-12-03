//! bllvm-lightning - Lightning Network payment processor module
//!
//! This module provides Lightning Network payment processing capabilities
//! for bllvm-node, including invoice verification, payment routing, and
//! channel management.

use anyhow::Result;
use blvm_node::module::{EventType, EventMessage};
use blvm_node::module::ipc::protocol::{EventPayload, LogLevel, ModuleMessage};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

mod provider;
mod processor;
mod invoice;
mod error;
mod client;
mod nodeapi_ipc;

use processor::LightningProcessor;
use error::LightningError;
use client::ModuleClient;
use nodeapi_ipc::NodeApiIpc;

/// Command-line arguments for the module
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Module ID (provided by node)
    #[arg(long)]
    module_id: Option<String>,

    /// IPC socket path (provided by node)
    #[arg(long)]
    socket_path: Option<PathBuf>,

    /// Data directory (provided by node)
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // Get module ID (from args or environment)
    let module_id = args.module_id
        .or_else(|| std::env::var("MODULE_NAME").ok())
        .unwrap_or_else(|| "bllvm-lightning".to_string());

    // Get socket path (from args, env, or default)
    let socket_path = args.socket_path
        .or_else(|| std::env::var("BLLVM_MODULE_SOCKET").ok().map(PathBuf::from))
        .or_else(|| std::env::var("MODULE_SOCKET_DIR").ok().map(|d| PathBuf::from(d).join("modules.sock")))
        .unwrap_or_else(|| PathBuf::from("data/modules/modules.sock"));

    info!("bllvm-lightning module starting... (module_id: {}, socket: {:?})", module_id, socket_path);

    // Connect to node (clone socket_path before moving it)
    let socket_path_for_connect = socket_path.clone();
    let mut client = match ModuleClient::connect(
        socket_path_for_connect,
        module_id.clone(),
        "bllvm-lightning".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to connect to node: {}", e);
            return Err(anyhow::anyhow!("Connection failed: {}", e));
        }
    };

    // Subscribe to payment events
    let event_types = vec![
        EventType::PaymentRequestCreated,
        EventType::PaymentSettled,
        EventType::PaymentFailed,
    ];

    if let Err(e) = client.subscribe_events(event_types).await {
        error!("Failed to subscribe to events: {}", e);
        return Err(anyhow::anyhow!("Subscription failed: {}", e));
    }

    // Create NodeAPI wrapper
    let ipc_client = client.get_ipc_client();
    let node_api = Arc::new(NodeApiIpc::new(ipc_client));

    // Create processor
    let ctx = blvm_node::module::traits::ModuleContext {
        module_id: module_id.clone(),
        config: std::collections::HashMap::new(),
        data_dir: args.data_dir.unwrap_or_else(|| PathBuf::from("data/modules/bllvm-lightning")).to_string_lossy().to_string(),
        socket_path: socket_path.clone().to_string_lossy().to_string(),
    };
    let processor = LightningProcessor::new(&ctx, node_api.clone()).await
        .map_err(|e| anyhow::anyhow!("Failed to create processor: {}", e))?;
    
    // Wrap processor in Arc for parallel processing
    let processor = Arc::new(processor);

    info!("Lightning module initialized and running");

    // Event processing loop with parallel batch processing
    let mut event_receiver = client.event_receiver();
    loop {
        // Collect batch of events (up to 10) for parallel processing
        let mut event_batch = Vec::with_capacity(10);
        for _ in 0..10 {
            match event_receiver.try_recv() {
                Ok(event) => event_batch.push(event),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    warn!("Event channel disconnected");
                    return Ok(());
                }
            }
        }
        
        // If no events in batch, wait for next event
        if event_batch.is_empty() {
            if let Some(event) = event_receiver.recv().await {
                event_batch.push(event);
            } else {
                break; // Channel closed
            }
        }
        
        // Process events in parallel
        let futures: Vec<_> = event_batch
            .iter()
            .map(|event| {
                let event = event.clone();
                let processor = Arc::clone(&processor);
                let node_api = Arc::clone(&node_api);
                async move {
                    // Handle events with processor
                    if let Err(e) = processor.handle_event(&event, node_api.as_ref()).await {
                        warn!("Error handling event in processor: {}", e);
                    }

                    match event {
                    ModuleMessage::Event(event_msg) => {
                        match event_msg.event_type {
                            EventType::PaymentRequestCreated => {
                                info!("Payment request created event received");
                            }
                            EventType::PaymentSettled => {
                                info!("Payment settled event received");
                            }
                            EventType::PaymentFailed => {
                                warn!("Payment failed event received");
                            }
                            _ => {
                                // Ignore other events
                            }
                        }
                    }
                    _ => {
                        // Not an event message
                    }
                }
                }
            })
            .collect();
        
        // Wait for all events in batch to be processed
        futures::future::join_all(futures).await;
    }

    warn!("Event receiver closed, module shutting down");
    Ok(())
}

