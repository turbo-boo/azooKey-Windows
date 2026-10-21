use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::proto::{
    azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    sync::mpsc,
    time,
};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;
use windows::Win32::Foundation::ERROR_PIPE_BUSY;

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
}

async fn open_pipe(name: &'static str) -> std::io::Result<TokioIo<NamedPipeClient>> {
    let client = loop {
        match ClientOptions::new().open(name) {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
            Err(e) => return Err(e),
        }

        time::sleep(Duration::from_millis(50)).await;
    };

    Ok(TokioIo::new(client))
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
                let channel = match Endpoint::try_from("http://[::]:50052") {
                    Ok(endpoint) => endpoint.connect_with_connector_lazy(service_fn(|_| {
                        open_pipe(r"\\.\pipe\azookey_ui")
                    })),
                    Err(error) => {
                        tracing::error!("Invalid window endpoint: {error:?}");
                        return;
                    }
                };
                let mut client = WindowServiceClient::new(channel);

                while let Some(command) = receiver.recv().await {
                    let result = match command {
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
                    };
                    if let Err(error) = result {
                        tracing::warn!("Candidate window update failed: {error:?}");
                    }
                }
            });
        })?;

    Ok(sender)
}

impl IPCService {
    pub fn new() -> Result<Self> {
        let runtime = current_thread_runtime()?;

        let server_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50051")?
                .connect_with_connector(service_fn(|_| open_pipe(r"\\.\pipe\azookey_server"))),
        )?;

        let azookey_client = AzookeyServiceClient::new(server_channel);
        tracing::debug!("Connected to server: {:?}", azookey_client);

        Ok(Self {
            azookey_client,
            window_commands: spawn_window_dispatcher()?,
            runtime: Arc::new(runtime),
        })
    }

    fn send_window_command(&self, command: WindowCommand) -> Result<()> {
        self.window_commands
            .send(command)
            .map_err(|_| anyhow::anyhow!("candidate window dispatcher stopped"))
    }
}

// implement methods to interact with kkc server
impl IPCService {
    #[tracing::instrument]
    pub fn append_text(&mut self, text: String) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::AppendTextRequest {
            text_to_append: text,
        });

        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.append_text(request))?;
        let composing_text = response.into_inner().composing_text;

        let candidates = if let Some(composing_text) = composing_text {
            Candidates {
                texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
                sub_texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.subtext.clone())
                    .collect(),
                hiragana: composing_text.hiragana,
                corresponding_count: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.corresponding_count)
                    .collect(),
                candidate_ids: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.candidate_id)
                    .collect(),
                is_converted: composing_text.is_converted,
            }
        } else {
            anyhow::bail!("composing_text is None");
        };

        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn remove_text(&mut self) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RemoveTextRequest {});
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.remove_text(request))?;
        let composing_text = response.into_inner().composing_text;

        let candidates = if let Some(composing_text) = composing_text {
            Candidates {
                texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
                sub_texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.subtext.clone())
                    .collect(),
                hiragana: composing_text.hiragana,
                corresponding_count: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.corresponding_count)
                    .collect(),
                candidate_ids: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.candidate_id)
                    .collect(),
                is_converted: composing_text.is_converted,
            }
        } else {
            anyhow::bail!("composing_text is None");
        };

        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn request_conversion(&mut self) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RequestConversionRequest {});
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.request_conversion(request))?;
        let composing_text = response
            .into_inner()
            .composing_text
            .ok_or_else(|| anyhow::anyhow!("composing_text is None"))?;

        Ok(Candidates {
            texts: composing_text.suggestions.iter().map(|s| s.text.clone()).collect(),
            sub_texts: composing_text.suggestions.iter().map(|s| s.subtext.clone()).collect(),
            hiragana: composing_text.hiragana,
            corresponding_count: composing_text
                .suggestions
                .iter()
                .map(|s| s.corresponding_count)
                .collect(),
            candidate_ids: composing_text
                .suggestions
                .iter()
                .map(|s| s.candidate_id)
                .collect(),
            is_converted: composing_text.is_converted,
        })
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearTextRequest {});
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.clear_text(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn commit_candidate(&mut self, candidate_id: i32) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::CommitCandidateRequest { candidate_id });
        self.runtime
            .clone()
            .block_on(self.azookey_client.commit_candidate(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn flush_learning(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::FlushLearningRequest {});
        self.runtime
            .clone()
            .block_on(self.azookey_client.flush_learning(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn get_post_composition_predictions(&mut self) -> anyhow::Result<PredictionCandidates> {
        let request = tonic::Request::new(shared::proto::GetPostCompositionPredictionsRequest {});
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.get_post_composition_predictions(request))?
            .into_inner();
        Ok(PredictionCandidates {
            texts: response.suggestions.iter().map(|item| item.text.clone()).collect(),
            prediction_ids: response.suggestions.iter().map(|item| item.prediction_id).collect(),
        })
    }

    #[tracing::instrument]
    pub fn commit_prediction(&mut self, prediction_id: i32) -> anyhow::Result<PredictionCandidates> {
        let request = tonic::Request::new(shared::proto::CommitPredictionRequest { prediction_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.commit_prediction(request))?
            .into_inner();
        Ok(PredictionCandidates {
            texts: response.suggestions.iter().map(|item| item.text.clone()).collect(),
            prediction_ids: response.suggestions.iter().map(|item| item.prediction_id).collect(),
        })
    }

    #[tracing::instrument]
    pub fn clear_post_composition_predictions(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearPostCompositionPredictionsRequest {});
        self.runtime
            .clone()
            .block_on(self.azookey_client.clear_post_composition_predictions(request))?;
        Ok(())
    }

    #[tracing::instrument]
    pub fn magic_convert(&mut self, text: String) -> anyhow::Result<Vec<String>> {
        let request = tonic::Request::new(shared::proto::MagicConvertRequest { text });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.magic_convert(request))?;
        Ok(response.into_inner().candidates)
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::ShrinkTextRequest { offset });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.shrink_text(request))?;
        let composing_text = response.into_inner().composing_text;

        let candidates = if let Some(composing_text) = composing_text {
            Candidates {
                texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
                sub_texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.subtext.clone())
                    .collect(),
                hiragana: composing_text.hiragana,
                corresponding_count: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.corresponding_count)
                    .collect(),
                candidate_ids: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.candidate_id)
                    .collect(),
                is_converted: composing_text.is_converted,
            }
        } else {
            anyhow::bail!("composing_text is None");
        };

        Ok(candidates)
    }

    pub fn set_context(&mut self, context: String) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetContextRequest { context });
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.set_context(request))?;

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
