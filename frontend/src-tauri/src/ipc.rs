use anyhow::{anyhow, Result};
use hyper_util::rt::TokioIo;
use shared::proto::azookey_service_client::AzookeyServiceClient;
use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::windows::named_pipe::NamedPipeClient, time};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;
use windows::Win32::Foundation::ERROR_PIPE_BUSY;

/// How long to wait for a free pipe instance and the connection handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the converter server may take to answer one request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub enum ConnectError {
    /// No converter server is running for this user and session.
    NotRunning,
    Failed(anyhow::Error),
}

/// A connection to the converter server of this user and session.
pub struct IPCService {
    azookey_client: AzookeyServiceClient<Channel>,
}

impl IPCService {
    pub async fn connect() -> Result<Self, ConnectError> {
        let path = shared::pipe::path(shared::pipe::SERVER_PIPE)
            .map_err(|error| ConnectError::Failed(error.into()))?;
        let client = match open_pipe(&path).await {
            Ok(client) => client,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConnectError::NotRunning)
            }
            Err(error) => return Err(ConnectError::Failed(error.into())),
        };

        // The pipe is opened above so that a missing server can be told
        // apart; the connector hands that one connection to the channel.
        let client = Arc::new(Mutex::new(Some(client)));
        let connector = service_fn(move |_| {
            let client = client.lock().unwrap_or_else(|e| e.into_inner()).take();
            async move {
                client
                    .map(TokioIo::new)
                    .ok_or_else(|| io::Error::other("the converter connection was closed"))
            }
        });
        let endpoint = Endpoint::try_from("http://[::]:50051")
            .map_err(|error| ConnectError::Failed(error.into()))?;
        let channel = time::timeout(CONNECT_TIMEOUT, endpoint.connect_with_connector(connector))
            .await
            .map_err(|_| ConnectError::Failed(connect_timeout_error()))?
            .map_err(|error| ConnectError::Failed(error.into()))?;

        Ok(Self {
            azookey_client: AzookeyServiceClient::new(channel),
        })
    }

    pub async fn update_config(&mut self) -> Result<()> {
        let request = shared::proto::UpdateConfigRequest {};
        with_deadline(self.azookey_client.update_config(request)).await
    }

    pub async fn reset_learning(&mut self) -> Result<()> {
        let request = shared::proto::ResetLearningRequest {};
        with_deadline(self.azookey_client.reset_learning(request)).await
    }
}

/// Opens the server's pipe, waiting up to [`CONNECT_TIMEOUT`] while every
/// instance is busy.
async fn open_pipe(path: &str) -> io::Result<NamedPipeClient> {
    let deadline = time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        match shared::pipe::open_client(path) {
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                if time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        connect_timeout_error().to_string(),
                    ));
                }
            }
            result => return result,
        }
        time::sleep(Duration::from_millis(50)).await;
    }
}

fn connect_timeout_error() -> anyhow::Error {
    anyhow!(
        "変換サーバーへ{}秒以内に接続できませんでした",
        CONNECT_TIMEOUT.as_secs()
    )
}

async fn with_deadline<T>(
    call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<()> {
    time::timeout(REQUEST_TIMEOUT, call).await.map_err(|_| {
        anyhow!(
            "変換サーバーが{}秒以内に応答しませんでした",
            REQUEST_TIMEOUT.as_secs()
        )
    })??;
    Ok(())
}
