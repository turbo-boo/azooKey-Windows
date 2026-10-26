use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::proto::{
    azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{net::windows::named_pipe::NamedPipeClient, sync::mpsc, time};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;
use windows::Win32::Foundation::ERROR_PIPE_BUSY;

/// Opening the local pipe normally takes microseconds. This also bounds the
/// ERROR_PIPE_BUSY retries in `open_pipe`: tonic applies it to the whole
/// connector future, including when a channel reconnects.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// How long the TSF thread waits for one converter call. TSF runs key
/// handling on the application's UI thread, and Windows reports a window as
/// not responding once it has not pumped messages for 5 seconds, so a call
/// must give up before that. Zenzai runs a single LLM pass per request
/// (inferenceLimit 1), which takes well under a second for typical input
/// even on the CPU backend; 4 seconds leaves room for long input and a busy
/// GPU without letting a hung server freeze the application.
const CONVERTER_RPC_TIMEOUT: Duration = Duration::from_secs(4);

/// Candidate window updates run on their own thread; the deadline only keeps
/// a hung window server from holding back the commands queued behind it.
const WINDOW_RPC_TIMEOUT: Duration = Duration::from_secs(2);

/// The AI conversion runs on a worker thread. The server gives the HTTP API
/// at most 60 seconds (magic_conversion.timeout_seconds is clamped to 1..=60).
const MAGIC_RPC_TIMEOUT: Duration = Duration::from_secs(70);

// connect to kkc server
#[derive(Debug, Clone)]
pub struct IPCService {
    // kkc server client
    azookey_client: AzookeyServiceClient<Channel>,
    // candidate window updates, delivered in order by a dedicated thread
    window_commands: mpsc::UnboundedSender<WindowCommand>,
    // current-thread runtime: block_on drives the pipe I/O on the calling
    // thread instead of waiting for a wakeup from a worker thread
    runtime: Arc<tokio::runtime::Runtime>,
    // set once a call on this handle missed its deadline: the server is not
    // answering, so later calls fail at once instead of stalling the UI
    // thread again. Each key gets a fresh handle from IMEState.
    unresponsive: bool,
}

#[derive(Debug)]
enum WindowCommand {
    Show,
    Hide,
    SetPosition(shared::proto::WindowPosition),
    SetCandidates(Vec<String>),
    SetSelection(i32),
    SetInputMode(String),
}

#[derive(Debug, Clone, Default)]
pub struct PredictionCandidates {
    pub texts: Vec<String>,
    pub prediction_ids: Vec<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct Candidates {
    pub texts: Vec<String>,
    pub sub_texts: Vec<String>,
    pub hiragana: String,
    pub corresponding_count: Vec<i32>,
    pub candidate_ids: Vec<i32>,
    pub is_converted: bool,
    /// Number of input units (typed keys, or kana after the converter folded
    /// them) in the whole composition. ShrinkText counts in these units.
    pub input_count: i32,
}

impl Candidates {
    fn from_composing_text(
        composing_text: Option<shared::proto::ComposingText>,
    ) -> anyhow::Result<Self> {
        let composing_text =
            composing_text.ok_or_else(|| anyhow::anyhow!("composing_text is None"))?;
        let suggestions = &composing_text.suggestions;
        Ok(Self {
            texts: suggestions.iter().map(|s| s.text.clone()).collect(),
            sub_texts: suggestions.iter().map(|s| s.subtext.clone()).collect(),
            corresponding_count: suggestions.iter().map(|s| s.corresponding_count).collect(),
            candidate_ids: suggestions.iter().map(|s| s.candidate_id).collect(),
            is_converted: composing_text.is_converted,
            input_count: composing_text.input_count,
            hiragana: composing_text.hiragana,
        })
    }
}

/// Opens the pipe, retrying while every server instance is busy. Callers bound
/// the retries with CONNECT_TIMEOUT (`Endpoint::connect_timeout`).
async fn open_pipe(base: &'static str) -> std::io::Result<TokioIo<NamedPipeClient>> {
    let path = shared::pipe::path(base)?;
    let client = loop {
        match shared::pipe::open_client(&path) {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
            Err(e) => return Err(e),
        }

        time::sleep(Duration::from_millis(50)).await;
    };

    Ok(TokioIo::new(client))
}

fn endpoint(uri: &'static str) -> Result<Endpoint> {
    Ok(Endpoint::try_from(uri)?.connect_timeout(CONNECT_TIMEOUT))
}

/// Waits for `call` at most `deadline`.
async fn with_deadline<T>(
    deadline: Duration,
    call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<T> {
    match time::timeout(deadline, call).await {
        Ok(response) => Ok(response?.into_inner()),
        Err(_) => anyhow::bail!("server did not answer within {deadline:?}"),
    }
}

fn current_thread_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

// Candidate window updates do not return data the composition depends on, so
// the TSF thread only enqueues them. A single consumer keeps them in order.
fn spawn_window_dispatcher() -> Result<mpsc::UnboundedSender<WindowCommand>> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<WindowCommand>();
    let runtime = current_thread_runtime()?;

    std::thread::Builder::new()
        .name("azookey-window-ipc".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                let channel = match endpoint("http://[::]:50052") {
                    Ok(endpoint) => endpoint.connect_with_connector_lazy(service_fn(|_| {
                        open_pipe(shared::pipe::UI_PIPE)
                    })),
                    Err(error) => {
                        tracing::error!("Invalid window endpoint: {error:?}");
                        return;
                    }
                };
                let mut client = WindowServiceClient::new(channel);

                while let Some(command) = receiver.recv().await {
                    let call = async {
                        match command {
                            WindowCommand::Show => client
                                .show_window(shared::proto::EmptyResponse {})
                                .await
                                .map(drop),
                            WindowCommand::Hide => client
                                .hide_window(shared::proto::EmptyResponse {})
                                .await
                                .map(drop),
                            WindowCommand::SetPosition(position) => client
                                .set_window_position(shared::proto::SetPositionRequest {
                                    position: Some(position),
                                })
                                .await
                                .map(drop),
                            WindowCommand::SetCandidates(candidates) => client
                                .set_candidate(shared::proto::SetCandidateRequest { candidates })
                                .await
                                .map(drop),
                            WindowCommand::SetSelection(index) => client
                                .set_selection(shared::proto::SetSelectionRequest { index })
                                .await
                                .map(drop),
                            WindowCommand::SetInputMode(mode) => client
                                .set_input_mode(shared::proto::SetInputModeRequest { mode })
                                .await
                                .map(drop),
                        }
                    };
                    let result = time::timeout(WINDOW_RPC_TIMEOUT, call)
                        .await
                        .unwrap_or_else(|_| {
                            Err(tonic::Status::deadline_exceeded(
                                "window server did not answer",
                            ))
                        });
                    if let Err(error) = result {
                        tracing::warn!("Candidate window update failed: {error:?}");
                    }
                }
            });
        })?;

    Ok(sender)
}

async fn connect_converter() -> Result<AzookeyServiceClient<Channel>> {
    let channel = endpoint("http://[::]:50051")?
        .connect_with_connector(service_fn(|_| open_pipe(shared::pipe::SERVER_PIPE)))
        .await?;
    Ok(AzookeyServiceClient::new(channel))
}

/// Requests an AI conversion of `text` on a worker thread with its own
/// connection, so the TSF thread does not wait for the HTTP API (up to a
/// minute). `done` runs on the worker thread with the result.
pub fn magic_convert_in_background(
    text: String,
    done: impl FnOnce(Result<Vec<String>>) + Send + 'static,
) -> Result<()> {
    std::thread::Builder::new()
        .name("azookey-magic-convert".to_string())
        .spawn(move || {
            let result = current_thread_runtime()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    runtime.block_on(async {
                        let mut client = connect_converter().await?;
                        let request = shared::proto::MagicConvertRequest { text };
                        let response =
                            with_deadline(MAGIC_RPC_TIMEOUT, client.magic_convert(request)).await?;
                        Ok(response.candidates)
                    })
                });
            done(result);
        })?;
    Ok(())
}

impl IPCService {
    pub fn new() -> Result<Self> {
        let runtime = current_thread_runtime()?;

        let azookey_client = runtime.block_on(connect_converter())?;
        tracing::debug!("Connected to server: {:?}", azookey_client);

        Ok(Self {
            azookey_client,
            window_commands: spawn_window_dispatcher()?,
            runtime: Arc::new(runtime),
            unresponsive: false,
        })
    }

    /// Fails at once when an earlier call on this handle missed its deadline.
    fn ensure_responsive(&self) -> Result<()> {
        if self.unresponsive {
            anyhow::bail!("server did not answer the previous call");
        }
        Ok(())
    }

    fn finish<T>(
        &mut self,
        result: Result<Result<tonic::Response<T>, tonic::Status>, time::error::Elapsed>,
    ) -> Result<T> {
        match result {
            Ok(response) => Ok(response?.into_inner()),
            Err(_) => {
                self.unresponsive = true;
                anyhow::bail!("server did not answer within {CONVERTER_RPC_TIMEOUT:?}")
            }
        }
    }

    fn send_window_command(&self, command: WindowCommand) -> Result<()> {
        self.window_commands
            .send(command)
            .map_err(|_| anyhow::anyhow!("candidate window dispatcher stopped"))
    }
}

/// Runs `$method` of the converter client on this thread, waiting at most
/// CONVERTER_RPC_TIMEOUT, and returns the response message.
macro_rules! converter_call {
    ($self:ident, $method:ident($request:expr)) => {{
        $self.ensure_responsive()?;
        let runtime = $self.runtime.clone();
        let result = runtime.block_on(time::timeout(
            CONVERTER_RPC_TIMEOUT,
            $self.azookey_client.$method($request),
        ));
        $self.finish(result)
    }};
}

// implement methods to interact with kkc server
impl IPCService {
    #[tracing::instrument]
    pub fn append_text(&mut self, text: String) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::AppendTextRequest {
            text_to_append: text,
        });

        let response = converter_call!(self, append_text(request))?;
        let composing_text = response.composing_text;
        Candidates::from_composing_text(composing_text)
    }

    #[tracing::instrument]
    pub fn remove_text(&mut self) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RemoveTextRequest {});
        let response = converter_call!(self, remove_text(request))?;
        let composing_text = response.composing_text;
        Candidates::from_composing_text(composing_text)
    }

    #[tracing::instrument]
    pub fn request_conversion(&mut self) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RequestConversionRequest {});
        let response = converter_call!(self, request_conversion(request))?;
        let composing_text = response.composing_text;
        Candidates::from_composing_text(composing_text)
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearTextRequest {});
        converter_call!(self, clear_text(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn commit_candidate(&mut self, candidate_id: i32) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::CommitCandidateRequest { candidate_id });
        converter_call!(self, commit_candidate(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn flush_learning(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::FlushLearningRequest {});
        converter_call!(self, flush_learning(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn get_post_composition_predictions(&mut self) -> anyhow::Result<PredictionCandidates> {
        let request = tonic::Request::new(shared::proto::GetPostCompositionPredictionsRequest {});
        let response = converter_call!(self, get_post_composition_predictions(request))?;
        Ok(PredictionCandidates {
            texts: response.suggestions.iter().map(|item| item.text.clone()).collect(),
            prediction_ids: response.suggestions.iter().map(|item| item.prediction_id).collect(),
        })
    }

    #[tracing::instrument]
    pub fn commit_prediction(&mut self, prediction_id: i32) -> anyhow::Result<PredictionCandidates> {
        let request = tonic::Request::new(shared::proto::CommitPredictionRequest { prediction_id });
        let response = converter_call!(self, commit_prediction(request))?;
        Ok(PredictionCandidates {
            texts: response.suggestions.iter().map(|item| item.text.clone()).collect(),
            prediction_ids: response.suggestions.iter().map(|item| item.prediction_id).collect(),
        })
    }

    #[tracing::instrument]
    pub fn clear_post_composition_predictions(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearPostCompositionPredictionsRequest {});
        converter_call!(self, clear_post_composition_predictions(request))?;
        Ok(())
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::ShrinkTextRequest { offset });
        let response = converter_call!(self, shrink_text(request))?;
        let composing_text = response.composing_text;
        Candidates::from_composing_text(composing_text)
    }

    pub fn set_context(&mut self, context: String) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetContextRequest { context });
        converter_call!(self, set_context(request))?;

        Ok(())
    }
}

// implement methods to interact with candidate window server
impl IPCService {
    pub fn show_window(&mut self) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::Show)
    }

    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::Hide)
    }

    pub fn set_window_position(
        &mut self,
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    ) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::SetPosition(shared::proto::WindowPosition {
            top,
            left,
            bottom,
            right,
        }))
    }

    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::SetCandidates(candidates))
    }

    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::SetSelection(index))
    }

    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        self.send_window_command(WindowCommand::SetInputMode(mode.to_string()))
    }
}
