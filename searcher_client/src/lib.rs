use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use jito_protos::{
    auth::{auth_service_client::AuthServiceClient, Role},
    bundle::{
        bundle_result::Result as BundleResultType, rejected::Reason, Accepted, Bundle,
        BundleResult, InternalError, SimulationFailure, StateAuctionBidRejected,
        WinningBatchBidRejected,
    },
    convert::proto_packet_from_versioned_tx,
    searcher::{
        searcher_service_client::SearcherServiceClient, SendBundleRequest, SendBundleResponse,
    },
};
use log::{info, warn};
use thiserror::Error;
use tokio::time::timeout;
use tonic::{
    codegen::{Body, Bytes, InterceptedService, StdError},
    transport,
    transport::{Channel, Endpoint},
    Response, Status, Streaming,
};
use solana_sdk::{
    transaction::VersionedTransaction,
    signer::keypair::Keypair
};

use crate::token_authenticator::ClientInterceptor;

pub mod token_authenticator;

#[derive(Debug, Error)]
pub enum BlockEngineConnectionError {
    #[error("transport error {0}")]
    TransportError(#[from] transport::Error),
    #[error("client error {0}")]
    ClientError(#[from] Status),
}

#[derive(Debug, Error)]
pub enum BundleRejectionError {
    #[error("bundle lost state auction, auction: {0}, tip {1} lamports")]
    StateAuctionBidRejected(String, u64),
    #[error("bundle won state auction but failed global auction, auction {0}, tip {1} lamports")]
    WinningBatchBidRejected(String, u64),
    #[error("bundle simulation failure on tx {0}, message: {1:?}")]
    SimulationFailure(String, Option<String>),
    #[error("internal error {0}")]
    InternalError(String),
}

pub type BlockEngineConnectionResult<T> = Result<T, BlockEngineConnectionError>;

pub async fn get_searcher_client_auth(
    block_engine_url: &str,
    auth_keypair: &Arc<Keypair>,
) -> BlockEngineConnectionResult<
    SearcherServiceClient<InterceptedService<Channel, ClientInterceptor>>,
> {
    let auth_channel = create_grpc_channel(block_engine_url).await?;
    let client_interceptor = ClientInterceptor::new(
        AuthServiceClient::new(auth_channel),
        auth_keypair,
        Role::Searcher,
    )
    .await?;

    let searcher_channel = create_grpc_channel(block_engine_url).await?;
    let searcher_client =
        SearcherServiceClient::with_interceptor(searcher_channel, client_interceptor);
    Ok(searcher_client)
}

pub async fn get_searcher_client_no_auth(
    block_engine_url: &str,
) -> BlockEngineConnectionResult<SearcherServiceClient<Channel>> {
    let searcher_channel = create_grpc_channel(block_engine_url).await?;
    let searcher_client = SearcherServiceClient::new(searcher_channel);
    Ok(searcher_client)
}

pub async fn create_grpc_channel(url: &str) -> BlockEngineConnectionResult<Channel> {
    let mut endpoint = Endpoint::from_shared(url.to_string()).expect("invalid url");
    if url.starts_with("https") {
        endpoint = endpoint.tls_config(tonic::transport::ClientTlsConfig::new())?;
    }
    Ok(endpoint.connect().await?)
}

pub async fn send_bundle_no_wait<T>(
    transactions: &[VersionedTransaction],
    searcher_client: &mut SearcherServiceClient<T>,
) -> Result<Response<SendBundleResponse>, Status>
where
    T: tonic::client::GrpcService<tonic::body::BoxBody> + Send + 'static + Clone,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    <T as tonic::client::GrpcService<tonic::body::BoxBody>>::Future: std::marker::Send,
{
    // convert them to packets + send over
    let packets: Vec<_> = transactions
        .iter()
        .map(proto_packet_from_versioned_tx)
        .collect();

    searcher_client
        .send_bundle(SendBundleRequest {
            bundle: Some(Bundle {
                header: None,
                packets,
            }),
        })
        .await
}
