//! NodeAPI IPC wrapper for Lightning module
//!
//! Provides NodeAPI trait implementation over IPC for the Lightning module.

use async_trait::async_trait;
use blvm_node::module::ipc::client::ModuleIpcClient;
use blvm_node::module::EventType;
use blvm_node::module::ipc::protocol::{EventPayload, RequestMessage, RequestPayload, ResponsePayload};
use blvm_node::module::traits::{ModuleError, NodeAPI};
use blvm_protocol::{Block, BlockHeader, Hash, OutPoint, Transaction, UTXO};
use std::sync::Arc;
use tokio::sync::Mutex;

/// NodeAPI implementation over IPC
pub struct NodeApiIpc {
    ipc_client: Arc<Mutex<ModuleIpcClient>>,
    correlation_id: Arc<Mutex<u64>>,
}

impl NodeApiIpc {
    /// Create a new NodeAPI IPC wrapper
    pub fn new(ipc_client: Arc<Mutex<ModuleIpcClient>>) -> Self {
        Self {
            ipc_client,
            correlation_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Get next correlation ID
    async fn next_correlation_id(&self) -> u64 {
        let mut id = self.correlation_id.lock().await;
        *id += 1;
        *id
    }

    /// Helper method to make IPC requests
    async fn request<T, F>(
        &self,
        payload: RequestPayload,
        request_type: blvm_node::module::ipc::protocol::MessageType,
        mapper: F,
    ) -> Result<T, ModuleError>
    where
        F: FnOnce(ResponsePayload) -> Result<T, ModuleError>,
    {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type,
            payload,
        };

        let response = {
            let mut client = self.ipc_client.lock().await;
            client.request(request).await?
        };

        if !response.success {
            return Err(ModuleError::OperationError(
                response.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        match response.payload {
            Some(payload) => mapper(payload),
            None => Err(ModuleError::OperationError("Empty response payload".to_string())),
        }
    }
}

#[async_trait]
impl NodeAPI for NodeApiIpc {
    async fn get_block(&self, hash: &Hash) -> Result<Option<Block>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::GetBlock { hash: *hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Block(block)) => Ok(block),
            _ => Ok(None),
        }
    }

    async fn get_block_header(&self, hash: &Hash) -> Result<Option<BlockHeader>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlockHeader,
            payload: RequestPayload::GetBlockHeader { hash: *hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::BlockHeader(header)) => Ok(header),
            _ => Ok(None),
        }
    }

    async fn get_transaction(&self, hash: &Hash) -> Result<Option<Transaction>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetTransaction,
            payload: RequestPayload::GetTransaction { hash: *hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Transaction(tx)) => Ok(tx),
            _ => Ok(None),
        }
    }

    async fn has_transaction(&self, hash: &Hash) -> Result<bool, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::HasTransaction,
            payload: RequestPayload::HasTransaction { hash: *hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Bool(value)) => Ok(value),
            _ => Ok(false),
        }
    }

    async fn get_chain_tip(&self) -> Result<Hash, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetChainTip,
            payload: RequestPayload::GetChainTip,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Hash(hash)) => Ok(hash),
            _ => Err(ModuleError::OperationError("Failed to get chain tip".to_string())),
        }
    }

    async fn get_block_height(&self) -> Result<u64, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlockHeight,
            payload: RequestPayload::GetBlockHeight,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::U64(height)) => Ok(height),
            _ => Err(ModuleError::OperationError("Failed to get block height".to_string())),
        }
    }

    async fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXO>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetUtxo,
            payload: RequestPayload::GetUtxo {
                outpoint: outpoint.clone(),
            },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Utxo(utxo)) => Ok(utxo),
            _ => Ok(None),
        }
    }

    async fn subscribe_events(
        &self,
        _event_types: Vec<blvm_node::module::traits::EventType>,
    ) -> Result<tokio::sync::mpsc::Receiver<blvm_node::module::ipc::protocol::ModuleMessage>, ModuleError> {
        // Events are handled via ModuleClient::subscribe_events() in main.rs
        // This is just a stub for the trait
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(rx)
    }

    async fn get_mempool_transactions(&self) -> Result<Vec<Hash>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetMempoolTransactions,
            payload: RequestPayload::GetMempoolTransactions,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::MempoolTransactions(hashes)) => Ok(hashes),
            _ => Ok(Vec::new()),
        }
    }

    async fn get_mempool_transaction(&self, tx_hash: &Hash) -> Result<Option<Transaction>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetMempoolTransaction,
            payload: RequestPayload::GetMempoolTransaction { tx_hash: *tx_hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::MempoolTransaction(tx)) => Ok(tx),
            _ => Ok(None),
        }
    }

    async fn get_mempool_size(&self) -> Result<blvm_node::module::traits::MempoolSize, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetMempoolSize,
            payload: RequestPayload::GetMempoolSize,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::MempoolSize(size)) => Ok(size),
            _ => Err(ModuleError::OperationError("Failed to get mempool size".to_string())),
        }
    }

    async fn get_network_stats(&self) -> Result<blvm_node::module::traits::NetworkStats, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetNetworkStats,
            payload: RequestPayload::GetNetworkStats,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::NetworkStats(stats)) => Ok(stats),
            _ => Err(ModuleError::OperationError("Failed to get network stats".to_string())),
        }
    }

    async fn get_network_peers(&self) -> Result<Vec<blvm_node::module::traits::PeerInfo>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetNetworkPeers,
            payload: RequestPayload::GetNetworkPeers,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::NetworkPeers(peers)) => Ok(peers),
            _ => Ok(Vec::new()),
        }
    }

    async fn get_chain_info(&self) -> Result<blvm_node::module::traits::ChainInfo, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetChainInfo,
            payload: RequestPayload::GetChainInfo,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::ChainInfo(info)) => Ok(info),
            _ => Err(ModuleError::OperationError("Failed to get chain info".to_string())),
        }
    }

    async fn get_block_by_height(&self, height: u64) -> Result<Option<Block>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlockByHeight,
            payload: RequestPayload::GetBlockByHeight { height },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::BlockByHeight(block)) => Ok(block),
            _ => Ok(None),
        }
    }

    async fn get_lightning_node_url(&self) -> Result<Option<String>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetLightningNodeUrl,
            payload: RequestPayload::GetLightningNodeUrl,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::LightningNodeUrl(url)) => Ok(url),
            _ => Ok(None),
        }
    }

    async fn get_lightning_info(&self) -> Result<Option<blvm_node::module::traits::LightningInfo>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetLightningInfo,
            payload: RequestPayload::GetLightningInfo,
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::LightningInfo(info)) => Ok(info),
            _ => Ok(None),
        }
    }

    async fn get_payment_state(&self, payment_id: &str) -> Result<Option<blvm_node::module::traits::PaymentState>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetPaymentState,
            payload: RequestPayload::GetPaymentState {
                payment_id: payment_id.to_string(),
            },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::PaymentState(state)) => Ok(state),
            _ => Ok(None),
        }
    }

    async fn check_transaction_in_mempool(&self, tx_hash: &Hash) -> Result<bool, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::CheckTransactionInMempool,
            payload: RequestPayload::CheckTransactionInMempool { tx_hash: *tx_hash },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::CheckTransactionInMempool(exists)) => Ok(exists),
            _ => Ok(false),
        }
    }

    async fn get_fee_estimate(&self, target_blocks: u32) -> Result<u64, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetFeeEstimate,
            payload: RequestPayload::GetFeeEstimate { target_blocks },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::FeeEstimate(estimate)) => Ok(estimate),
            _ => Err(ModuleError::OperationError("Failed to get fee estimate".to_string())),
        }
    }

    // Filesystem API methods (implemented via IPC - same as governance)
    async fn read_file(&self, path: String) -> Result<Vec<u8>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::ReadFile { path },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::FileData(data)) => Ok(data),
            _ => Err(ModuleError::OperationError("Failed to read file".to_string())),
        }
    }

    async fn write_file(&self, path: String, data: Vec<u8>) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::WriteFile { path, data },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to write file".to_string())))
        }
    }

    async fn delete_file(&self, path: String) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::DeleteFile { path },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to delete file".to_string())))
        }
    }

    async fn list_directory(&self, path: String) -> Result<Vec<String>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::ListDirectory { path },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::DirectoryListing(entries)) => Ok(entries),
            _ => Ok(Vec::new()),
        }
    }

    async fn create_directory(&self, path: String) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::CreateDirectory { path },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to create directory".to_string())))
        }
    }

    async fn get_file_metadata(
        &self,
        path: String,
    ) -> Result<blvm_node::module::ipc::protocol::FileMetadata, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::GetFileMetadata { path },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::FileMetadata(metadata)) => Ok(metadata),
            _ => Err(ModuleError::OperationError("Failed to get file metadata".to_string())),
        }
    }

    // Storage API methods (implemented via IPC)
    async fn storage_open_tree(&self, name: String) -> Result<String, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageOpenTree { name },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::StorageTreeId(tree_id)) => Ok(tree_id),
            _ => Err(ModuleError::OperationError("Failed to open storage tree".to_string())),
        }
    }

    async fn storage_insert(&self, tree_id: String, key: Vec<u8>, value: Vec<u8>) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageInsert { tree_id, key, value },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to insert into storage".to_string())))
        }
    }

    async fn storage_get(&self, tree_id: String, key: Vec<u8>) -> Result<Option<Vec<u8>>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageGet { tree_id, key },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::StorageValue(value)) => Ok(value),
            _ => Ok(None),
        }
    }

    async fn storage_remove(&self, tree_id: String, key: Vec<u8>) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageRemove { tree_id, key },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to remove from storage".to_string())))
        }
    }

    async fn storage_contains_key(&self, tree_id: String, key: Vec<u8>) -> Result<bool, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageContainsKey { tree_id, key },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::Bool(exists)) => Ok(exists),
            _ => Ok(false),
        }
    }

    async fn storage_iter(&self, tree_id: String) -> Result<Vec<(Vec<u8>, Vec<u8>)>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageIter { tree_id },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::StorageKeyValuePairs(pairs)) => Ok(pairs),
            _ => Ok(Vec::new()),
        }
    }

    async fn storage_transaction(
        &self,
        tree_id: String,
        operations: Vec<blvm_node::module::ipc::protocol::StorageOperation>,
    ) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetBlock,
            payload: RequestPayload::StorageTransaction { tree_id, operations },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(response.error.unwrap_or_else(|| "Failed to execute storage transaction".to_string())))
        }
    }

    // Module RPC endpoint registration (not yet implemented in IPC protocol)
    // Module RPC endpoint registration
    async fn register_rpc_endpoint(
        &self,
        method: String,
        description: String,
    ) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::RegisterRpcEndpoint,
            payload: RequestPayload::RegisterRpcEndpoint { method, description },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(
                response.error.unwrap_or_else(|| "Failed to register RPC endpoint".to_string()),
            ))
        }
    }

    async fn unregister_rpc_endpoint(&self, method: &str) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::UnregisterRpcEndpoint,
            payload: RequestPayload::UnregisterRpcEndpoint {
                method: method.to_string(),
            },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(
                response.error.unwrap_or_else(|| "Failed to unregister RPC endpoint".to_string()),
            ))
        }
    }

    // Timers and scheduled tasks
    // Note: Timer callbacks cannot be serialized over IPC, so modules should manage
    // timers locally using tokio::time::interval and tokio::time::sleep
    async fn register_timer(
        &self,
        _interval_seconds: u64,
        _callback: Arc<dyn blvm_node::module::timers::manager::TimerCallback>,
    ) -> Result<blvm_node::module::timers::manager::TimerId, ModuleError> {
        Err(ModuleError::OperationError(
            "Timer callbacks cannot be serialized over IPC. Use tokio::time::interval for module-side timers.".to_string(),
        ))
    }

    async fn cancel_timer(
        &self,
        _timer_id: blvm_node::module::timers::manager::TimerId,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError(
            "Timer callbacks cannot be serialized over IPC. Manage timers locally in the module.".to_string(),
        ))
    }

    async fn schedule_task(
        &self,
        _delay_seconds: u64,
        _callback: Arc<dyn blvm_node::module::timers::manager::TaskCallback>,
    ) -> Result<blvm_node::module::timers::manager::TaskId, ModuleError> {
        Err(ModuleError::OperationError(
            "Task callbacks cannot be serialized over IPC. Use tokio::time::sleep for module-side delayed tasks.".to_string(),
        ))
    }

    // Metrics and telemetry
    async fn report_metric(&self, metric: blvm_node::module::metrics::manager::Metric) -> Result<(), ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::ReportMetric,
            payload: RequestPayload::ReportMetric { metric },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        if response.success {
            Ok(())
        } else {
            Err(ModuleError::OperationError(
                response.error.unwrap_or_else(|| "Failed to report metric".to_string()),
            ))
        }
    }

    async fn get_module_metrics(
        &self,
        module_id: &str,
    ) -> Result<Vec<blvm_node::module::metrics::manager::Metric>, ModuleError> {
        let correlation_id = self.next_correlation_id().await;
        let request = RequestMessage {
            correlation_id,
            request_type: blvm_node::module::ipc::protocol::MessageType::GetModuleMetrics,
            payload: RequestPayload::GetModuleMetrics {
                module_id: module_id.to_string(),
            },
        };

        let response = self.ipc_client.lock().await.request(request).await?;
        match response.payload {
            Some(ResponsePayload::ModuleMetrics(metrics)) => Ok(metrics),
            _ => Err(ModuleError::OperationError(
                response.error.unwrap_or_else(|| "Failed to get module metrics".to_string()),
            )),
        }
    }

    // Module initialization (handled by IPC server during handshake)
    async fn initialize_module(
        &self,
        _module_id: String,
        _module_data_dir: std::path::PathBuf,
        _base_data_dir: std::path::PathBuf,
    ) -> Result<(), ModuleError> {
        // This is called by the IPC server during handshake, not by modules directly
        Ok(())
    }
    
    async fn discover_modules(&self) -> Result<Vec<blvm_node::module::traits::ModuleInfo>, ModuleError> {
        self.request(
            RequestPayload::DiscoverModules,
            blvm_node::module::ipc::protocol::MessageType::DiscoverModules,
            |payload| match payload {
                ResponsePayload::ModuleList(modules) => Ok(modules),
                _ => Err(ModuleError::OperationError("Unexpected response type".to_string())),
            },
        )
        .await
    }
    
    async fn get_module_info(&self, module_id: &str) -> Result<Option<blvm_node::module::traits::ModuleInfo>, ModuleError> {
        self.request(
            RequestPayload::GetModuleInfo {
                module_id: module_id.to_string(),
            },
            blvm_node::module::ipc::protocol::MessageType::GetModuleInfo,
            |payload| match payload {
                ResponsePayload::ModuleInfo(info) => Ok(info),
                _ => Err(ModuleError::OperationError("Unexpected response type".to_string())),
            },
        )
        .await
    }
    
    async fn is_module_available(&self, module_id: &str) -> Result<bool, ModuleError> {
        self.request(
            RequestPayload::IsModuleAvailable {
                module_id: module_id.to_string(),
            },
            blvm_node::module::ipc::protocol::MessageType::IsModuleAvailable,
            |payload| match payload {
                ResponsePayload::ModuleAvailable(available) => Ok(available),
                _ => Err(ModuleError::OperationError("Unexpected response type".to_string())),
            },
        )
        .await
    }
    
    async fn publish_event(
        &self,
        event_type: EventType,
        payload: EventPayload,
    ) -> Result<(), ModuleError> {
        self.request(
            RequestPayload::PublishEvent { event_type, payload },
            blvm_node::module::ipc::protocol::MessageType::PublishEvent,
            |payload| match payload {
                ResponsePayload::EventPublished => Ok(()),
                _ => Err(ModuleError::OperationError("Unexpected response type".to_string())),
            },
        )
        .await
    }
    
    async fn send_mesh_packet_to_peer(
        &self,
        peer_addr: String,
        packet_data: Vec<u8>,
    ) -> Result<(), ModuleError> {
        self.request(
            RequestPayload::SendMeshPacketToPeer { peer_addr, packet_data },
            blvm_node::module::ipc::protocol::MessageType::SendMeshPacketToPeer,
            |payload| match payload {
                ResponsePayload::Bool(success) => {
                    if success {
                        Ok(())
                    } else {
                        Err(ModuleError::OperationError("Failed to send mesh packet".to_string()))
                    }
                }
                _ => Err(ModuleError::OperationError("Invalid response format".to_string())),
            },
        )
        .await
    }

    async fn get_all_metrics(&self) -> Result<std::collections::HashMap<String, Vec<blvm_node::module::metrics::manager::Metric>>, ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn call_module(
        &self,
        _target_module_id: Option<&str>,
        _method: &str,
        _params: Vec<u8>,
    ) -> Result<Vec<u8>, ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn register_module_api(
        &self,
        _api: Arc<dyn blvm_node::module::inter_module::api::ModuleAPI>,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn unregister_module_api(&self) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn send_mesh_packet_to_module(
        &self,
        _module_id: &str,
        _packet_data: Vec<u8>,
        _peer_addr: String,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn send_stratum_v2_message_to_peer(
        &self,
        _peer_addr: String,
        _message_data: Vec<u8>,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn get_module_health(&self, _module_id: &str) -> Result<Option<blvm_node::module::process::monitor::ModuleHealth>, ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn get_all_module_health(&self) -> Result<Vec<(String, blvm_node::module::process::monitor::ModuleHealth)>, ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }

    async fn report_module_health(
        &self,
        _health: blvm_node::module::process::monitor::ModuleHealth,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::OperationError("Not implemented".to_string()))
    }
}

