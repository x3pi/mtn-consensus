// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::BTreeMap,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
use consensus_config::{AuthorityIndex, NetworkKeyPair, NetworkPublicKey};
use consensus_types::block::{BlockRef, Round};
use futures::{stream, Stream, StreamExt as _};
use meta_http::ServerHandle;
use meta_tls::AllowPublicKeys;
use mysten_network::{
    callback::{CallbackLayer, MakeCallbackHandler, ResponseHandler},
    multiaddr::Protocol,
    Multiaddr,
};
use parking_lot::RwLock;
use tokio_stream::{iter, Iter};
use tonic::{codec::CompressionEncoding, Request, Response, Streaming};
use tower_http::trace::{DefaultMakeSpan, DefaultOnFailure, TraceLayer};
use tracing::{debug, info, trace, warn};

use super::{
    metrics_layer::{MetricsCallbackMaker, MetricsResponseCallback, SizedRequest, SizedResponse},
    tonic_gen::{
        consensus_service_client::ConsensusServiceClient,
        consensus_service_server::ConsensusService,
    },
    BlockStream, ExtendedSerializedBlock, NetworkClient, NetworkManager, NetworkService,
};
use crate::{
    commit::CommitRange,
    context::Context,
    epoch_change::{EpochChangeProposal, EpochChangeVote},
    error::{ConsensusError, ConsensusResult},
    network::{
        tonic_gen::consensus_service_server::ConsensusServiceServer,
        tonic_tls::certificate_server_name,
    },
    CommitIndex,
};

// Maximum bytes size in a single fetch_blocks()response.
// TODO: put max RPC response size in protocol config.
const MAX_FETCH_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

// Maximum total bytes fetched in a single fetch_blocks() call, after combining the responses.
const MAX_TOTAL_FETCHED_BYTES: usize = 128 * 1024 * 1024;

const DEFAULT_GRPC_SERVER_TIMEOUT: Duration = Duration::from_secs(300);

// Implements Tonic RPC client for Consensus.
pub struct TonicClient {
    context: Arc<Context>,
    network_keypair: NetworkKeyPair,
    channel_pool: Arc<ChannelPool>,
}

impl TonicClient {
    pub fn new(context: Arc<Context>, network_keypair: NetworkKeyPair) -> Self {
        Self {
            context: context.clone(),
            network_keypair,
            channel_pool: Arc::new(ChannelPool::new(context)),
        }
    }

    async fn get_client(
        &self,
        peer: AuthorityIndex,
        timeout: Duration,
    ) -> ConsensusResult<ConsensusServiceClient<Channel>> {
        let config = &self.context.parameters.tonic;
        let channel = self
            .channel_pool
            .get_channel(self.network_keypair.clone(), peer, timeout)
            .await?;
        let client = ConsensusServiceClient::new(channel)
            .max_encoding_message_size(config.message_size_limit)
            .max_decoding_message_size(config.message_size_limit)
            .send_compressed(CompressionEncoding::Zstd)
            .accept_compressed(CompressionEncoding::Zstd);
        Ok(client)
    }
}

// TODO: make sure callsites do not send request to own index, and return error otherwise.
#[async_trait]
impl NetworkClient for TonicClient {
    async fn subscribe_blocks(
        &self,
        peer: AuthorityIndex,
        last_received: Round,
        timeout: Duration,
    ) -> ConsensusResult<BlockStream> {
        let mut client = self.get_client(peer, timeout).await?;
        // TODO: add sampled block acknowledgments for latency measurements.
        let request = Request::new(stream::once(async move {
            SubscribeBlocksRequest {
                last_received_round: last_received,
            }
        }));
        let response = client.subscribe_blocks(request).await.map_err(|e| {
            ConsensusError::NetworkRequest(format!("subscribe_blocks failed: {e:?}"))
        })?;
        let stream = response
            .into_inner()
            .take_while(|b| futures::future::ready(b.is_ok()))
            .filter_map(move |b| async move {
                match b {
                    Ok(response) => Some(ExtendedSerializedBlock {
                        block: response.block,
                        excluded_ancestors: response.excluded_ancestors,
                    }),
                    Err(e) => {
                        debug!("Network error received from {}: {e:?}", peer);
                        None
                    }
                }
            });
        let rate_limited_stream =
            tokio_stream::StreamExt::throttle(stream, self.context.parameters.min_round_delay / 2)
                .boxed();
        Ok(rate_limited_stream)
    }

    async fn fetch_blocks(
        &self,
        peer: AuthorityIndex,
        block_refs: Vec<BlockRef>,
        highest_accepted_rounds: Vec<Round>,
        breadth_first: bool,
        timeout: Duration,
    ) -> ConsensusResult<Vec<Bytes>> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(FetchBlocksRequest {
            block_refs: block_refs
                .iter()
                .filter_map(|r| match bcs::to_bytes(r) {
                    Ok(serialized) => Some(serialized),
                    Err(e) => {
                        debug!("Failed to serialize block ref {:?}: {e:?}", r);
                        None
                    }
                })
                .collect(),
            highest_accepted_rounds,
            breadth_first,
        });
        request.set_timeout(timeout);
        let mut stream = client
            .fetch_blocks(request)
            .await
            .map_err(|e| {
                if e.code() == tonic::Code::DeadlineExceeded {
                    ConsensusError::NetworkRequestTimeout(format!("fetch_blocks failed: {e:?}"))
                } else {
                    ConsensusError::NetworkRequest(format!("fetch_blocks failed: {e:?}"))
                }
            })?
            .into_inner();
        let mut blocks = vec![];
        let mut total_fetched_bytes = 0;
        loop {
            match stream.message().await {
                Ok(Some(response)) => {
                    for b in &response.blocks {
                        total_fetched_bytes += b.len();
                    }
                    blocks.extend(response.blocks);
                    if total_fetched_bytes > MAX_TOTAL_FETCHED_BYTES {
                        info!(
                            "fetch_blocks() fetched bytes exceeded limit: {} > {}, terminating stream.",
                            total_fetched_bytes, MAX_TOTAL_FETCHED_BYTES,
                        );
                        break;
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(e) => {
                    if blocks.is_empty() {
                        if e.code() == tonic::Code::DeadlineExceeded {
                            return Err(ConsensusError::NetworkRequestTimeout(format!(
                                "fetch_blocks failed mid-stream: {e:?}"
                            )));
                        }
                        return Err(ConsensusError::NetworkRequest(format!(
                            "fetch_blocks failed mid-stream: {e:?}"
                        )));
                    } else {
                        warn!("fetch_blocks failed mid-stream: {e:?}");
                        break;
                    }
                }
            }
        }
        Ok(blocks)
    }

    async fn fetch_commits(
        &self,
        peer: AuthorityIndex,
        commit_range: CommitRange,
        timeout: Duration,
    ) -> ConsensusResult<(Vec<Bytes>, Vec<Bytes>)> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(FetchCommitsRequest {
            start: commit_range.start(),
            end: commit_range.end(),
        });
        request.set_timeout(timeout);
        let response = client
            .fetch_commits(request)
            .await
            .map_err(|e| ConsensusError::NetworkRequest(format!("fetch_commits failed: {e:?}")))?;
        let response = response.into_inner();
        Ok((response.commits, response.certifier_blocks))
    }

    async fn fetch_commits_by_global_range(
        &self,
        peer: AuthorityIndex,
        start_global_index: u64,
        end_global_index: u64,
        timeout: Duration,
    ) -> ConsensusResult<Vec<GlobalCommitInfo>> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(FetchCommitsByGlobalRangeRequest {
            start_global_index,
            end_global_index,
        });
        request.set_timeout(timeout);
        let response = client
            .fetch_commits_by_global_range(request)
            .await
            .map_err(|e| {
                ConsensusError::NetworkRequest(format!(
                    "fetch_commits_by_global_range failed: {e:?}"
                ))
            })?;
        Ok(response.into_inner().commits)
    }

    async fn fetch_latest_blocks(
        &self,
        peer: AuthorityIndex,
        authorities: Vec<AuthorityIndex>,
        timeout: Duration,
    ) -> ConsensusResult<Vec<Bytes>> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(FetchLatestBlocksRequest {
            authorities: authorities
                .iter()
                .map(|authority| authority.value() as u32)
                .collect(),
        });
        request.set_timeout(timeout);
        let mut stream = client
            .fetch_latest_blocks(request)
            .await
            .map_err(|e| {
                if e.code() == tonic::Code::DeadlineExceeded {
                    ConsensusError::NetworkRequestTimeout(format!("fetch_blocks failed: {e:?}"))
                } else {
                    ConsensusError::NetworkRequest(format!("fetch_blocks failed: {e:?}"))
                }
            })?
            .into_inner();
        let mut blocks = vec![];
        let mut total_fetched_bytes = 0;
        loop {
            match stream.message().await {
                Ok(Some(response)) => {
                    for b in &response.blocks {
                        total_fetched_bytes += b.len();
                    }
                    blocks.extend(response.blocks);
                    if total_fetched_bytes > MAX_TOTAL_FETCHED_BYTES {
                        info!(
                            "fetch_blocks() fetched bytes exceeded limit: {} > {}, terminating stream.",
                            total_fetched_bytes, MAX_TOTAL_FETCHED_BYTES,
                        );
                        break;
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(e) => {
                    if blocks.is_empty() {
                        if e.code() == tonic::Code::DeadlineExceeded {
                            return Err(ConsensusError::NetworkRequestTimeout(format!(
                                "fetch_blocks failed mid-stream: {e:?}"
                            )));
                        }
                        return Err(ConsensusError::NetworkRequest(format!(
                            "fetch_blocks failed mid-stream: {e:?}"
                        )));
                    } else {
                        warn!("fetch_latest_blocks failed mid-stream: {e:?}");
                        break;
                    }
                }
            }
        }
        Ok(blocks)
    }

    async fn get_latest_rounds(
        &self,
        peer: AuthorityIndex,
        timeout: Duration,
    ) -> ConsensusResult<(Vec<Round>, Vec<Round>)> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(GetLatestRoundsRequest {});
        request.set_timeout(timeout);
        let response = client.get_latest_rounds(request).await.map_err(|e| {
            ConsensusError::NetworkRequest(format!("get_latest_rounds failed: {e:?}"))
        })?;
        let response = response.into_inner();
        Ok((response.highest_received, response.highest_accepted))
    }

    async fn send_epoch_change_proposal(
        &self,
        peer: AuthorityIndex,
        proposal: &EpochChangeProposal,
        timeout: Duration,
    ) -> ConsensusResult<()> {
        let mut client = self.get_client(peer, timeout).await?;
        let proposal_bytes = bcs::to_bytes(proposal).map_err(|e| {
            ConsensusError::NetworkRequest(format!("serialize proposal failed: {e:?}"))
        })?;
        let mut request = Request::new(SendEpochChangeProposalRequest {
            proposal: proposal_bytes.into(),
        });
        request.set_timeout(timeout);
        client
            .send_epoch_change_proposal(request)
            .await
            .map_err(|e| {
                ConsensusError::NetworkRequest(format!("send_epoch_change_proposal failed: {e:?}"))
            })?;
        Ok(())
    }

    async fn send_epoch_change_vote(
        &self,
        peer: AuthorityIndex,
        vote: &EpochChangeVote,
        timeout: Duration,
    ) -> ConsensusResult<()> {
        let mut client = self.get_client(peer, timeout).await?;
        let vote_bytes = bcs::to_bytes(vote)
            .map_err(|e| ConsensusError::NetworkRequest(format!("serialize vote failed: {e:?}")))?;
        let mut request = Request::new(SendEpochChangeVoteRequest {
            vote: vote_bytes.into(),
        });
        request.set_timeout(timeout);
        client.send_epoch_change_vote(request).await.map_err(|e| {
            ConsensusError::NetworkRequest(format!("send_epoch_change_vote failed: {e:?}"))
        })?;
        Ok(())
    }

    #[cfg(test)]
    async fn send_block(
        &self,
        peer: AuthorityIndex,
        block: &crate::VerifiedBlock,
        timeout: Duration,
    ) -> ConsensusResult<()> {
        let mut client = self.get_client(peer, timeout).await?;
        let mut request = Request::new(SendBlockRequest {
            block: block.serialized().clone(),
        });
        request.set_timeout(timeout);
        client
            .send_block(request)
            .await
            .map_err(|e| ConsensusError::NetworkRequest(format!("send_block failed: {e:?}")))?;
        Ok(())
    }
}

// Tonic channel wrapped with layers - using plain TCP
type Channel = mysten_network::callback::Callback<
    tower_http::trace::Trace<
        tonic::transport::Channel,
        tower_http::classify::SharedClassifier<tower_http::classify::GrpcErrorsAsFailures>,
    >,
    MetricsCallbackMaker,
>;

/// Manages a pool of connections to peers to avoid constantly reconnecting,
/// which can be expensive.
struct ChannelPool {
    context: Arc<Context>,
    // Size is limited by known authorities in the committee.
    channels: RwLock<BTreeMap<AuthorityIndex, Channel>>,
}

impl ChannelPool {
    fn new(context: Arc<Context>) -> Self {
        Self {
            context,
            channels: RwLock::new(BTreeMap::new()),
        }
    }

    async fn get_channel(
        &self,
        _network_keypair: NetworkKeyPair,
        peer: AuthorityIndex,
        timeout: Duration,
    ) -> ConsensusResult<Channel> {
        {
            let channels = self.channels.read();
            if let Some(channel) = channels.get(&peer) {
                return Ok(channel.clone());
            }
        }

        let authority = self.context.committee.authority(peer);
        let address = to_host_port_str(&authority.address).map_err(|e| {
            ConsensusError::NetworkConfig(format!("Cannot convert address to host:port: {e:?}"))
        })?;
        let config = &self.context.parameters.tonic;
        let buffer_size = config.connection_buffer_size;

        // Check if TLS should be disabled (for local development)
        let disable_tls = true; // Hardcode for testing

        let deadline = tokio::time::Instant::now() + timeout;
        let channel = loop {
            trace!("Connecting to endpoint at {}", address);
            let addr = if disable_tls {
                format!("http://{address}")
            } else {
                format!("https://{address}")
            };

            // Use plain TCP without TLS
            let endpoint = tonic::transport::Channel::from_shared(addr.clone())
                .expect("valid HTTP/HTTPS URI from committee address config")
                .connect_timeout(timeout)
                .initial_connection_window_size(Some(buffer_size as u32))
                .initial_stream_window_size(Some(buffer_size as u32 / 2))
                .keep_alive_while_idle(true)
                .keep_alive_timeout(config.keepalive_interval)
                .http2_keep_alive_interval(config.keepalive_interval)
                .user_agent("mysticeti")
                .expect("static user_agent string is always valid");
            let result = endpoint.connect().await;

            match result {
                Ok(channel) => break channel,
                Err(e) => {
                    debug!("Failed to connect to endpoint at {addr}: {e:?}");
                    if tokio::time::Instant::now() >= deadline {
                        return Err(ConsensusError::NetworkClientConnection(format!(
                            "Timed out connecting to endpoint at {addr}: {e:?}"
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        trace!("Connected to {address}");

        let channel = tower::ServiceBuilder::new()
            .layer(CallbackLayer::new(MetricsCallbackMaker::new(
                self.context.metrics.network_metrics.outbound.clone(),
                self.context.parameters.tonic.excessive_message_size,
            )))
            .layer(
                TraceLayer::new_for_grpc()
                    .make_span_with(DefaultMakeSpan::new().level(tracing::Level::TRACE))
                    .on_failure(DefaultOnFailure::new().level(tracing::Level::DEBUG)),
            )
            .service(channel);

        let mut channels = self.channels.write();
        // There should not be many concurrent attempts at connecting to the same peer.
        let channel = channels.entry(peer).or_insert(channel);
        Ok(channel.clone())
    }
}

/// Proxies Tonic requests to NetworkService with actual handler implementation.
struct TonicServiceProxy<S: NetworkService> {
    context: Arc<Context>,
    service: Arc<S>,
}

impl<S: NetworkService> TonicServiceProxy<S> {
    fn new(context: Arc<Context>, service: Arc<S>) -> Self {
        Self { context, service }
    }
}

#[async_trait]
impl<S: NetworkService> ConsensusService for TonicServiceProxy<S> {
    async fn send_block(
        &self,
        request: Request<SendBlockRequest>,
    ) -> Result<Response<SendBlockResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let block = request.into_inner().block;
        let block = ExtendedSerializedBlock {
            block,
            excluded_ancestors: vec![],
        };
        self.service
            .handle_send_block(peer_index, block)
            .await
            .map_err(|e| tonic::Status::invalid_argument(format!("{e:?}")))?;
        Ok(Response::new(SendBlockResponse {}))
    }

    type SubscribeBlocksStream =
        Pin<Box<dyn Stream<Item = Result<SubscribeBlocksResponse, tonic::Status>> + Send>>;

    async fn subscribe_blocks(
        &self,
        request: Request<Streaming<SubscribeBlocksRequest>>,
    ) -> Result<Response<Self::SubscribeBlocksStream>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let mut request_stream = request.into_inner();
        let first_request = match request_stream.next().await {
            Some(Ok(r)) => r,
            Some(Err(e)) => {
                debug!(
                    "subscribe_blocks() request from {} failed: {e:?}",
                    peer_index
                );
                return Err(tonic::Status::invalid_argument("Request error"));
            }
            None => {
                return Err(tonic::Status::invalid_argument("Missing request"));
            }
        };
        let stream = self
            .service
            .handle_subscribe_blocks(peer_index, first_request.last_received_round)
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?
            .map(|block| {
                Ok(SubscribeBlocksResponse {
                    block: block.block,
                    excluded_ancestors: block.excluded_ancestors,
                })
            });
        let rate_limited_stream =
            tokio_stream::StreamExt::throttle(stream, self.context.parameters.min_round_delay / 2)
                .boxed();
        Ok(Response::new(rate_limited_stream))
    }

    type FetchBlocksStream = Iter<std::vec::IntoIter<Result<FetchBlocksResponse, tonic::Status>>>;

    async fn fetch_blocks(
        &self,
        request: Request<FetchBlocksRequest>,
    ) -> Result<Response<Self::FetchBlocksStream>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let inner = request.into_inner();
        let block_refs = inner
            .block_refs
            .into_iter()
            .filter_map(|serialized| match bcs::from_bytes(&serialized) {
                Ok(r) => Some(r),
                Err(e) => {
                    debug!("Failed to deserialize block ref {:?}: {e:?}", serialized);
                    None
                }
            })
            .collect();
        let highest_accepted_rounds = inner.highest_accepted_rounds;
        let breadth_first = inner.breadth_first;
        let blocks = self
            .service
            .handle_fetch_blocks(
                peer_index,
                block_refs,
                highest_accepted_rounds,
                breadth_first,
            )
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        let responses: std::vec::IntoIter<Result<FetchBlocksResponse, tonic::Status>> =
            chunk_blocks(blocks, MAX_FETCH_RESPONSE_BYTES)
                .into_iter()
                .map(|blocks| Ok(FetchBlocksResponse { blocks }))
                .collect::<Vec<_>>()
                .into_iter();
        let stream = iter(responses);
        Ok(Response::new(stream))
    }

    async fn fetch_commits(
        &self,
        request: Request<FetchCommitsRequest>,
    ) -> Result<Response<FetchCommitsResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let request = request.into_inner();
        let (commits, certifier_blocks) = self
            .service
            .handle_fetch_commits(peer_index, (request.start..=request.end).into())
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        let commits = commits
            .into_iter()
            .map(|c| c.serialized().clone())
            .collect();
        let certifier_blocks = certifier_blocks
            .into_iter()
            .map(|b| b.serialized().clone())
            .collect();
        Ok(Response::new(FetchCommitsResponse {
            commits,
            certifier_blocks,
        }))
    }

    async fn fetch_commits_by_global_range(
        &self,
        request: Request<FetchCommitsByGlobalRangeRequest>,
    ) -> Result<Response<FetchCommitsByGlobalRangeResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            });
        let request = request.into_inner();
        let commits = self
            .service
            .handle_fetch_commits_by_global_range(
                peer_index,
                request.start_global_index,
                request.end_global_index,
            )
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;

        // Collect all blocks from commits
        let blocks: Vec<Bytes> = commits.iter().flat_map(|c| c.block_refs.clone()).collect();

        Ok(Response::new(FetchCommitsByGlobalRangeResponse {
            commits,
            blocks,
        }))
    }

    type FetchLatestBlocksStream =
        Iter<std::vec::IntoIter<Result<FetchLatestBlocksResponse, tonic::Status>>>;

    async fn fetch_latest_blocks(
        &self,
        request: Request<FetchLatestBlocksRequest>,
    ) -> Result<Response<Self::FetchLatestBlocksStream>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let inner = request.into_inner();

        // Convert the authority indexes and validate them
        let mut authorities = vec![];
        for authority in inner.authorities.into_iter() {
            let Some(authority) = self
                .context
                .committee
                .to_authority_index(authority as usize)
            else {
                return Err(tonic::Status::internal(format!(
                    "Invalid authority index provided {authority}"
                )));
            };
            authorities.push(authority);
        }

        let blocks = self
            .service
            .handle_fetch_latest_blocks(peer_index, authorities)
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        let responses: std::vec::IntoIter<Result<FetchLatestBlocksResponse, tonic::Status>> =
            chunk_blocks(blocks, MAX_FETCH_RESPONSE_BYTES)
                .into_iter()
                .map(|blocks| Ok(FetchLatestBlocksResponse { blocks }))
                .collect::<Vec<_>>()
                .into_iter();
        let stream = iter(responses);
        Ok(Response::new(stream))
    }

    async fn get_latest_rounds(
        &self,
        request: Request<GetLatestRoundsRequest>,
    ) -> Result<Response<GetLatestRoundsResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            }); // Use dummy index if PeerInfo missing
        let (highest_received, highest_accepted) = self
            .service
            .handle_get_latest_rounds(peer_index)
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        Ok(Response::new(GetLatestRoundsResponse {
            highest_received,
            highest_accepted,
        }))
    }

    async fn send_epoch_change_proposal(
        &self,
        request: Request<SendEpochChangeProposalRequest>,
    ) -> Result<Response<SendEpochChangeProposalResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            });
        let proposal_bytes = request.into_inner().proposal;
        let proposal: EpochChangeProposal = bcs::from_bytes(&proposal_bytes).map_err(|e| {
            tonic::Status::invalid_argument(format!("deserialize proposal failed: {e:?}"))
        })?;
        self.service
            .handle_send_epoch_change_proposal(peer_index, proposal)
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        Ok(Response::new(SendEpochChangeProposalResponse {}))
    }

    async fn send_epoch_change_vote(
        &self,
        request: Request<SendEpochChangeVoteRequest>,
    ) -> Result<Response<SendEpochChangeVoteResponse>, tonic::Status> {
        let peer_index = request
            .extensions()
            .get::<PeerInfo>()
            .map(|p| p.authority_index)
            .unwrap_or_else(|| {
                trace!("⚠️ [PEERINFO] PeerInfo missing, using dummy index 0");
                AuthorityIndex::new_for_test(0)
            });
        let vote_bytes = request.into_inner().vote;
        let vote: EpochChangeVote = bcs::from_bytes(&vote_bytes).map_err(|e| {
            tonic::Status::invalid_argument(format!("deserialize vote failed: {e:?}"))
        })?;
        self.service
            .handle_send_epoch_change_vote(peer_index, vote)
            .await
            .map_err(|e| tonic::Status::internal(format!("{e:?}")))?;
        Ok(Response::new(SendEpochChangeVoteResponse {}))
    }
}

/// Manages the lifecycle of Tonic network client and service. Typical usage during initialization:
/// 1. Create a new `TonicManager`.
/// 2. Take `TonicClient` from `TonicManager::client()`.
/// 3. Create consensus components.
/// 4. Create `TonicService` for consensus service handler.
/// 5. Install `TonicService` to `TonicManager` with `TonicManager::install_service()`.
pub(crate) struct TonicManager {
    context: Arc<Context>,
    network_keypair: NetworkKeyPair,
    client: Arc<TonicClient>,
    server: Option<ServerHandle>,
}

impl TonicManager {
    pub(crate) fn new(context: Arc<Context>, network_keypair: NetworkKeyPair) -> Self {
        Self {
            context: context.clone(),
            network_keypair: network_keypair.clone(),
            client: Arc::new(TonicClient::new(context, network_keypair)),
            server: None,
        }
    }
}

impl<S: NetworkService> NetworkManager<S> for TonicManager {
    type Client = TonicClient;

    fn new(context: Arc<Context>, network_keypair: NetworkKeyPair) -> Self {
        TonicManager::new(context, network_keypair)
    }

    fn client(&self) -> Arc<Self::Client> {
        self.client.clone()
    }

    async fn install_service(&mut self, service: Arc<S>) {
        self.context
            .metrics
            .network_metrics
            .network_type
            .with_label_values(&["tonic"])
            .set(1);

        info!("Starting tonic service");

        let authority = self.context.committee.authority(self.context.own_index);
        // By default, bind to the unspecified address to allow the actual address to be assigned.
        // But bind to localhost if it is requested.
        let own_address = if authority.address.is_localhost_ip() {
            authority.address.clone()
        } else {
            authority.address.with_zero_ip()
        };
        let own_address = to_socket_addr(&own_address)
            .expect("own committee address must be a valid socket addr");
        let service = TonicServiceProxy::new(self.context.clone(), service);
        let config = &self.context.parameters.tonic;

        // Hardcode disable TLS for testing - use proper PeerInfo lookup from committee
        let committee = self.context.committee.clone(); // Clone for lifetime
        let layers = tower::ServiceBuilder::new()
            .map_request(move |mut request: http::Request<_>| {
                // Get remote address first
                let remote_addr = request.extensions().get::<std::net::SocketAddr>();

                // Lookup authority index from committee by matching address/port
                let authority_index = if let Some(addr) = remote_addr {
                    let peer_ip = addr.ip();
                    let peer_port = addr.port();

                    // Search through committee authorities to find matching address
                    let mut found_index = None;
                    for (idx, authority) in committee.authorities() {
                        if let Ok(auth_addr) = authority.address.to_socket_addr() {
                            if auth_addr.ip() == peer_ip && auth_addr.port() == peer_port {
                                found_index = Some(idx);
                                break;
                            }
                        }
                    }

                    found_index.unwrap_or(AuthorityIndex::new_for_test(0)) // Fallback to 0 if not found
                } else {
                    AuthorityIndex::new_for_test(0) // Fallback if no remote address
                };

                let peer_info = PeerInfo { authority_index };
                info!("🔧 [PEERINFO] Injecting PeerInfo with authority_index={:?} for remote_addr={:?}",
                      authority_index, remote_addr);
                request.extensions_mut().insert(peer_info);
                request
            })
            .layer(CallbackLayer::new(MetricsCallbackMaker::new(
                self.context.metrics.network_metrics.inbound.clone(),
                self.context.parameters.tonic.excessive_message_size,
            )))
            .layer(
                TraceLayer::new_for_grpc()
                    .make_span_with(DefaultMakeSpan::new().level(tracing::Level::TRACE))
                    .on_failure(DefaultOnFailure::new().level(tracing::Level::DEBUG)),
            )
            .layer_fn(|service| {
                mysten_network::grpc_timeout::GrpcTimeout::new(
                    service,
                    // This should only bound the unary and initial response time,
                    // not the duration of streaming responses.
                    DEFAULT_GRPC_SERVER_TIMEOUT,
                )
            });

        let consensus_service_server = ConsensusServiceServer::new(service)
            .max_encoding_message_size(config.message_size_limit)
            .max_decoding_message_size(config.message_size_limit)
            .send_compressed(CompressionEncoding::Zstd)
            .accept_compressed(CompressionEncoding::Zstd);

        let consensus_service = tonic::service::Routes::new(consensus_service_server)
            .into_axum_router()
            .route_layer(layers);

        // Check if TLS should be disabled (for local development)
        let _disable_tls = true; // Hardcode for testing

        let tls_server_config = if true {
            // Hardcode disable TLS for testing
            None
        } else {
            Some(meta_tls::create_rustls_server_config_with_client_verifier(
                self.network_keypair.clone().private_key().into_inner(),
                certificate_server_name(&self.context),
                AllowPublicKeys::new(
                    self.context
                        .committee
                        .authorities()
                        .map(|(_i, a)| a.network_key.clone().into_inner())
                        .collect(),
                ),
            ))
        };

        // Calculate some metrics around send/recv buffer sizes for the current machine/OS
        #[cfg(not(msim))]
        {
            let tcp_connection_metrics =
                &self.context.metrics.network_metrics.tcp_connection_metrics;

            // Try creating an ephemeral port to test the highest allowed send and recv buffer sizes.
            // Buffer sizes are not set explicitly on the socket used for real traffic,
            // to allow the OS to set appropriate values.
            {
                let ephemeral_addr = SocketAddr::new(own_address.ip(), 0);
                let ephemeral_socket = create_socket(&ephemeral_addr);
                tcp_connection_metrics
                    .socket_send_buffer_size
                    .set(ephemeral_socket.send_buffer_size().unwrap_or(0) as i64);
                tcp_connection_metrics
                    .socket_recv_buffer_size
                    .set(ephemeral_socket.recv_buffer_size().unwrap_or(0) as i64);

                if let Err(e) = ephemeral_socket.set_send_buffer_size(32 << 20) {
                    info!("Failed to set send buffer size: {e:?}");
                }
                if let Err(e) = ephemeral_socket.set_recv_buffer_size(32 << 20) {
                    info!("Failed to set recv buffer size: {e:?}");
                }
                if ephemeral_socket.bind(ephemeral_addr).is_ok() {
                    tcp_connection_metrics
                        .socket_send_buffer_max_size
                        .set(ephemeral_socket.send_buffer_size().unwrap_or(0) as i64);
                    tcp_connection_metrics
                        .socket_recv_buffer_max_size
                        .set(ephemeral_socket.recv_buffer_size().unwrap_or(0) as i64);
                };
            }
        }

        let http_config = meta_http::Config::default()
            .initial_connection_window_size(64 << 20)
            .initial_stream_window_size(32 << 20)
            .http2_keepalive_interval(Some(config.keepalive_interval))
            .http2_keepalive_timeout(Some(config.keepalive_interval))
            .accept_http1(false);

        // Create server
        //
        // During simtest crash/restart tests there may be an older instance of consensus running
        // that is bound to the TCP port of `own_address` that hasn't finished relinquishing
        // control of the port yet. So instead of crashing when the address is inuse, we will retry
        // for a short/reasonable period of time before giving up.
        let deadline = Instant::now() + Duration::from_secs(20);
        let server = loop {
            let builder = meta_http::Builder::new().config(http_config.clone());
            let builder = if let Some(tls_config) = &tls_server_config {
                builder.tls_config(tls_config.clone())
            } else {
                builder
            };

            match builder.serve(own_address, consensus_service.clone()) {
                Ok(server) => break server,
                Err(err) => {
                    warn!("Error starting consensus server: {err:?}");
                    if Instant::now() > deadline {
                        panic!("Failed to start consensus server within required deadline");
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        };

        info!("Server started at: {own_address}");
        self.server = Some(server);
    }

    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await;
        }

        self.context
            .metrics
            .network_metrics
            .network_type
            .with_label_values(&["tonic"])
            .set(0);
    }
}

// Ensure that if there is an active network running that it is shutdown when the TonicManager is
// dropped.
impl Drop for TonicManager {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_ref() {
            server.trigger_shutdown();
        }
    }
}

/// Attempts to convert a multiaddr of the form `/[ip4,ip6,dns]/{}/[udp,tcp]/{port}` into
/// a host:port string.
fn to_host_port_str(addr: &Multiaddr) -> Result<String, String> {
    let mut iter = addr.iter();

    match (iter.next(), iter.next()) {
        (Some(Protocol::Ip4(ipaddr)), Some(Protocol::Udp(port) | Protocol::Tcp(port))) => {
            Ok(format!("{}:{}", ipaddr, port))
        }
        (Some(Protocol::Ip6(ipaddr)), Some(Protocol::Udp(port) | Protocol::Tcp(port))) => {
            Ok(format!("{}:{}", ipaddr, port))
        }
        (Some(Protocol::Dns(hostname)), Some(Protocol::Udp(port) | Protocol::Tcp(port))) => {
            Ok(format!("{}:{}", hostname, port))
        }

        _ => Err(format!("unsupported multiaddr: {addr}")),
    }
}

/// Attempts to convert a multiaddr of the form `/[ip4,ip6]/{}/[udp,tcp]/{port}` into
/// a SocketAddr value.
pub fn to_socket_addr(addr: &Multiaddr) -> Result<SocketAddr, String> {
    let mut iter = addr.iter();

    match (iter.next(), iter.next()) {
        (Some(Protocol::Ip4(ipaddr)), Some(Protocol::Udp(port)))
        | (Some(Protocol::Ip4(ipaddr)), Some(Protocol::Tcp(port))) => {
            Ok(SocketAddr::V4(SocketAddrV4::new(ipaddr, port)))
        }

        (Some(Protocol::Ip6(ipaddr)), Some(Protocol::Udp(port)))
        | (Some(Protocol::Ip6(ipaddr)), Some(Protocol::Tcp(port))) => {
            Ok(SocketAddr::V6(SocketAddrV6::new(ipaddr, port, 0, 0)))
        }

        _ => Err(format!("unsupported multiaddr: {addr}")),
    }
}

#[cfg(not(msim))]
fn create_socket(address: &SocketAddr) -> tokio::net::TcpSocket {
    let socket = if address.is_ipv4() {
        tokio::net::TcpSocket::new_v4()
    } else if address.is_ipv6() {
        tokio::net::TcpSocket::new_v6()
    } else {
        panic!("Invalid own address: {address:?}");
    }
    .unwrap_or_else(|e| panic!("Cannot create TCP socket: {e:?}"));
    if let Err(e) = socket.set_nodelay(true) {
        info!("Failed to set TCP_NODELAY: {e:?}");
    }
    if let Err(e) = socket.set_reuseaddr(true) {
        info!("Failed to set SO_REUSEADDR: {e:?}");
    }
    socket
}

/// Looks up authority index by authority public key.
///
/// TODO: Add connection monitoring, and keep track of connected peers.
/// TODO: Maybe merge with connection_monitor.rs
#[allow(dead_code)]
struct ConnectionsInfo {
    authority_key_to_index: BTreeMap<NetworkPublicKey, AuthorityIndex>,
}

#[allow(dead_code)]
impl ConnectionsInfo {
    fn new(context: Arc<Context>) -> Self {
        let authority_key_to_index = context
            .committee
            .authorities()
            .map(|(index, authority)| (authority.network_key.clone(), index))
            .collect();
        Self {
            authority_key_to_index,
        }
    }

    #[allow(dead_code)]
    fn authority_index(&self, key: &NetworkPublicKey) -> Option<AuthorityIndex> {
        self.authority_key_to_index.get(key).copied()
    }
}

/// Information about the client peer, set per connection.
#[derive(Clone, Debug)]
struct PeerInfo {
    authority_index: AuthorityIndex,
}

// Adapt MetricsCallbackMaker and MetricsResponseCallback to http.

impl SizedRequest for http::request::Parts {
    fn size(&self) -> usize {
        self.headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
    }

    fn route(&self) -> String {
        let path = self.uri.path();
        path.rsplit_once('/')
            .map(|(_, route)| route)
            .unwrap_or("unknown")
            .to_string()
    }
}

impl SizedResponse for http::response::Parts {
    fn size(&self) -> usize {
        self.headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
    }

    fn error_type(&self) -> Option<String> {
        if self.status.is_success() {
            None
        } else {
            Some(self.status.to_string())
        }
    }
}

impl MakeCallbackHandler for MetricsCallbackMaker {
    type Handler = MetricsResponseCallback;

    fn make_handler(&self, request: &http::request::Parts) -> Self::Handler {
        self.handle_request(request)
    }
}

impl ResponseHandler for MetricsResponseCallback {
    fn on_response(&mut self, response: &http::response::Parts) {
        MetricsResponseCallback::on_response(self, response)
    }

    fn on_error<E>(&mut self, err: &E) {
        MetricsResponseCallback::on_error(self, err)
    }
}

/// Network message types.
#[derive(Clone, prost::Message)]
pub(crate) struct SendBlockRequest {
    // Serialized SignedBlock.
    #[prost(bytes = "bytes", tag = "1")]
    block: Bytes,
}

#[derive(Clone, prost::Message)]
pub(crate) struct SendBlockResponse {}

#[derive(Clone, prost::Message)]
pub(crate) struct SubscribeBlocksRequest {
    #[prost(uint32, tag = "1")]
    last_received_round: Round,
}

#[derive(Clone, prost::Message)]
pub(crate) struct SubscribeBlocksResponse {
    #[prost(bytes = "bytes", tag = "1")]
    block: Bytes,
    // Serialized BlockRefs that are excluded from the blocks ancestors.
    #[prost(bytes = "vec", repeated, tag = "2")]
    excluded_ancestors: Vec<Vec<u8>>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchBlocksRequest {
    #[prost(bytes = "vec", repeated, tag = "1")]
    block_refs: Vec<Vec<u8>>,
    // The highest accepted round per authority. The vector represents the round for each authority
    // and its length should be the same as the committee size.
    // When this field is non-empty, additional ancestors of the requested blocks can be fetched.
    #[prost(uint32, repeated, tag = "2")]
    highest_accepted_rounds: Vec<Round>,
    // When true, this indicates that missing ancestors should be added breadth-first, by searching through
    // missing ancestors of the requested blocks.
    // When false, this indicates that missing ancestors should be added depth-first, by adding missing
    // ancestors from the requested block authorities.
    // This field is only meaningful when highest_accepted_rounds is non-empty.
    #[prost(bool, tag = "3")]
    breadth_first: bool,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchBlocksResponse {
    // The response of the requested blocks as Serialized SignedBlock.
    #[prost(bytes = "bytes", repeated, tag = "1")]
    blocks: Vec<Bytes>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchCommitsRequest {
    #[prost(uint32, tag = "1")]
    start: CommitIndex,
    #[prost(uint32, tag = "2")]
    end: CommitIndex,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchCommitsResponse {
    // Serialized consecutive Commit.
    #[prost(bytes = "bytes", repeated, tag = "1")]
    commits: Vec<Bytes>,
    // Serialized SignedBlock that certify the last commit from above.
    #[prost(bytes = "bytes", repeated, tag = "2")]
    certifier_blocks: Vec<Bytes>,
}

/// Request to fetch commits by global execution index range.
/// This is epoch-agnostic - server will search across all epochs.
#[derive(Clone, prost::Message)]
pub(crate) struct FetchCommitsByGlobalRangeRequest {
    /// Start global execution index (inclusive)
    #[prost(uint64, tag = "1")]
    start_global_index: u64,
    /// End global execution index (inclusive)
    #[prost(uint64, tag = "2")]
    end_global_index: u64,
}

/// Response containing commits with their global execution indices and epoch info.
#[derive(Clone, prost::Message)]
pub(crate) struct FetchCommitsByGlobalRangeResponse {
    /// Serialized commits with global index metadata
    #[prost(message, repeated, tag = "1")]
    pub commits: Vec<GlobalCommitInfo>,
    /// Serialized SignedBlocks for the commits
    #[prost(bytes = "bytes", repeated, tag = "2")]
    pub blocks: Vec<Bytes>,
}

/// Commit info with global execution index
#[derive(Clone, prost::Message)]
pub struct GlobalCommitInfo {
    /// The epoch this commit belongs to
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
    /// Global execution index (unique across all epochs)
    #[prost(uint64, tag = "2")]
    pub global_exec_index: u64,
    /// Epoch-local commit index
    #[prost(uint32, tag = "3")]
    pub local_commit_index: u32,
    /// Epoch boundary block (first block of this epoch minus 1)
    #[prost(uint64, tag = "4")]
    pub epoch_boundary_block: u64,
    /// Serialized commit data
    #[prost(bytes = "bytes", tag = "5")]
    pub commit_data: Bytes,
    /// Block references in this commit
    #[prost(bytes = "bytes", repeated, tag = "6")]
    pub block_refs: Vec<Bytes>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchLatestBlocksRequest {
    #[prost(uint32, repeated, tag = "1")]
    authorities: Vec<u32>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct FetchLatestBlocksResponse {
    // The response of the requested blocks as Serialized SignedBlock.
    #[prost(bytes = "bytes", repeated, tag = "1")]
    blocks: Vec<Bytes>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct GetLatestRoundsRequest {}

#[derive(Clone, prost::Message)]
pub(crate) struct GetLatestRoundsResponse {
    // Highest received round per authority.
    #[prost(uint32, repeated, tag = "1")]
    highest_received: Vec<u32>,
    // Highest accepted round per authority.
    #[prost(uint32, repeated, tag = "2")]
    highest_accepted: Vec<u32>,
}

#[derive(Clone, prost::Message)]
pub(crate) struct SendEpochChangeProposalRequest {
    // Serialized EpochChangeProposal.
    #[prost(bytes = "bytes", tag = "1")]
    proposal: Bytes,
}

#[derive(Clone, prost::Message)]
pub(crate) struct SendEpochChangeProposalResponse {}

#[derive(Clone, prost::Message)]
pub(crate) struct SendEpochChangeVoteRequest {
    // Serialized EpochChangeVote.
    #[prost(bytes = "bytes", tag = "1")]
    vote: Bytes,
}

#[derive(Clone, prost::Message)]
pub(crate) struct SendEpochChangeVoteResponse {}

fn chunk_blocks(blocks: Vec<Bytes>, chunk_limit: usize) -> Vec<Vec<Bytes>> {
    let mut chunks = vec![];
    let mut chunk = vec![];
    let mut chunk_size = 0;
    for block in blocks {
        let block_size = block.len();
        if !chunk.is_empty() && chunk_size + block_size > chunk_limit {
            chunks.push(chunk);
            chunk = vec![];
            chunk_size = 0;
        }
        chunk.push(block);
        chunk_size += block_size;
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}
