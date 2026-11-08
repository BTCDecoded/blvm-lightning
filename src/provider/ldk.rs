//! LDK provider.
//!
//! Owns an LDK [`ChannelManager`] and [`ChainMonitor`] in this module process.
//! Invoices are registered with that manager and signed by its node key.
//! A parsed invoice is not a paid invoice.
//!
//! On startup the processor calls [`LightningProvider::attach_node`]. A manager
//! that is still at the genesis height and has no channels is rebuilt at the
//! node tip. Any other manager walks forward one block at a time. Transactions
//! the manager asks to broadcast are submitted to the node mempool and appended
//! to `ldk/broadcasts.log`.
//!
//! `listen_port` binds a peer listener. `peers` are dialed as `pubkey@host:port`.
//! `pay_invoice` asks LDK to send. LDK accepting the send is not the preimage.
//! Coins reserved by `open_channel` are written to `ldk/funding.json` so a
//! restart can still build the funding transaction when the peer accepts.

use crate::error::LightningError;
use crate::provider::{
    ChannelInfo, LightningProvider, PayInvoiceResult, PaymentVerificationResult, ProviderType,
};
use async_trait::async_trait;
use bitcoin::Network;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::PublicKey;
use blvm_node::module::ipc::protocol::EventPayload;
use blvm_node::module::traits::{EventType, NodeAPI};
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning::chain::channelmonitor::{ChannelMonitor, ChannelMonitorUpdate};
use lightning::chain::{
    BestBlock, ChannelMonitorUpdateStatus, Filter, Listen, Watch, WatchedOutput,
};
use lightning::events::{Event, EventsProvider, PaymentPurpose, ReplayEvent};
use lightning::ln::channelmanager::{
    ChainParameters, ChannelManager, ChannelManagerReadArgs, MIN_FINAL_CLTV_EXPIRY_DELTA,
    PaymentId, RecipientOnionFields, Retry,
};
use lightning::ln::peer_handler::{IgnoringMessageHandler, MessageHandler, PeerManager};
use lightning::ln::types::ChannelId;
use lightning::ln::types::{PaymentHash, PaymentPreimage};
use lightning::routing::gossip::{NetworkGraph, P2PGossipSync};
use lightning::routing::router::{DefaultRouter, PaymentParameters, RouteParameters};
use lightning::routing::scoring::{
    ProbabilisticScorer, ProbabilisticScoringDecayParameters, ProbabilisticScoringFeeParameters,
};
use lightning::routing::utxo::{UtxoLookup, UtxoLookupError, UtxoResult};
use lightning::sign::{
    InMemorySigner, KeysManager, NodeSigner, OutputSpender, Recipient, SignerProvider,
    SpendableOutputDescriptor,
};
use lightning::util::config::UserConfig;
use lightning::util::logger::{Level, Logger, Record};
use lightning::util::ser::{ReadableArgs, Writeable};
use lightning_invoice::{Bolt11Invoice, Currency, InvoiceBuilder};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, trace, warn};

const FEE_SAT_PER_1000_WEIGHT: u32 = 1_000;

type Keys = Arc<KeysManager>;
type FeeEst = Arc<NodeFeeEstimator>;
type Broadcaster = Arc<QueuedBroadcaster>;
type Log = Arc<TracingLogger>;
type Store = Arc<DiskStore>;
type Graph = Arc<NetworkGraph<Log>>;
type Score = ProbabilisticScorer<Graph, Log>;
type Scorer = Arc<std::sync::RwLock<Score>>;
type LdkRouter = DefaultRouter<Graph, Log, Keys, Scorer, ProbabilisticScoringFeeParameters, Score>;
type Monitor = lightning::chain::chainmonitor::ChainMonitor<
    InMemorySigner,
    Arc<NullFilter>,
    Broadcaster,
    FeeEst,
    Log,
    Store,
>;
type Manager =
    ChannelManager<Arc<Monitor>, Broadcaster, Keys, Keys, Keys, FeeEst, Arc<LdkRouter>, Log>;
type Gossip = P2PGossipSync<Graph, Arc<NoUtxo>, Log>;
type PeerMgr = PeerManager<
    lightning_net_tokio::SocketDescriptor,
    Arc<Manager>,
    Arc<Gossip>,
    IgnoringMessageHandler,
    Log,
    IgnoringMessageHandler,
    Keys,
>;

/// LDK provider configuration.
#[derive(Debug, Clone)]
pub struct LDKConfig {
    /// Data directory for the LDK seed, channel manager, and monitors.
    pub data_dir: PathBuf,
    /// Network name: mainnet, testnet, signet, or regtest.
    pub network: String,
    /// Optional 32-byte LDK seed. This is not the node secret.
    pub node_private_key: Option<Vec<u8>>,
    /// Inbound peer port. `None` does not bind a socket.
    pub listen_port: Option<u16>,
    /// `pubkey@host:port` peers to dial after the chain tip is current.
    pub peers: Vec<String>,
    /// Largest inbound channel in satoshis. `None` rejects every inbound open.
    pub max_inbound_sats: Option<u64>,
}

/// Something the node should hear about after LDK emits it.
enum NodeNotice {
    Verified {
        id: String,
        amount_msats: u64,
    },
    Failed {
        id: String,
        reason: String,
    },
    ChannelReady {
        channel_id: String,
        peer_pubkey: Vec<u8>,
        capacity_sats: u64,
    },
    RouteFound {
        id: String,
        hops: usize,
        cost_msats: u64,
    },
    ChannelClosed {
        channel_id: String,
        reason: String,
    },
}

/// A confirmed output that pays the LDK deposit address.
#[derive(Clone)]
struct FundingCoin {
    txid: bitcoin::Txid,
    vout: u16,
    value_sat: u64,
    script: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredCoin {
    txid: String,
    vout: u16,
    value_sat: u64,
    script: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredReservation {
    channel_id: String,
    coins: Vec<StoredCoin>,
}

/// Bitcoin coinbase maturity. A coinbase output cannot fund a channel before this many confirms.
const COINBASE_MATURITY: u64 = 100;

/// Lightning provider backed by an LDK channel manager.
pub struct LDKProvider {
    network: Network,
    keys: Keys,
    fee: FeeEst,
    broadcaster: Broadcaster,
    logger: Log,
    graph: Graph,
    router: Arc<LdkRouter>,
    channel_manager: RwLock<Arc<Manager>>,
    chain_monitor: Arc<Monitor>,
    ldk_dir: PathBuf,
    scorer: Arc<RwLock<Score>>,
    confirmed: Arc<RwLock<HashSet<[u8; 32]>>>,
    /// Invoice amounts recorded when a payment is sent or claimed.
    amounts: Arc<Mutex<HashMap<[u8; 32], u64>>>,
    /// Fee from `PaymentSent`, used when the path event follows.
    fees: Arc<Mutex<HashMap<[u8; 32], u64>>>,
    chain_lock: tokio::sync::Mutex<()>,
    peer_manager: Mutex<Option<Arc<PeerMgr>>>,
    listen_port: Option<u16>,
    peer_specs: Vec<String>,
    max_inbound_sats: Option<u64>,
    stop: Arc<AtomicBool>,
    /// Coins reserved for an outbound channel, keyed by the temporary channel id.
    funding: Arc<Mutex<HashMap<ChannelId, Vec<FundingCoin>>>>,
}

impl LDKProvider {
    /// Open or create the LDK node under `config.data_dir/ldk`.
    pub fn new(config: LDKConfig) -> Result<Self, LightningError> {
        let network = parse_network(&config.network)?;
        let ldk_dir = config.data_dir.join("ldk");
        fs::create_dir_all(ldk_dir.join("monitors")).map_err(io_err)?;

        let seed = load_seed(&ldk_dir, config.node_private_key)?;
        let started = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
            LightningError::ProcessorError(format!("system clock is before unix epoch: {e}"))
        })?;
        let keys = Arc::new(KeysManager::new(
            &seed,
            started.as_secs(),
            started.subsec_nanos(),
        ));
        let fee = Arc::new(NodeFeeEstimator::new());
        let broadcaster = Arc::new(QueuedBroadcaster {
            path: ldk_dir.join("broadcasts.log"),
            pending: Mutex::new(Vec::new()),
        });
        let logger = Arc::new(TracingLogger);
        let store = Arc::new(DiskStore {
            dir: ldk_dir.join("monitors"),
        });
        let graph = Arc::new(load_graph(&ldk_dir, network, logger.clone())?);
        let scorer = Arc::new(std::sync::RwLock::new(load_scorer(
            &ldk_dir, &graph, &logger,
        )?));
        let router = Arc::new(DefaultRouter::new(
            Arc::clone(&graph),
            logger.clone(),
            keys.clone(),
            Arc::clone(&scorer),
            ProbabilisticScoringFeeParameters::default(),
        ));
        let chain_monitor = Arc::new(Monitor::new(
            None::<Arc<NullFilter>>,
            broadcaster.clone(),
            logger.clone(),
            fee.clone(),
            store.clone(),
        ));

        let manager_path = ldk_dir.join("channel_manager");
        let manager = if manager_path.exists() {
            load_manager(
                &manager_path,
                keys.clone(),
                fee.clone(),
                broadcaster.clone(),
                logger.clone(),
                router.clone(),
                chain_monitor.clone(),
                store.as_ref(),
            )?
        } else {
            let genesis = genesis_block(network).block_hash();
            let now = u32::try_from(started.as_secs()).unwrap_or(u32::MAX);
            Arc::new(ChannelManager::new(
                fee.clone(),
                chain_monitor.clone(),
                broadcaster.clone(),
                router.clone(),
                logger.clone(),
                keys.clone(),
                keys.clone(),
                keys.clone(),
                channel_config(),
                ChainParameters {
                    network,
                    best_block: BestBlock::new(genesis, 0),
                },
                now,
            ))
        };

        let node_id = manager.get_our_node_id();
        info!(%network, node_id = %node_id, dir = %ldk_dir.display(), "LDK channel manager ready");
        let funding = load_funding(&ldk_dir)?;
        let (confirmed, amounts) = load_payments(&ldk_dir)?;

        let provider = Self {
            network,
            keys,
            fee,
            broadcaster,
            logger,
            graph,
            scorer,
            router,
            channel_manager: RwLock::new(manager),
            chain_monitor,
            ldk_dir,
            confirmed: Arc::new(RwLock::new(confirmed)),
            amounts: Arc::new(Mutex::new(amounts)),
            fees: Arc::new(Mutex::new(HashMap::new())),
            chain_lock: tokio::sync::Mutex::new(()),
            peer_manager: Mutex::new(None),
            listen_port: config.listen_port,
            peer_specs: config.peers,
            max_inbound_sats: config.max_inbound_sats,
            stop: Arc::new(AtomicBool::new(false)),
            funding: Arc::new(Mutex::new(funding)),
        };
        let _ = provider.drain_events();
        Ok(provider)
    }

    fn manager(&self) -> Arc<Manager> {
        self.channel_manager
            .read()
            .expect("LDK channel manager lock")
            .clone()
    }

    fn drain_events(&self) -> Vec<NodeNotice> {
        drain_manager(
            self.manager().as_ref(),
            &self.confirmed,
            self.keys.as_ref(),
            self.fee.as_ref(),
            self.broadcaster.as_ref(),
            &self.funding,
            &self.ldk_dir,
            self.max_inbound_sats,
            &self.amounts,
            &self.fees,
        )
    }

    #[cfg(test)]
    fn connected_peers(&self) -> usize {
        self.peer_manager
            .lock()
            .expect("LDK peer manager")
            .as_ref()
            .map(|peers| peers.list_peers().len())
            .unwrap_or(0)
    }

    fn is_confirmed(&self, payment_hash: &[u8; 32]) -> bool {
        self.confirmed
            .read()
            .map(|set| set.contains(payment_hash))
            .unwrap_or(false)
    }

    fn persist_manager(&self) -> Result<(), LightningError> {
        let path = self.ldk_dir.join("channel_manager");
        let tmp = self.ldk_dir.join("channel_manager.tmp");
        let mut file = File::create(&tmp).map_err(io_err)?;
        self.manager()
            .write(&mut file)
            .map_err(|err| LightningError::ProcessorError(err.to_string()))?;
        file.sync_all().map_err(io_err)?;
        fs::rename(&tmp, &path).map_err(io_err)?;
        Ok(())
    }
}

#[async_trait]
impl LightningProvider for LDKProvider {
    async fn verify_payment(
        &self,
        invoice: &str,
        payment_hash: &[u8; 32],
        payment_id: &str,
    ) -> Result<PaymentVerificationResult, LightningError> {
        debug!(payment_id, "checking LDK payment");
        let _ = self.drain_events();

        let parsed: Bolt11Invoice = invoice
            .parse()
            .map_err(|e| LightningError::InvoiceError(format!("failed to parse invoice: {e:?}")))?;
        parsed.check_signature().map_err(|e| {
            LightningError::InvoiceError(format!("invoice signature check failed: {e:?}"))
        })?;

        if parsed.payment_hash().to_byte_array() != *payment_hash {
            return Ok(unverified(self, payment_hash, "payment_hash_mismatch"));
        }

        let confirmed = self.is_confirmed(payment_hash);
        Ok(PaymentVerificationResult {
            verified: confirmed,
            amount_msats: parsed.amount_milli_satoshis(),
            timestamp: None,
            metadata: serde_json::json!({
                "provider": "ldk",
                "payment_hash": hex::encode(payment_hash),
                "network": format!("{:?}", self.network),
                "node_id": self.manager().get_our_node_id().to_string(),
                "chain_height": self.manager().current_best_block().height,
            }),
        })
    }

    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: &str,
        expiry_seconds: u64,
    ) -> Result<String, LightningError> {
        let expiry = u32::try_from(expiry_seconds).map_err(|_| {
            LightningError::ProcessorError("invoice expiry does not fit in u32 seconds".to_string())
        })?;
        let (hash, secret) = self
            .manager()
            .create_inbound_payment(
                Some(amount_msats),
                expiry,
                Some(MIN_FINAL_CLTV_EXPIRY_DELTA),
            )
            .map_err(|()| {
                LightningError::ProcessorError(
                    "LDK rejected the inbound payment registration".to_string(),
                )
            })?;

        let currency = Currency::from(self.network);
        let payment_hash = bitcoin::hashes::sha256::Hash::from_byte_array(hash.0);
        let mut builder = InvoiceBuilder::new(currency)
            .amount_milli_satoshis(amount_msats)
            .description(description.to_string())
            .payment_hash(payment_hash)
            .payment_secret(secret)
            .expiry_time(std::time::Duration::from_secs(expiry_seconds))
            .min_final_cltv_expiry_delta(u64::from(MIN_FINAL_CLTV_EXPIRY_DELTA))
            .current_timestamp();
        for channel in self.manager().list_usable_channels() {
            let Some(short_channel_id) = channel.get_inbound_payment_scid() else {
                continue;
            };
            let Some(info) = channel.counterparty.forwarding_info.as_ref() else {
                continue;
            };
            builder = builder.private_route(lightning_invoice::RouteHint(vec![
                lightning_invoice::RouteHintHop {
                    src_node_id: channel.counterparty.node_id,
                    short_channel_id,
                    fees: lightning_invoice::RoutingFees {
                        base_msat: info.fee_base_msat,
                        proportional_millionths: info.fee_proportional_millionths,
                    },
                    cltv_expiry_delta: info.cltv_expiry_delta,
                    htlc_minimum_msat: channel.counterparty.outbound_htlc_minimum_msat,
                    htlc_maximum_msat: channel.counterparty.outbound_htlc_maximum_msat,
                },
            ]));
        }
        let raw = builder.build_raw().map_err(|e| {
            LightningError::ProcessorError(format!("failed to build invoice: {e:?}"))
        })?;

        let signature = self
            .keys
            .sign_invoice(&raw, Recipient::Node)
            .map_err(|()| {
                LightningError::ProcessorError(
                    "LDK node key refused to sign the invoice".to_string(),
                )
            })?;
        let signed = raw.sign(|_hash| Ok::<_, ()>(signature)).map_err(|e| {
            LightningError::ProcessorError(format!("failed to attach invoice signature: {e:?}"))
        })?;
        let invoice = Bolt11Invoice::from_signed(signed).map_err(|e| {
            LightningError::ProcessorError(format!("invoice failed semantic checks: {e:?}"))
        })?;
        self.persist_manager()?;
        Ok(invoice.to_string())
    }

    async fn is_payment_confirmed(&self, payment_hash: &[u8; 32]) -> Result<bool, LightningError> {
        let _ = self.drain_events();
        Ok(self.is_confirmed(payment_hash))
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::LDK
    }

    async fn get_balance(&self) -> Result<Option<u64>, LightningError> {
        let sats = self
            .chain_monitor
            .get_claimable_balances(&[])
            .iter()
            .map(lightning::chain::channelmonitor::Balance::claimable_amount_satoshis)
            .sum();
        Ok(Some(sats))
    }

    async fn pay_invoice(&self, invoice: &str) -> Result<PayInvoiceResult, LightningError> {
        let parsed: Bolt11Invoice = invoice
            .parse()
            .map_err(|e| LightningError::InvoiceError(format!("failed to parse invoice: {e:?}")))?;
        parsed.check_signature().map_err(|e| {
            LightningError::InvoiceError(format!("invoice signature check failed: {e:?}"))
        })?;
        let amount = parsed.amount_milli_satoshis().ok_or_else(|| {
            LightningError::InvoiceError(
                "LDK pay_invoice needs an invoice amount in millisatoshis".to_string(),
            )
        })?;
        let cltv = u32::try_from(parsed.min_final_cltv_expiry_delta())
            .unwrap_or(u32::from(MIN_FINAL_CLTV_EXPIRY_DELTA));
        let pubkey = parsed.recover_payee_pub_key();
        let params = PaymentParameters::from_node_id(pubkey, cltv);
        let params = match params.with_route_hints(parsed.route_hints()) {
            Ok(params) => params,
            Err(()) => PaymentParameters::from_node_id(pubkey, cltv),
        };
        let hash = PaymentHash(parsed.payment_hash().to_byte_array());
        let secret = *parsed.payment_secret();
        self.manager()
            .send_payment(
                hash,
                RecipientOnionFields::secret_only(secret),
                PaymentId(hash.0),
                RouteParameters::from_payment_params_and_value(params, amount),
                Retry::Attempts(3),
            )
            .map_err(|err| {
                LightningError::RoutingError(format!("LDK did not accept the payment: {err:?}"))
            })?;
        self.amounts
            .lock()
            .expect("LDK payment amounts")
            .insert(hash.0, amount);
        Ok(PayInvoiceResult {
            payment_id: hex::encode(hash.0),
            route_found: false,
            route_hops: 0,
            route_cost_msats: 0,
        })
    }

    async fn list_channels(&self) -> Result<Vec<ChannelInfo>, LightningError> {
        Ok(self
            .manager()
            .list_channels()
            .into_iter()
            .map(|channel| {
                let state = if channel.is_usable {
                    "usable"
                } else if channel.is_channel_ready {
                    "ready"
                } else {
                    "pending"
                };
                ChannelInfo {
                    channel_id: hex::encode(channel.channel_id.0),
                    peer_pubkey: channel.counterparty.node_id.serialize().to_vec(),
                    capacity_sats: channel.channel_value_satoshis,
                    state: state.to_string(),
                }
            })
            .collect())
    }

    async fn close_channel(&self, channel_id: &str) -> Result<(), LightningError> {
        let bytes = hex::decode(channel_id)
            .map_err(|e| LightningError::ProcessorError(format!("channel id is not hex: {e}")))?;
        if bytes.len() != 32 {
            return Err(LightningError::ProcessorError(
                "channel id must be 32 bytes".to_string(),
            ));
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        let channel_id = ChannelId(id);
        let peer = self
            .manager()
            .list_channels()
            .into_iter()
            .find(|channel| channel.channel_id == channel_id)
            .map(|channel| channel.counterparty.node_id)
            .ok_or_else(|| LightningError::ProcessorError(format!("no channel {channel_id}")))?;
        self.manager()
            .close_channel(&channel_id, &peer)
            .map_err(|e| LightningError::ProcessorError(format!("close_channel failed: {e:?}")))?;
        self.persist_manager()
    }

    async fn deposit_address(&self) -> Result<Option<String>, LightningError> {
        let script = self.keys.get_destination_script([0; 32]).map_err(|()| {
            LightningError::ProcessorError("LDK deposit script is unavailable".to_string())
        })?;
        let address = bitcoin::Address::from_script(&script, self.network).map_err(|err| {
            LightningError::ProcessorError(format!("LDK deposit script is not an address: {err}"))
        })?;
        Ok(Some(address.to_string()))
    }

    async fn open_channel(
        &self,
        node: &dyn NodeAPI,
        peer_pubkey: &str,
        value_sats: u64,
        outpoints: &str,
    ) -> Result<String, LightningError> {
        if value_sats < 1_000 {
            return Err(LightningError::ProcessorError(
                "channel value must be at least 1000 satoshis".to_string(),
            ));
        }
        let peer = PublicKey::from_str(peer_pubkey).map_err(|err| {
            LightningError::ProcessorError(format!("peer pubkey is invalid: {err}"))
        })?;
        let coins = self
            .collect_funding_coins(node, outpoints, value_sats)
            .await?;
        let temporary = self
            .manager()
            .create_channel(peer, value_sats, 0, 0, None, None)
            .map_err(|err| {
                LightningError::ProcessorError(format!(
                    "LDK did not open the channel. The peer must already be connected: {err:?}"
                ))
            })?;
        self.funding
            .lock()
            .expect("LDK funding reservation")
            .insert(temporary, coins);
        if let Err(err) = save_funding(&self.ldk_dir, &self.funding) {
            self.funding
                .lock()
                .expect("LDK funding reservation")
                .remove(&temporary);
            let _ = self.manager().close_channel(&temporary, &peer);
            return Err(err);
        }
        self.persist_manager()?;
        Ok(hex::encode(temporary.0))
    }

    async fn attach_node(&self, node: Arc<dyn NodeAPI>) -> Result<(), LightningError> {
        self.sync_chain(node.as_ref()).await?;
        self.start_peers(Arc::clone(&node)).await
    }

    async fn on_block_connected(
        &self,
        node: &dyn NodeAPI,
        _hash: &[u8; 32],
        _height: u64,
    ) -> Result<(), LightningError> {
        self.sync_chain(node).await
    }

    async fn on_block_disconnected(
        &self,
        node: &dyn NodeAPI,
        hash: &[u8; 32],
        height: u64,
    ) -> Result<(), LightningError> {
        let _guard = self.chain_lock.lock().await;
        let height32 = height_u32(height)?;
        let best = self.manager().current_best_block();
        if best.height != height32 {
            debug!(
                height,
                best = best.height,
                "ignoring a disconnect that is not the LDK tip"
            );
            return Ok(());
        }
        let header = match node.get_block_header(hash).await? {
            Some(header) => header,
            None => {
                let mut reversed = *hash;
                reversed.reverse();
                node.get_block_header(&reversed).await?.ok_or_else(|| {
                    LightningError::ProcessorError(format!(
                        "node has no header for the disconnected block at height {height}"
                    ))
                })?
            }
        };
        let bytes = blvm_protocol::serialization::serialize_block_header(&header);
        let bitcoin_header: bitcoin::block::Header = bitcoin::consensus::deserialize(&bytes)
            .map_err(|err| {
                LightningError::ProcessorError(format!("disconnected header is not bitcoin: {err}"))
            })?;
        self.manager().block_disconnected(&bitcoin_header, height32);
        self.chain_monitor
            .block_disconnected(&bitcoin_header, height32);
        self.persist_manager()?;
        self.publish_notices(node, self.drain_events()).await;
        self.flush_broadcasts(node).await;
        Ok(())
    }
}

fn unverified(
    provider: &LDKProvider,
    payment_hash: &[u8; 32],
    reason: &str,
) -> PaymentVerificationResult {
    PaymentVerificationResult {
        verified: false,
        amount_msats: None,
        timestamp: None,
        metadata: serde_json::json!({
            "provider": "ldk",
            "error": reason,
            "payment_hash": hex::encode(payment_hash),
            "node_id": provider.manager().get_our_node_id().to_string(),
        }),
    }
}

fn preimage_from_purpose(purpose: &PaymentPurpose) -> Option<PaymentPreimage> {
    match purpose {
        PaymentPurpose::Bolt11InvoicePayment {
            payment_preimage, ..
        }
        | PaymentPurpose::Bolt12OfferPayment {
            payment_preimage, ..
        } => *payment_preimage,
        _ => None,
    }
}

fn parse_network(name: &str) -> Result<Network, LightningError> {
    match name.to_lowercase().as_str() {
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        "testnet" => Ok(Network::Testnet),
        "signet" => Ok(Network::Signet),
        "regtest" => Ok(Network::Regtest),
        other => Err(LightningError::ConfigError(format!(
            "unknown LDK network '{other}'. Use mainnet, testnet, signet, or regtest."
        ))),
    }
}

fn load_seed(ldk_dir: &Path, configured: Option<Vec<u8>>) -> Result<[u8; 32], LightningError> {
    let path = ldk_dir.join("seed");
    let configured = match configured {
        Some(bytes) => {
            if bytes.len() != 32 {
                return Err(LightningError::ConfigError(
                    "lightning.ldk.node_private_key must decode to 32 bytes".to_string(),
                ));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            Some(seed)
        }
        None => None,
    };

    if path.exists() {
        let existing = fs::read(&path).map_err(io_err)?;
        if existing.len() != 32 {
            return Err(LightningError::ConfigError(format!(
                "LDK seed at {} is {} bytes",
                path.display(),
                existing.len()
            )));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&existing);
        if let Some(configured) = configured {
            if configured != seed {
                return Err(LightningError::ConfigError(
                    "configured LDK seed does not match the seed stored in the data directory"
                        .to_string(),
                ));
            }
        }
        return Ok(seed);
    }

    let seed = configured.unwrap_or_else(rand::random);
    write_secret(&path, &seed)?;
    Ok(seed)
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), LightningError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io_err)?;
    file.write_all(bytes).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn load_manager(
    path: &Path,
    keys: Keys,
    fee: FeeEst,
    broadcaster: Broadcaster,
    logger: Log,
    router: Arc<LdkRouter>,
    chain_monitor: Arc<Monitor>,
    store: &DiskStore,
) -> Result<Arc<Manager>, LightningError> {
    let mut monitors = store.load_monitors(keys.as_ref())?;
    let mut refs = Vec::new();
    for monitor in monitors.iter_mut() {
        refs.push(monitor);
    }
    let args = ChannelManagerReadArgs::new(
        keys.clone(),
        keys.clone(),
        keys.clone(),
        fee,
        chain_monitor.clone(),
        broadcaster,
        router,
        logger,
        channel_config(),
        refs,
    );
    let mut file = File::open(path).map_err(io_err)?;
    let (_tip, manager) = <(bitcoin::BlockHash, Manager)>::read(&mut file, args).map_err(|e| {
        LightningError::ProcessorError(format!("failed to read {}: {e:?}", path.display()))
    })?;
    for monitor in monitors {
        let outpoint = monitor.get_funding_txo().0;
        chain_monitor
            .watch_channel(outpoint, monitor)
            .map_err(|()| {
                LightningError::ProcessorError(
                    "failed to rewatch a persisted channel monitor".to_string(),
                )
            })?;
    }
    Ok(Arc::new(manager))
}

fn io_err(err: std::io::Error) -> LightningError {
    LightningError::ProcessorError(err.to_string())
}

struct NodeFeeEstimator {
    sat_per_kw: AtomicU32,
}

impl NodeFeeEstimator {
    fn new() -> Self {
        Self {
            sat_per_kw: AtomicU32::new(FEE_SAT_PER_1000_WEIGHT),
        }
    }

    fn set_sat_per_kw(&self, sat_per_kw: u32) {
        if sat_per_kw > 0 {
            self.sat_per_kw.store(sat_per_kw, Ordering::Relaxed);
        }
    }
}

impl FeeEstimator for NodeFeeEstimator {
    fn get_est_sat_per_1000_weight(&self, _confirmation_target: ConfirmationTarget) -> u32 {
        self.sat_per_kw.load(Ordering::Relaxed)
    }
}

struct QueuedBroadcaster {
    path: PathBuf,
    pending: Mutex<Vec<Vec<u8>>>,
}

impl QueuedBroadcaster {
    fn take_pending(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending.lock().expect("LDK broadcast queue"))
    }

    fn requeue_front(&self, queued: Vec<Vec<u8>>) {
        if queued.is_empty() {
            return;
        }
        let mut pending = self.pending.lock().expect("LDK broadcast queue");
        let mut queued = queued;
        queued.append(&mut *pending);
        *pending = queued;
    }
}

impl BroadcasterInterface for QueuedBroadcaster {
    fn broadcast_transactions(&self, txs: &[&bitcoin::Transaction]) {
        if txs.is_empty() {
            return;
        }
        let mut pending = self.pending.lock().expect("LDK broadcast queue");
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                for tx in txs {
                    let bytes = bitcoin::consensus::encode::serialize(*tx);
                    let line = hex::encode(&bytes);
                    if writeln!(file, "{line}").is_err() {
                        error!(path = %self.path.display(), "failed to queue LDK broadcast");
                    }
                    pending.push(bytes);
                }
                let _ = file.sync_all();
            }
            Err(err) => {
                error!(%err, path = %self.path.display(), "failed to open LDK broadcast log");
                for tx in txs {
                    pending.push(bitcoin::consensus::encode::serialize(*tx));
                }
            }
        }
    }
}

struct TracingLogger;

impl Logger for TracingLogger {
    fn log(&self, record: Record) {
        let message = format!("{}", record.args);
        match record.level {
            Level::Error => error!(target: "blvm_lightning::ldk", "{message}"),
            Level::Warn => warn!(target: "blvm_lightning::ldk", "{message}"),
            Level::Info => info!(target: "blvm_lightning::ldk", "{message}"),
            Level::Debug => debug!(target: "blvm_lightning::ldk", "{message}"),
            Level::Trace | Level::Gossip => trace!(target: "blvm_lightning::ldk", "{message}"),
        }
    }
}

struct NullFilter;

impl Filter for NullFilter {
    fn register_tx(&self, _txid: &bitcoin::Txid, _script_pubkey: &bitcoin::Script) {}

    fn register_output(&self, _output: WatchedOutput) {}
}

struct NoUtxo;

impl UtxoLookup for NoUtxo {
    fn get_utxo(
        &self,
        _chain_hash: &bitcoin::constants::ChainHash,
        _short_channel_id: u64,
    ) -> UtxoResult {
        UtxoResult::Sync(Err(UtxoLookupError::UnknownChain))
    }
}

impl LDKProvider {
    async fn collect_funding_coins(
        &self,
        node: &dyn NodeAPI,
        outpoints: &str,
        value_sats: u64,
    ) -> Result<Vec<FundingCoin>, LightningError> {
        let destination = self.keys.get_destination_script([0; 32]).map_err(|()| {
            LightningError::ProcessorError("LDK deposit script is unavailable".to_string())
        })?;
        let tip = node.get_block_height().await?;
        let mut coins = Vec::new();
        let mut total = 0u64;
        for spec in outpoints
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let (txid_hex, vout_text) = spec.rsplit_once(':').ok_or_else(|| {
                LightningError::ProcessorError(format!("outpoint '{spec}' must be txid:vout"))
            })?;
            let internal =
                blvm_node::storage::hashing::hash_from_rpc_hex(txid_hex).map_err(|err| {
                    LightningError::ProcessorError(format!("outpoint '{spec}' txid: {err}"))
                })?;
            let vout = vout_text.parse::<u32>().map_err(|err| {
                LightningError::ProcessorError(format!("outpoint '{spec}' vout: {err}"))
            })?;
            let index = u16::try_from(vout).map_err(|_| {
                LightningError::ProcessorError(format!(
                    "outpoint '{spec}' vout does not fit in u16"
                ))
            })?;
            let utxo = node
                .get_utxo(&blvm_node::OutPoint {
                    hash: internal,
                    index: vout,
                })
                .await?
                .ok_or_else(|| {
                    LightningError::ProcessorError(format!("outpoint '{spec}' is not unspent"))
                })?;
            if utxo.is_coinbase && tip.saturating_sub(utxo.height) < COINBASE_MATURITY {
                return Err(LightningError::ProcessorError(format!(
                    "outpoint '{spec}' is an immature coinbase"
                )));
            }
            if utxo.script_pubkey.as_ref() != destination.as_bytes() {
                return Err(LightningError::ProcessorError(format!(
                    "outpoint '{spec}' does not pay the LDK deposit address"
                )));
            }
            if utxo.value < 0 {
                return Err(LightningError::ProcessorError(format!(
                    "outpoint '{spec}' has a negative value"
                )));
            }
            let value_sat = u64::try_from(utxo.value).map_err(|_| {
                LightningError::ProcessorError(format!("outpoint '{spec}' value does not fit"))
            })?;
            total = total.saturating_add(value_sat);
            coins.push(FundingCoin {
                txid: bitcoin::Txid::from_byte_array(internal),
                vout: index,
                value_sat,
                script: destination.as_bytes().to_vec(),
            });
        }
        if coins.is_empty() {
            return Err(LightningError::ProcessorError(
                "open-channel needs at least one txid:vout that pays the deposit address"
                    .to_string(),
            ));
        }
        if total < value_sats {
            return Err(LightningError::ProcessorError(format!(
                "deposit coins total {total} sats, which is below the {value_sats} sat channel"
            )));
        }
        Ok(coins)
    }

    async fn sync_chain(&self, node: &dyn NodeAPI) -> Result<(), LightningError> {
        let _guard = self.chain_lock.lock().await;
        self.update_fee(node).await;
        let tip_height = node.get_block_height().await?;
        let best = self.manager().current_best_block();
        if best.height == 0 && self.manager().list_channels().is_empty() && tip_height > 0 {
            let tip_hash = node.get_chain_tip().await?;
            let block = self.bitcoin_block_by_hash(node, &tip_hash).await?;
            let best_block = BestBlock::new(block.block_hash(), height_u32(tip_height)?);
            self.reanchor(best_block)?;
            warn!(
                height = tip_height,
                "LDK had no channels and was still at genesis; anchored it at the node tip. Invoices registered before this anchor are gone."
            );
        } else {
            let best_height = u64::from(best.height);
            if tip_height < best_height {
                return Err(LightningError::ProcessorError(format!(
                    "node tip {tip_height} is behind the LDK tip {best_height}"
                )));
            }
            for height in (best_height + 1)..=tip_height {
                let Some(block) = node.get_block_by_height(height).await? else {
                    return Err(LightningError::ProcessorError(format!(
                        "node has no block at height {height}; LDK will not skip it"
                    )));
                };
                let bitcoin_block = self.bitcoin_block_from_parts(node, &block).await?;
                self.connect_bitcoin_block(&bitcoin_block, height_u32(height)?)?;
                if height % 2_000 == 0 {
                    info!(height, "LDK chain sync");
                    self.persist_manager()?;
                }
            }
            if tip_height > best_height {
                self.persist_manager()?;
            }
        }
        self.publish_notices(node, self.drain_events()).await;
        self.flush_broadcasts(node).await;
        Ok(())
    }

    fn reanchor(&self, best_block: BestBlock) -> Result<(), LightningError> {
        let current = self.manager();
        if !current.list_channels().is_empty() || current.current_best_block().height != 0 {
            return Err(LightningError::ProcessorError(
                "refusing to jump an LDK manager that has channels or has left genesis".to_string(),
            ));
        }
        let now = unix_time()?;
        let manager = Arc::new(ChannelManager::new(
            self.fee.clone(),
            self.chain_monitor.clone(),
            self.broadcaster.clone(),
            self.router.clone(),
            self.logger.clone(),
            self.keys.clone(),
            self.keys.clone(),
            self.keys.clone(),
            channel_config(),
            ChainParameters {
                network: self.network,
                best_block,
            },
            now,
        ));
        *self
            .channel_manager
            .write()
            .expect("LDK channel manager lock") = manager;
        self.persist_manager()
    }

    fn connect_bitcoin_block(
        &self,
        block: &bitcoin::Block,
        height: u32,
    ) -> Result<(), LightningError> {
        let best = self.manager().current_best_block();
        if height != best.height.saturating_add(1) {
            return Err(LightningError::ProcessorError(format!(
                "LDK tip is {} and the next block must be height {}",
                best.height,
                best.height.saturating_add(1)
            )));
        }
        if block.header.prev_blockhash != best.block_hash {
            return Err(LightningError::ProcessorError(format!(
                "block at height {height} does not connect to the LDK tip"
            )));
        }
        self.chain_monitor.block_connected(block, height);
        self.manager().block_connected(block, height);
        Ok(())
    }

    async fn bitcoin_block_by_hash(
        &self,
        node: &dyn NodeAPI,
        hash: &[u8; 32],
    ) -> Result<bitcoin::Block, LightningError> {
        let block = match node.get_block(hash).await? {
            Some(block) => block,
            None => {
                let mut reversed = *hash;
                reversed.reverse();
                node.get_block(&reversed).await?.ok_or_else(|| {
                    LightningError::ProcessorError(
                        "node tip hash is not in the block store".to_string(),
                    )
                })?
            }
        };
        self.bitcoin_block_from_parts(node, &block).await
    }

    async fn bitcoin_block_from_parts(
        &self,
        node: &dyn NodeAPI,
        block: &blvm_node::Block,
    ) -> Result<bitcoin::Block, LightningError> {
        let header_bytes = blvm_protocol::serialization::serialize_block_header(&block.header);
        let header: bitcoin::block::Header = bitcoin::consensus::deserialize(&header_bytes)
            .map_err(|err| {
                LightningError::ProcessorError(format!("block header is not bitcoin: {err}"))
            })?;
        let hash = header.block_hash().to_byte_array();
        let mut reversed = hash;
        reversed.reverse();
        let witnesses = if let Some((_, witnesses)) = node.get_block_and_witnesses(&hash).await? {
            witnesses
        } else if let Some((_, witnesses)) = node.get_block_and_witnesses(&reversed).await? {
            witnesses
        } else {
            Vec::new()
        };
        let bytes =
            blvm_protocol::wire::serialize_block_witnesses(block, &witnesses).map_err(|err| {
                LightningError::ProcessorError(format!("failed to serialize block: {err}"))
            })?;
        bitcoin::consensus::deserialize(&bytes).map_err(|err| {
            LightningError::ProcessorError(format!("serialized block is not bitcoin: {err}"))
        })
    }

    async fn update_fee(&self, node: &dyn NodeAPI) {
        match node.get_mempool_fee_rate_summary().await {
            Ok(summary) if summary.median_sat_vb > 0 => {
                let sat_per_kw =
                    u32::try_from(summary.median_sat_vb.saturating_mul(250)).unwrap_or(u32::MAX);
                self.fee.set_sat_per_kw(sat_per_kw);
            }
            Ok(_) => {}
            Err(err) => debug!(%err, "mempool fee summary unavailable; keeping the LDK fee"),
        }
    }

    async fn flush_broadcasts(&self, node: &dyn NodeAPI) {
        flush_broadcaster(self.broadcaster.as_ref(), node).await;
    }

    async fn publish_notices(&self, node: &dyn NodeAPI, notices: Vec<NodeNotice>) {
        for notice in notices {
            let published = match notice {
                NodeNotice::Verified { id, amount_msats } => {
                    node.publish_event(
                        EventType::ModuleEvent,
                        EventPayload::module_event_json(
                            "PaymentVerified",
                            &serde_json::json!({
                                "payment_id": id,
                                "amount_msats": amount_msats,
                                "invoice": "",
                            }),
                        ),
                    )
                    .await
                }
                NodeNotice::Failed { id, reason } => {
                    node.publish_event(
                        EventType::ModuleEvent,
                        EventPayload::module_event_json(
                            "PaymentRouteFailed",
                            &serde_json::json!({
                                "payment_id": id,
                                "reason": reason,
                            }),
                        ),
                    )
                    .await
                }
                NodeNotice::RouteFound {
                    id,
                    hops,
                    cost_msats,
                } => {
                    node.publish_event(
                        EventType::ModuleEvent,
                        EventPayload::module_event_json(
                            "PaymentRouteFound",
                            &serde_json::json!({
                                "payment_id": id,
                                "route_hops": hops,
                                "route_cost_msats": cost_msats,
                            }),
                        ),
                    )
                    .await
                }
                NodeNotice::ChannelClosed { channel_id, reason } => {
                    node.publish_event(
                        EventType::ModuleEvent,
                        EventPayload::module_event_json(
                            "ChannelClosed",
                            &serde_json::json!({
                                "channel_id": channel_id,
                                "reason": reason,
                            }),
                        ),
                    )
                    .await
                }
                NodeNotice::ChannelReady {
                    channel_id,
                    peer_pubkey,
                    capacity_sats,
                } => {
                    node.publish_event(
                        EventType::ModuleEvent,
                        EventPayload::module_event_json(
                            "ChannelOpened",
                            &serde_json::json!({
                                "channel_id": channel_id,
                                "peer_pubkey": peer_pubkey,
                                "capacity_sats": capacity_sats,
                            }),
                        ),
                    )
                    .await
                }
            };
            if let Err(err) = published {
                debug!(%err, "failed to publish an LDK payment event");
            }
        }
    }

    async fn start_peers(&self, node: Arc<dyn NodeAPI>) -> Result<(), LightningError> {
        {
            let existing = self.peer_manager.lock().expect("LDK peer manager");
            if existing.is_some() {
                return Ok(());
            }
        }
        let peers = self.peer_specs.clone();
        let mut dial = Vec::with_capacity(peers.len());
        for spec in &peers {
            dial.push(parse_peer(spec)?);
        }
        let gossip = Arc::new(P2PGossipSync::new(
            self.graph.clone(),
            None::<Arc<NoUtxo>>,
            self.logger.clone(),
        ));
        let now = unix_time()?;
        let ephemeral: [u8; 32] = rand::random();
        let peer_manager = Arc::new(PeerManager::new(
            MessageHandler {
                chan_handler: self.manager(),
                route_handler: gossip,
                onion_message_handler: IgnoringMessageHandler {},
                custom_message_handler: IgnoringMessageHandler {},
            },
            now,
            &ephemeral,
            self.logger.clone(),
            self.keys.clone(),
        ));
        if let Some(port) = self.listen_port {
            let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
                .await
                .map_err(|err| {
                    LightningError::ProcessorError(format!(
                        "failed to bind Lightning listen port {port}: {err}"
                    ))
                })?;
            let accept = Arc::clone(&peer_manager);
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let peers = Arc::clone(&accept);
                            tokio::spawn(async move {
                                if let Ok(std_stream) = stream.into_std() {
                                    lightning_net_tokio::setup_inbound(peers, std_stream).await;
                                }
                            });
                        }
                        Err(err) => {
                            error!(%err, "Lightning accept failed");
                            break;
                        }
                    }
                }
            });
            info!(port, "LDK peer listener bound");
        }
        for (pubkey, addr) in dial {
            let peers = Arc::clone(&peer_manager);
            tokio::spawn(async move {
                if let Some(disconnect) =
                    lightning_net_tokio::connect_outbound(peers, pubkey, addr).await
                {
                    disconnect.await;
                } else {
                    warn!(%pubkey, %addr, "LDK outbound peer connect failed");
                }
            });
        }
        let peers = Arc::clone(&peer_manager);
        // Reanchor has already replaced the manager. The tick keeps that Arc.
        let manager_slot = self.manager();
        let broadcaster = self.broadcaster.clone();
        let keys = self.keys.clone();
        let fee = self.fee.clone();
        let funding = Arc::clone(&self.funding);
        let ldk_dir = self.ldk_dir.clone();
        let confirmed = Arc::clone(&self.confirmed);
        let amounts = Arc::clone(&self.amounts);
        let fees = Arc::clone(&self.fees);
        let graph = Arc::clone(&self.graph);
        let scorer = Arc::clone(&self.scorer);
        let max_inbound = self.max_inbound_sats;
        let stop = Arc::clone(&self.stop);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tick.tick().await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                peers.timer_tick_occurred();
                peers.process_events();
                manager_slot.timer_tick_occurred();
                let notices = drain_manager(
                    &manager_slot,
                    &confirmed,
                    keys.as_ref(),
                    fee.as_ref(),
                    broadcaster.as_ref(),
                    &funding,
                    &ldk_dir,
                    max_inbound,
                    &amounts,
                    &fees,
                );
                publish_notices_detached(node.as_ref(), notices).await;
                flush_broadcaster(&broadcaster, node.as_ref()).await;
                if manager_slot.get_and_clear_needs_persistence() {
                    if let Err(err) = persist_manager_file(&ldk_dir, manager_slot.as_ref()) {
                        error!(%err, "failed to persist LDK channel manager");
                    }
                }
                if let Err(err) = persist_routing(&ldk_dir, graph.as_ref(), &scorer) {
                    error!(%err, "failed to persist LDK routing state");
                }
            }
        });
        *self.peer_manager.lock().expect("LDK peer manager") = Some(peer_manager);
        Ok(())
    }
}

impl Drop for LDKProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn unix_time() -> Result<u32, LightningError> {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            LightningError::ProcessorError(format!("system clock is before unix epoch: {err}"))
        })?;
    Ok(u32::try_from(started.as_secs()).unwrap_or(u32::MAX))
}

fn height_u32(height: u64) -> Result<u32, LightningError> {
    u32::try_from(height).map_err(|_| {
        LightningError::ProcessorError(format!("block height {height} does not fit in u32"))
    })
}

fn parse_peer(spec: &str) -> Result<(PublicKey, SocketAddr), LightningError> {
    let (key, addr) = spec.rsplit_once('@').ok_or_else(|| {
        LightningError::ConfigError(format!(
            "Lightning peer '{spec}' must be node_pubkey@host:port"
        ))
    })?;
    let pubkey = PublicKey::from_str(key).map_err(|err| {
        LightningError::ConfigError(format!(
            "Lightning peer pubkey in '{spec}' is invalid: {err}"
        ))
    })?;
    let addr = addr.parse::<SocketAddr>().map_err(|err| {
        LightningError::ConfigError(format!(
            "Lightning peer address in '{spec}' is invalid: {err}"
        ))
    })?;
    Ok((pubkey, addr))
}

#[allow(clippy::too_many_arguments)]
fn fund_ready_channel(
    manager: &Manager,
    keys: &KeysManager,
    fee: &NodeFeeEstimator,
    funding: &Mutex<HashMap<ChannelId, Vec<FundingCoin>>>,
    ldk_dir: &Path,
    temporary_channel_id: &ChannelId,
    counterparty_node_id: &PublicKey,
    channel_value_satoshis: u64,
    output_script: bitcoin::ScriptBuf,
) {
    let coins = funding
        .lock()
        .expect("LDK funding reservation")
        .remove(temporary_channel_id);
    if coins.is_some() {
        if let Err(err) = save_funding(ldk_dir, funding) {
            error!(%err, "failed to persist the funding reservation after it was spent");
        }
    }
    let Some(coins) = coins else {
        error!(
            channel = %temporary_channel_id,
            "LDK is ready to fund a channel this module did not reserve coins for"
        );
        let _ = manager.close_channel(temporary_channel_id, counterparty_node_id);
        return;
    };
    let height = manager.current_best_block().height;
    let rate = fee.get_est_sat_per_1000_weight(
        lightning::chain::chaininterface::ConfirmationTarget::UrgentOnChainSweep,
    );
    match build_funding_transaction(
        keys,
        &coins,
        channel_value_satoshis,
        output_script,
        rate,
        height,
    ) {
        Ok(tx) => {
            if let Err(err) = manager.funding_transaction_generated(
                *temporary_channel_id,
                *counterparty_node_id,
                tx,
            ) {
                error!(err = ?err, "LDK rejected the funding transaction");
                let _ = manager.close_channel(temporary_channel_id, counterparty_node_id);
            }
        }
        Err(err) => {
            error!(%err, "failed to build the channel funding transaction");
            let _ = manager.close_channel(temporary_channel_id, counterparty_node_id);
        }
    }
}

fn sweep_spendable(
    manager: &Manager,
    keys: &KeysManager,
    fee: &NodeFeeEstimator,
    broadcaster: &QueuedBroadcaster,
    outputs: &[SpendableOutputDescriptor],
) {
    let Ok(destination) = keys.get_destination_script([0; 32]) else {
        return;
    };
    let pending: Vec<&SpendableOutputDescriptor> = outputs
        .iter()
        .filter(|output| match output {
            SpendableOutputDescriptor::StaticOutput { output, .. } => {
                output.script_pubkey != destination
            }
            _ => true,
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let height = manager.current_best_block().height;
    let rate = fee.get_est_sat_per_1000_weight(
        lightning::chain::chaininterface::ConfirmationTarget::UrgentOnChainSweep,
    );
    let locktime = bitcoin::absolute::LockTime::from_height(height).ok();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    match keys.spend_spendable_outputs(&pending, Vec::new(), destination, rate, locktime, &secp) {
        Ok(tx) => broadcaster.broadcast_transactions(&[&tx]),
        Err(()) => error!("LDK could not sweep spendable outputs to the deposit address"),
    }
}

fn build_funding_transaction(
    keys: &KeysManager,
    coins: &[FundingCoin],
    channel_value_satoshis: u64,
    output_script: bitcoin::ScriptBuf,
    feerate_sat_per_1000_weight: u32,
    height: u32,
) -> Result<bitcoin::Transaction, LightningError> {
    let descriptors: Vec<SpendableOutputDescriptor> = coins
        .iter()
        .map(|coin| SpendableOutputDescriptor::StaticOutput {
            outpoint: lightning::chain::transaction::OutPoint {
                txid: coin.txid,
                index: coin.vout,
            },
            output: bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(coin.value_sat),
                script_pubkey: bitcoin::ScriptBuf::from_bytes(coin.script.clone()),
            },
            channel_keys_id: None,
        })
        .collect();
    let refs: Vec<&SpendableOutputDescriptor> = descriptors.iter().collect();
    let change = keys.get_destination_script([0; 32]).map_err(|()| {
        LightningError::ProcessorError("LDK deposit script is unavailable".to_string())
    })?;
    let locktime = bitcoin::absolute::LockTime::from_height(height).ok();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    keys.spend_spendable_outputs(
        &refs,
        vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(channel_value_satoshis),
            script_pubkey: output_script,
        }],
        change,
        feerate_sat_per_1000_weight,
        locktime,
        &secp,
    )
    .map_err(|()| {
        LightningError::ProcessorError(
            "funding coins do not cover the channel value and fee, or they do not pay the LDK deposit address"
                .to_string(),
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn drain_manager(
    manager: &Manager,
    confirmed: &RwLock<HashSet<[u8; 32]>>,
    keys: &KeysManager,
    fee: &NodeFeeEstimator,
    broadcaster: &QueuedBroadcaster,
    funding: &Mutex<HashMap<ChannelId, Vec<FundingCoin>>>,
    ldk_dir: &Path,
    max_inbound_sats: Option<u64>,
    amounts: &Mutex<HashMap<[u8; 32], u64>>,
    fees: &Mutex<HashMap<[u8; 32], u64>>,
) -> Vec<NodeNotice> {
    let mut notices = Vec::new();
    // Claim and forward each batch, then read the events that those calls queued.
    for _ in 0..8 {
        let pending = Mutex::new(Vec::new());
        let handler = |event: Event| -> Result<(), ReplayEvent> {
            // LDK drops these events once this callback returns Ok.
            match &event {
                Event::PaymentClaimable {
                    payment_hash,
                    amount_msat,
                    ..
                }
                | Event::PaymentClaimed {
                    payment_hash,
                    amount_msat,
                    ..
                } => {
                    remember_payment(ldk_dir, confirmed, amounts, payment_hash.0, *amount_msat);
                }
                Event::PaymentSent { payment_hash, .. } => {
                    let amount = amounts
                        .lock()
                        .expect("LDK payment amounts")
                        .get(&payment_hash.0)
                        .copied()
                        .unwrap_or(0);
                    remember_payment(ldk_dir, confirmed, amounts, payment_hash.0, amount);
                }
                _ => {}
            }
            pending.lock().expect("LDK event lock").push(event);
            Ok(())
        };
        manager.process_pending_events(&handler);
        let events = pending.into_inner().unwrap_or_default();
        if events.is_empty() {
            break;
        }
        let mut forward = false;
        for event in events {
            if matches!(event, Event::PendingHTLCsForwardable { .. }) {
                forward = true;
                continue;
            }
            match event {
                Event::PaymentClaimable {
                    payment_hash,
                    purpose,
                    amount_msat,
                    ..
                } => {
                    if let Some(preimage) = preimage_from_purpose(&purpose) {
                        manager.claim_funds(preimage);
                    }
                    remember_payment(ldk_dir, confirmed, amounts, payment_hash.0, amount_msat);
                }
                Event::PaymentClaimed {
                    payment_hash,
                    amount_msat,
                    ..
                } => {
                    remember_payment(ldk_dir, confirmed, amounts, payment_hash.0, amount_msat);
                    notices.push(NodeNotice::Verified {
                        id: hex::encode(payment_hash.0),
                        amount_msats: amount_msat,
                    });
                }
                Event::PaymentSent {
                    payment_hash,
                    fee_paid_msat,
                    ..
                } => {
                    let amount = amounts
                        .lock()
                        .expect("LDK payment amounts")
                        .get(&payment_hash.0)
                        .copied()
                        .unwrap_or(0);
                    fees.lock()
                        .expect("LDK payment fees")
                        .insert(payment_hash.0, fee_paid_msat.unwrap_or(0));
                    remember_payment(ldk_dir, confirmed, amounts, payment_hash.0, amount);
                    notices.push(NodeNotice::Verified {
                        id: hex::encode(payment_hash.0),
                        amount_msats: amount,
                    });
                }
                Event::PaymentPathSuccessful {
                    payment_hash, path, ..
                } => {
                    let Some(payment_hash) = payment_hash else {
                        continue;
                    };
                    let cost_msats = fees
                        .lock()
                        .expect("LDK payment fees")
                        .get(&payment_hash.0)
                        .copied()
                        .unwrap_or(0);
                    notices.push(NodeNotice::RouteFound {
                        id: hex::encode(payment_hash.0),
                        hops: path.hops.len(),
                        cost_msats,
                    });
                }
                Event::PaymentFailed {
                    payment_id,
                    payment_hash,
                    reason,
                } => {
                    let id = payment_hash
                        .map(|hash| hex::encode(hash.0))
                        .unwrap_or_else(|| hex::encode(payment_id.0));
                    notices.push(NodeNotice::Failed {
                        id,
                        reason: format!("{reason:?}"),
                    });
                }
                Event::FundingGenerationReady {
                    temporary_channel_id,
                    counterparty_node_id,
                    channel_value_satoshis,
                    output_script,
                    ..
                } => {
                    fund_ready_channel(
                        manager,
                        keys,
                        fee,
                        funding,
                        ldk_dir,
                        &temporary_channel_id,
                        &counterparty_node_id,
                        channel_value_satoshis,
                        output_script,
                    );
                    if let Err(err) = persist_manager_file(ldk_dir, manager) {
                        error!(%err, "failed to persist LDK channel manager after funding");
                    }
                }
                Event::ChannelReady {
                    channel_id,
                    counterparty_node_id,
                    ..
                } => {
                    let capacity = manager
                        .list_channels()
                        .into_iter()
                        .find(|channel| channel.channel_id == channel_id)
                        .map(|channel| channel.channel_value_satoshis)
                        .unwrap_or(0);
                    notices.push(NodeNotice::ChannelReady {
                        channel_id: hex::encode(channel_id.0),
                        peer_pubkey: counterparty_node_id.serialize().to_vec(),
                        capacity_sats: capacity,
                    });
                }
                Event::SpendableOutputs { outputs, .. } => {
                    sweep_spendable(manager, keys, fee, broadcaster, &outputs);
                }
                Event::OpenChannelRequest {
                    temporary_channel_id,
                    counterparty_node_id,
                    funding_satoshis,
                    ..
                } => {
                    let accept = max_inbound_sats.is_some_and(|cap| funding_satoshis <= cap);
                    if accept {
                        if let Err(err) = manager.accept_inbound_channel(
                            &temporary_channel_id,
                            &counterparty_node_id,
                            0,
                        ) {
                            error!(err = ?err, "LDK rejected an inbound channel accept");
                            let _ = manager.force_close_without_broadcasting_txn(
                                &temporary_channel_id,
                                &counterparty_node_id,
                                "inbound channel was not accepted".into(),
                            );
                        }
                    } else {
                        let _ = manager.force_close_without_broadcasting_txn(
                            &temporary_channel_id,
                            &counterparty_node_id,
                            "inbound channel is above max_inbound_sats".into(),
                        );
                    }
                }
                Event::DiscardFunding { channel_id, .. } => {
                    release_funding(funding, ldk_dir, &channel_id);
                }
                Event::ChannelClosed {
                    channel_id, reason, ..
                } => {
                    release_funding(funding, ldk_dir, &channel_id);
                    notices.push(NodeNotice::ChannelClosed {
                        channel_id: hex::encode(channel_id.0),
                        reason: format!("{reason:?}"),
                    });
                }
                other => {
                    debug!(event = ?std::mem::discriminant(&other), "ignoring LDK event");
                }
            }
        }
        if forward {
            manager.process_pending_htlc_forwards();
        }
    }
    notices
}

async fn publish_notices_detached(node: &dyn NodeAPI, notices: Vec<NodeNotice>) {
    for notice in notices {
        let published = match notice {
            NodeNotice::Verified { id, amount_msats } => {
                node.publish_event(
                    EventType::ModuleEvent,
                    EventPayload::module_event_json(
                        "PaymentVerified",
                        &serde_json::json!({
                            "payment_id": id,
                            "amount_msats": amount_msats,
                            "invoice": "",
                        }),
                    ),
                )
                .await
            }
            NodeNotice::Failed { id, reason } => {
                node.publish_event(
                    EventType::ModuleEvent,
                    EventPayload::module_event_json(
                        "PaymentRouteFailed",
                        &serde_json::json!({
                            "payment_id": id,
                            "reason": reason,
                        }),
                    ),
                )
                .await
            }
            NodeNotice::RouteFound {
                id,
                hops,
                cost_msats,
            } => {
                node.publish_event(
                    EventType::ModuleEvent,
                    EventPayload::module_event_json(
                        "PaymentRouteFound",
                        &serde_json::json!({
                            "payment_id": id,
                            "route_hops": hops,
                            "route_cost_msats": cost_msats,
                        }),
                    ),
                )
                .await
            }
            NodeNotice::ChannelClosed { channel_id, reason } => {
                node.publish_event(
                    EventType::ModuleEvent,
                    EventPayload::module_event_json(
                        "ChannelClosed",
                        &serde_json::json!({
                            "channel_id": channel_id,
                            "reason": reason,
                        }),
                    ),
                )
                .await
            }
            NodeNotice::ChannelReady {
                channel_id,
                peer_pubkey,
                capacity_sats,
            } => {
                node.publish_event(
                    EventType::ModuleEvent,
                    EventPayload::module_event_json(
                        "ChannelOpened",
                        &serde_json::json!({
                            "channel_id": channel_id,
                            "peer_pubkey": peer_pubkey,
                            "capacity_sats": capacity_sats,
                        }),
                    ),
                )
                .await
            }
        };
        if let Err(err) = published {
            debug!(%err, "failed to publish an LDK payment event");
        }
    }
}

async fn flush_broadcaster(broadcaster: &QueuedBroadcaster, node: &dyn NodeAPI) {
    let queued = broadcaster.take_pending();
    let mut retry = Vec::new();
    let mut failed = false;
    for bytes in queued {
        if failed {
            retry.push(bytes);
            continue;
        }
        let parsed = blvm_protocol::serialization::deserialize_transaction_with_witness(&bytes);
        let (tx, witnesses) = match parsed {
            Ok((tx, witnesses, _)) => (tx, witnesses),
            Err(err) => {
                error!(%err, "LDK broadcast is not a transaction this node can submit");
                continue;
            }
        };
        let witnesses = if witnesses.iter().any(|stack| !stack.is_empty()) {
            Some(witnesses)
        } else {
            None
        };
        if let Err(err) = node.submit_mempool_transaction(tx, witnesses).await {
            debug!(%err, "node did not accept an LDK broadcast; leaving it queued");
            retry.push(bytes);
            failed = true;
        }
    }
    broadcaster.requeue_front(retry);
}

fn load_funding(ldk_dir: &Path) -> Result<HashMap<ChannelId, Vec<FundingCoin>>, LightningError> {
    let path = ldk_dir.join("funding.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(io_err(err)),
    };
    let stored: Vec<StoredReservation> = serde_json::from_slice(&bytes).map_err(|err| {
        LightningError::ProcessorError(format!("LDK funding reservations do not decode: {err}"))
    })?;
    let mut map = HashMap::new();
    for row in stored {
        let id = hex::decode(&row.channel_id).map_err(|err| {
            LightningError::ProcessorError(format!("LDK funding channel id: {err}"))
        })?;
        let id: [u8; 32] = id.try_into().map_err(|_| {
            LightningError::ProcessorError("LDK funding channel id is not 32 bytes".to_string())
        })?;
        let mut coins = Vec::new();
        for coin in row.coins {
            let txid = bitcoin::Txid::from_str(&coin.txid).map_err(|err| {
                LightningError::ProcessorError(format!("LDK funding txid: {err}"))
            })?;
            let script = hex::decode(&coin.script).map_err(|err| {
                LightningError::ProcessorError(format!("LDK funding script: {err}"))
            })?;
            coins.push(FundingCoin {
                txid,
                vout: coin.vout,
                value_sat: coin.value_sat,
                script,
            });
        }
        map.insert(ChannelId(id), coins);
    }
    Ok(map)
}

fn save_funding(
    ldk_dir: &Path,
    funding: &Mutex<HashMap<ChannelId, Vec<FundingCoin>>>,
) -> Result<(), LightningError> {
    let map = funding.lock().expect("LDK funding reservation").clone();
    let stored: Vec<StoredReservation> = map
        .into_iter()
        .map(|(id, coins)| StoredReservation {
            channel_id: hex::encode(id.0),
            coins: coins
                .into_iter()
                .map(|coin| StoredCoin {
                    txid: coin.txid.to_string(),
                    vout: coin.vout,
                    value_sat: coin.value_sat,
                    script: hex::encode(coin.script),
                })
                .collect(),
        })
        .collect();
    let bytes = serde_json::to_vec(&stored).map_err(|err| {
        LightningError::ProcessorError(format!("LDK funding reservations do not encode: {err}"))
    })?;
    let path = ldk_dir.join("funding.json");
    let tmp = ldk_dir.join("funding.json.tmp");
    let mut file = File::create(&tmp).map_err(io_err)?;
    file.write_all(&bytes).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(())
}

fn channel_config() -> UserConfig {
    UserConfig {
        manually_accept_inbound_channels: true,
        ..UserConfig::default()
    }
}

fn load_graph(
    ldk_dir: &Path,
    network: Network,
    logger: Log,
) -> Result<NetworkGraph<Log>, LightningError> {
    let path = ldk_dir.join("network_graph");
    if !path.exists() {
        return Ok(NetworkGraph::new(network, logger));
    }
    let mut file = File::open(&path).map_err(io_err)?;
    NetworkGraph::read(&mut file, logger).map_err(|err| {
        LightningError::ProcessorError(format!("LDK network graph does not decode: {err}"))
    })
}

fn load_scorer(ldk_dir: &Path, graph: &Graph, logger: &Log) -> Result<Score, LightningError> {
    let path = ldk_dir.join("scorer");
    let decay = ProbabilisticScoringDecayParameters::default();
    if !path.exists() {
        return Ok(ProbabilisticScorer::new(
            decay,
            Arc::clone(graph),
            logger.clone(),
        ));
    }
    let mut file = File::open(&path).map_err(io_err)?;
    ProbabilisticScorer::read(&mut file, (decay, Arc::clone(graph), logger.clone()))
        .map_err(|err| LightningError::ProcessorError(format!("LDK scorer does not decode: {err}")))
}

fn persist_routing(
    ldk_dir: &Path,
    graph: &NetworkGraph<Log>,
    scorer: &RwLock<Score>,
) -> Result<(), LightningError> {
    write_ldk_file(ldk_dir, "network_graph", graph)?;
    let guard = scorer.read().expect("LDK scorer");
    write_ldk_file(ldk_dir, "scorer", &*guard)?;
    Ok(())
}

fn write_ldk_file(
    ldk_dir: &Path,
    name: &str,
    value: &impl Writeable,
) -> Result<(), LightningError> {
    let tmp = ldk_dir.join(format!("{name}.tmp"));
    let path = ldk_dir.join(name);
    let mut file = File::create(&tmp).map_err(io_err)?;
    value
        .write(&mut file)
        .map_err(|err| LightningError::ProcessorError(err.to_string()))?;
    file.sync_all().map_err(io_err)?;
    fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredPayment {
    hash: String,
    amount_msat: u64,
}

type ConfirmedPayments = (HashSet<[u8; 32]>, HashMap<[u8; 32], u64>);

fn load_payments(ldk_dir: &Path) -> Result<ConfirmedPayments, LightningError> {
    let path = ldk_dir.join("payments.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((HashSet::new(), HashMap::new()));
        }
        Err(err) => return Err(io_err(err)),
    };
    let stored: Vec<StoredPayment> = serde_json::from_slice(&bytes).map_err(|err| {
        LightningError::ProcessorError(format!("LDK payments do not decode: {err}"))
    })?;
    let mut confirmed = HashSet::new();
    let mut amounts = HashMap::new();
    for row in stored {
        let bytes = hex::decode(&row.hash)
            .map_err(|err| LightningError::ProcessorError(format!("LDK payment hash: {err}")))?;
        let hash: [u8; 32] = bytes.try_into().map_err(|_| {
            LightningError::ProcessorError("LDK payment hash is not 32 bytes".to_string())
        })?;
        confirmed.insert(hash);
        amounts.insert(hash, row.amount_msat);
    }
    Ok((confirmed, amounts))
}

fn save_payments(
    ldk_dir: &Path,
    confirmed: &HashSet<[u8; 32]>,
    amounts: &HashMap<[u8; 32], u64>,
) -> Result<(), LightningError> {
    let stored: Vec<StoredPayment> = confirmed
        .iter()
        .map(|hash| StoredPayment {
            hash: hex::encode(hash),
            amount_msat: amounts.get(hash).copied().unwrap_or(0),
        })
        .collect();
    let bytes = serde_json::to_vec(&stored).map_err(|err| {
        LightningError::ProcessorError(format!("LDK payments do not encode: {err}"))
    })?;
    let path = ldk_dir.join("payments.json");
    let tmp = ldk_dir.join("payments.json.tmp");
    let mut file = File::create(&tmp).map_err(io_err)?;
    file.write_all(&bytes).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(())
}

fn remember_payment(
    ldk_dir: &Path,
    confirmed: &RwLock<HashSet<[u8; 32]>>,
    amounts: &Mutex<HashMap<[u8; 32], u64>>,
    hash: [u8; 32],
    amount_msat: u64,
) {
    if let Ok(mut set) = confirmed.write() {
        set.insert(hash);
    }
    if let Ok(mut map) = amounts.lock() {
        map.insert(hash, amount_msat);
    }
    let set = confirmed.read().expect("LDK payments").clone();
    let map = amounts.lock().expect("LDK payment amounts").clone();
    if let Err(err) = save_payments(ldk_dir, &set, &map) {
        error!(%err, "failed to persist LDK payments");
    }
}

fn release_funding(
    funding: &Mutex<HashMap<ChannelId, Vec<FundingCoin>>>,
    ldk_dir: &Path,
    id: &ChannelId,
) {
    let removed = funding
        .lock()
        .expect("LDK funding reservation")
        .remove(id)
        .is_some();
    if removed {
        if let Err(err) = save_funding(ldk_dir, funding) {
            error!(%err, "failed to persist funding.json after releasing a reservation");
        }
    }
}

fn persist_manager_file(ldk_dir: &Path, manager: &Manager) -> Result<(), LightningError> {
    let path = ldk_dir.join("channel_manager");
    let tmp = ldk_dir.join("channel_manager.tmp");
    let mut file = File::create(&tmp).map_err(io_err)?;
    manager
        .write(&mut file)
        .map_err(|err| LightningError::ProcessorError(err.to_string()))?;
    file.sync_all().map_err(io_err)?;
    fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(())
}

struct DiskStore {
    dir: PathBuf,
}

impl DiskStore {
    fn monitor_path(&self, outpoint: &lightning::chain::transaction::OutPoint) -> PathBuf {
        self.dir
            .join(format!("{}-{}.monitor", outpoint.txid, outpoint.index))
    }

    fn load_monitors(
        &self,
        keys: &KeysManager,
    ) -> Result<Vec<ChannelMonitor<InMemorySigner>>, LightningError> {
        let mut monitors = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !name.ends_with(".monitor") {
                continue;
            }
            let mut file = File::open(&path).map_err(io_err)?;
            let (_hash, monitor) = <(bitcoin::BlockHash, ChannelMonitor<InMemorySigner>)>::read(
                &mut file,
                (keys, keys),
            )
            .map_err(|e| {
                LightningError::ProcessorError(format!("failed to read {}: {e:?}", path.display()))
            })?;
            monitors.push(monitor);
        }
        Ok(monitors)
    }

    fn write_monitor(
        &self,
        outpoint: lightning::chain::transaction::OutPoint,
        monitor: &ChannelMonitor<InMemorySigner>,
    ) -> ChannelMonitorUpdateStatus {
        let path = self.monitor_path(&outpoint);
        let tmp = path.with_extension("tmp");
        let write = (|| {
            let mut file = File::create(&tmp)?;
            monitor
                .write(&mut file)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            file.sync_all()?;
            fs::rename(&tmp, &path)?;
            Ok::<(), std::io::Error>(())
        })();
        match write {
            Ok(()) => ChannelMonitorUpdateStatus::Completed,
            Err(err) => {
                error!(%err, path = %path.display(), "failed to persist channel monitor");
                ChannelMonitorUpdateStatus::UnrecoverableError
            }
        }
    }
}

impl lightning::chain::chainmonitor::Persist<InMemorySigner> for DiskStore {
    fn persist_new_channel(
        &self,
        channel_funding_outpoint: lightning::chain::transaction::OutPoint,
        monitor: &ChannelMonitor<InMemorySigner>,
    ) -> ChannelMonitorUpdateStatus {
        self.write_monitor(channel_funding_outpoint, monitor)
    }

    fn update_persisted_channel(
        &self,
        channel_funding_outpoint: lightning::chain::transaction::OutPoint,
        _monitor_update: Option<&ChannelMonitorUpdate>,
        monitor: &ChannelMonitor<InMemorySigner>,
    ) -> ChannelMonitorUpdateStatus {
        self.write_monitor(channel_funding_outpoint, monitor)
    }

    fn archive_persisted_channel(
        &self,
        channel_funding_outpoint: lightning::chain::transaction::OutPoint,
    ) {
        let path = self.monitor_path(&channel_funding_outpoint);
        let archived = PathBuf::from(format!("{}.archived", path.display()));
        if let Err(err) = fs::rename(&path, &archived) {
            if err.kind() != std::io::ErrorKind::NotFound {
                error!(%err, path = %path.display(), "failed to archive channel monitor");
            }
        }
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    fn open() -> (tempfile::TempDir, LDKProvider) {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = LDKProvider::new(LDKConfig {
            data_dir: dir.path().to_path_buf(),
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: Vec::new(),
            max_inbound_sats: None,
        })
        .expect("provider");
        (dir, provider)
    }

    #[test]
    fn connecting_the_next_block_advances_the_tip() {
        let (_dir, provider) = open();
        let genesis = genesis_block(Network::Regtest);
        let best = provider.manager().current_best_block();
        assert_eq!(best.height, 0);
        assert_eq!(best.block_hash, genesis.block_hash());

        let header = bitcoin::block::Header {
            version: genesis.header.version,
            prev_blockhash: best.block_hash,
            merkle_root: genesis.header.merkle_root,
            time: genesis.header.time.saturating_add(1),
            bits: genesis.header.bits,
            nonce: 0,
        };
        let block = bitcoin::Block {
            header,
            txdata: vec![genesis.txdata[0].clone()],
        };
        provider
            .connect_bitcoin_block(&block, 1)
            .expect("block connects");
        let next = provider.manager().current_best_block();
        assert_eq!(next.height, 1);
        assert_eq!(next.block_hash, block.block_hash());
        assert!(provider.connect_bitcoin_block(&block, 1).is_err());
    }

    #[test]
    fn reanchor_moves_an_empty_genesis_manager_and_then_refuses() {
        let (_dir, provider) = open();
        let mut bytes = genesis_block(Network::Regtest).block_hash().to_byte_array();
        bytes[0] ^= 0xff;
        let tip = bitcoin::BlockHash::from_byte_array(bytes);
        provider
            .reanchor(BestBlock::new(tip, 50))
            .expect("reanchor from genesis");
        let best = provider.manager().current_best_block();
        assert_eq!(best.height, 50);
        assert_eq!(best.block_hash, tip);
        assert!(provider.reanchor(BestBlock::new(tip, 60)).is_err());
    }

    #[test]
    fn funding_reservation_is_still_there_after_reopen() {
        let (dir, provider) = open();
        let id = ChannelId([7u8; 32]);
        provider.funding.lock().expect("reservation").insert(
            id,
            vec![FundingCoin {
                txid: bitcoin::Txid::from_byte_array([1u8; 32]),
                vout: 1,
                value_sat: 50_000,
                script: vec![0x00, 0x14],
            }],
        );
        save_funding(&provider.ldk_dir, &provider.funding).expect("save");
        drop(provider);
        let reopened = LDKProvider::new(LDKConfig {
            data_dir: dir.path().to_path_buf(),
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: Vec::new(),
            max_inbound_sats: None,
        })
        .expect("reopen");
        let coins = reopened
            .funding
            .lock()
            .expect("reservation")
            .get(&id)
            .cloned()
            .expect("stored reservation");
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].vout, 1);
        assert_eq!(coins[0].value_sat, 50_000);
        assert_eq!(coins[0].script, vec![0x00, 0x14]);
    }

    #[test]
    fn deposit_address_is_regtest_and_can_fund_a_channel_output() {
        let (_dir, provider) = open();
        let address = provider
            .keys
            .get_destination_script([0; 32])
            .expect("destination");
        let shown = bitcoin::Address::from_script(&address, Network::Regtest)
            .expect("address")
            .to_string();
        assert!(
            shown.starts_with("bcrt"),
            "regtest deposit address, got {shown}"
        );

        let coin = FundingCoin {
            txid: bitcoin::Txid::from_byte_array([7; 32]),
            vout: 0,
            value_sat: 150_000,
            script: address.as_bytes().to_vec(),
        };
        let funding_script = bitcoin::ScriptBuf::from_bytes(vec![
            0x00, 0x14, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
            0xdd, 0xee, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50,
        ]);
        let tx = build_funding_transaction(
            provider.keys.as_ref(),
            &[coin],
            50_000,
            funding_script.clone(),
            1_000,
            100,
        )
        .expect("funding transaction");
        assert!(
            tx.output
                .iter()
                .any(|output| output.value.to_sat() == 50_000
                    && output.script_pubkey == funding_script),
            "funding output is present"
        );
        assert!(tx.input[0].witness.len() >= 2, "deposit input is signed");
    }

    #[test]
    fn inbound_channels_are_manual_and_a_payment_survives_reopen() {
        assert!(channel_config().manually_accept_inbound_channels);
        let (dir, provider) = open();
        let hash = [3u8; 32];
        remember_payment(
            &provider.ldk_dir,
            &provider.confirmed,
            &provider.amounts,
            hash,
            42,
        );
        drop(provider);
        let reopened = LDKProvider::new(LDKConfig {
            data_dir: dir.path().to_path_buf(),
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: Vec::new(),
            max_inbound_sats: None,
        })
        .expect("reopen");
        assert!(reopened.is_confirmed(&hash));
    }

    #[tokio::test]
    async fn two_nodes_open_a_channel_pay_and_remember_it() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("port")
            .local_addr()
            .expect("addr")
            .port();
        let recv_dir = tempfile::tempdir().expect("tempdir");
        let open_dir = tempfile::tempdir().expect("tempdir");
        let receiver = LDKProvider::new(LDKConfig {
            data_dir: recv_dir.path().to_path_buf(),
            network: "regtest".into(),
            node_private_key: None,
            listen_port: Some(port),
            peers: Vec::new(),
            max_inbound_sats: Some(200_000),
        })
        .expect("receiver");
        let recv_id = receiver.manager().get_our_node_id();
        let opener = LDKProvider::new(LDKConfig {
            data_dir: open_dir.path().to_path_buf(),
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: vec![format!("{recv_id}@127.0.0.1:{port}")],
            max_inbound_sats: None,
        })
        .expect("opener");
        let node = Arc::new(crate::provider::chain_node::ChainNode::new());
        let script = opener
            .keys
            .get_destination_script([0; 32])
            .expect("destination");
        let txid = bitcoin::Txid::from_byte_array([9u8; 32]);
        node.utxos.lock().expect("utxos").insert(
            (txid.to_byte_array(), 0),
            blvm_node::UTXO {
                value: 250_000,
                script_pubkey: script.as_bytes().to_vec().into(),
                height: 0,
                is_coinbase: false,
            },
        );
        receiver
            .attach_node(Arc::clone(&node) as Arc<dyn NodeAPI>)
            .await
            .expect("receiver attach");
        opener
            .attach_node(Arc::clone(&node) as Arc<dyn NodeAPI>)
            .await
            .expect("opener attach");
        let mut linked = false;
        for _ in 0..50 {
            if opener.connected_peers() >= 1 && receiver.connected_peers() >= 1 {
                linked = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(linked, "peers did not connect");
        let mut opened = None;
        for _ in 0..25 {
            match opener
                .open_channel(
                    node.as_ref(),
                    &recv_id.to_string(),
                    100_000,
                    &format!("{txid}:0"),
                )
                .await
            {
                Ok(id) => {
                    opened = Some(id);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
        let _opened = opened.expect("open-channel");
        let mut funding = None;
        for _ in 0..50 {
            if let Some(tx) = funding_tx_from_log(&opener.ldk_dir) {
                funding = Some(tx);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let funding = funding.expect("funding transaction");
        confirm_funding(&opener, &receiver, funding);
        let mut usable = false;
        for _ in 0..75 {
            let open_ready = opener
                .list_channels()
                .await
                .expect("opener channels")
                .iter()
                .any(|channel| channel.state == "usable");
            let recv_ready = receiver
                .list_channels()
                .await
                .expect("receiver channels")
                .iter()
                .any(|channel| channel.state == "usable");
            if open_ready && recv_ready {
                usable = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(usable, "channel did not become usable");
        let invoice = receiver
            .create_invoice(10_000, "direct", 3_600)
            .await
            .expect("invoice");
        opener.pay_invoice(&invoice).await.expect("pay");
        let mut paid = false;
        let mut payment_hash = [0u8; 32];
        for _ in 0..75 {
            let _ = receiver.drain_events();
            if let Ok(parsed) = invoice.parse::<Bolt11Invoice>() {
                payment_hash = parsed.payment_hash().to_byte_array();
                if receiver.is_confirmed(&payment_hash) {
                    paid = true;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(paid, "receiver did not confirm the payment");
        let mut sender_done = false;
        for _ in 0..75 {
            let _ = opener.drain_events();
            if opener.is_confirmed(&payment_hash) {
                sender_done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(sender_done, "opener did not record the sent payment");
        let channel_id = opener
            .list_channels()
            .await
            .expect("opener channels")
            .into_iter()
            .next()
            .expect("open channel")
            .channel_id;
        opener.close_channel(&channel_id).await.expect("close");
        let mut closed = false;
        for _ in 0..75 {
            let _ = opener.drain_events();
            let _ = receiver.drain_events();
            let open_left = opener.list_channels().await.expect("opener channels");
            let recv_left = receiver.list_channels().await.expect("receiver channels");
            if open_left.is_empty() && recv_left.is_empty() {
                closed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert!(closed, "channel did not close");
        let recv_path = recv_dir.path().to_path_buf();
        let open_path = open_dir.path().to_path_buf();
        drop(receiver);
        drop(opener);
        tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
        let receiver = LDKProvider::new(LDKConfig {
            data_dir: recv_path,
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: Vec::new(),
            max_inbound_sats: Some(200_000),
        })
        .expect("reopen receiver");
        assert!(receiver.is_confirmed(&payment_hash));
        let opener = LDKProvider::new(LDKConfig {
            data_dir: open_path,
            network: "regtest".into(),
            node_private_key: None,
            listen_port: None,
            peers: Vec::new(),
            max_inbound_sats: None,
        })
        .expect("reopen opener");
        let funding_file = opener.ldk_dir.join("funding.json");
        if funding_file.exists() {
            let text = fs::read_to_string(&funding_file).expect("funding file");
            assert_eq!(text, "[]", "funding reservation remained: {text}");
        }
    }

    fn funding_tx_from_log(ldk_dir: &Path) -> Option<bitcoin::Transaction> {
        let text = fs::read_to_string(ldk_dir.join("broadcasts.log")).ok()?;
        let line = text.lines().rev().find(|line| !line.is_empty())?;
        let bytes = hex::decode(line).ok()?;
        bitcoin::consensus::deserialize(&bytes).ok()
    }

    fn confirm_funding(
        opener: &LDKProvider,
        receiver: &LDKProvider,
        funding: bitcoin::Transaction,
    ) {
        let mut prev = opener.manager().current_best_block().block_hash;
        let mut height = opener.manager().current_best_block().height;
        for step in 1u8..=6 {
            let mut txs = vec![coinbase_tx(step)];
            if step == 1 {
                txs.push(funding.clone());
            }
            let block = mine_block(prev, txs);
            height = height.saturating_add(1);
            opener
                .connect_bitcoin_block(&block, height)
                .expect("opener block");
            receiver
                .connect_bitcoin_block(&block, height)
                .expect("receiver block");
            prev = block.block_hash();
        }
    }

    fn coinbase_tx(marker: u8) -> bitcoin::Transaction {
        bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::null(),
                script_sig: bitcoin::ScriptBuf::from_bytes(vec![marker]),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::default(),
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(50 * 100_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        }
    }

    fn mine_block(prev: bitcoin::BlockHash, txdata: Vec<bitcoin::Transaction>) -> bitcoin::Block {
        let genesis = genesis_block(Network::Regtest);
        let mut block = bitcoin::Block {
            header: bitcoin::block::Header {
                version: genesis.header.version,
                prev_blockhash: prev,
                merkle_root: genesis.header.merkle_root,
                time: genesis
                    .header
                    .time
                    .saturating_add(u32::from(txdata.len() as u16)),
                bits: genesis.header.bits,
                nonce: 0,
            },
            txdata,
        };
        block.header.merkle_root = block.compute_merkle_root().expect("merkle root");
        block
    }
}
