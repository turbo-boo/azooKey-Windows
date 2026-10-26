use azookey_server::presentation::{presentation, CompositionEdit, Presentation};
use azookey_server::TonicNamedPipeServer;
use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

use shared::proto::azookey_service_server::{AzookeyService, AzookeyServiceServer};
use shared::proto::{
    AppendTextRequest, AppendTextResponse, ClearTextRequest, ClearTextResponse, ComposingText,
    ClearPostCompositionPredictionsRequest, ClearPostCompositionPredictionsResponse,
    CommitPredictionRequest, CommitPredictionResponse, GetPostCompositionPredictionsRequest,
    GetPostCompositionPredictionsResponse, MagicConvertRequest, MagicConvertResponse,
    MoveCursorRequest, MoveCursorResponse, PredictionSuggestion, RemoveTextRequest,
    RemoveTextResponse, RequestConversionRequest, RequestConversionResponse,
    ShrinkTextRequest, ShrinkTextResponse, Suggestion,
};
use shared::{parse_magic_candidates, validate_magic_endpoint_parts, AppConfig};

use std::collections::hash_map::RandomState;
use std::ffi::{c_char, c_int, CStr, CString};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

const USE_ZENZAI: bool = true;
static LIVE_CONVERSION_ENABLED: AtomicBool = AtomicBool::new(true);

/// The converter's current composition (see ComposingText.composition_id).
/// Starts from a random value so a restarted server never repeats the
/// previous instance's identifiers.
static COMPOSITION_ID: LazyLock<AtomicU64> =
    LazyLock::new(|| AtomicU64::new(RandomState::new().hash_one(std::process::id())));

struct RawComposingText {
    text: String,
    cursor: i32,
}

#[derive(Debug, Clone)]
#[repr(C)]
struct FFICandidate {
    text: *mut c_char,
    subtext: *mut c_char,
    hiragana: *mut c_char,
    corresponding_count: c_int,
    candidate_index: c_int,
}

unsafe extern "C" {
    fn Initialize(path: *const c_char, use_zenzai: bool);
    fn SetContext(context: *const c_char);
    fn AppendText(input: *const c_char, cursorPtr: *mut c_int) -> *mut c_char;
    fn RemoveText(cursorPtr: *mut c_int) -> *mut c_char;
    fn MoveCursor(offset: c_int, cursorPtr: *mut c_int) -> *mut c_char;
    fn ShrinkText(offset: c_int) -> *mut c_char;
    fn ClearText();
    fn GetInputCount() -> c_int;
    fn GetInputText() -> *mut c_char;
    fn GetComposedText(lengthPtr: *mut c_int, requestRichCandidates: bool) -> *mut *mut FFICandidate;
    fn CommitCandidate(candidateIndex: c_int);
    fn FlushLearning();
    fn ResetLearning();
    fn GetPostCompositionPredictions(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn CommitPrediction(predictionIndex: c_int);
    fn ClearPostCompositionPredictions();
    fn LoadConfig();
    fn FreeString(ptr: *mut c_char);
    fn FreeCandidates(list: *mut *mut FFICandidate, length: c_int);
}

/// Copies a string returned by the converter and releases the original.
unsafe fn take_string(ptr: *mut c_char) -> String {
    let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    FreeString(ptr);
    text
}

/// Copies a candidate list returned by the converter and releases the original.
unsafe fn take_candidates(list: *mut *mut FFICandidate, length: c_int) -> Vec<FFICandidateData> {
    let candidates = (0..length.max(0) as usize)
        .map(|index| {
            let candidate = &**list.add(index);
            FFICandidateData {
                text: CStr::from_ptr(candidate.text).to_string_lossy().into_owned(),
                subtext: CStr::from_ptr(candidate.subtext)
                    .to_string_lossy()
                    .into_owned(),
                corresponding_count: candidate.corresponding_count,
                candidate_index: candidate.candidate_index,
            }
        })
        .collect();
    FreeCandidates(list, length);
    candidates
}

struct FFICandidateData {
    text: String,
    subtext: String,
    corresponding_count: i32,
    candidate_index: i32,
}

fn initialize(path: &str) {
    unsafe {
        let path = CString::new(path).expect("CString::new failed");
        Initialize(path.as_ptr(), USE_ZENZAI);
    }
}

/// The text the converter is given crosses the FFI as a C string, which
/// cannot carry NUL; protobuf strings can.
fn c_string(text: &str) -> Result<CString, Status> {
    CString::new(text).map_err(|_| Status::invalid_argument("text must not contain NUL"))
}

fn add_text(input: &CString) -> RawComposingText {
    unsafe {
        let mut cursor: c_int = 0;

        let result = AppendText(input.as_ptr(), &mut cursor);

        RawComposingText {
            text: take_string(result),
            cursor,
        }
    }
}

fn move_cursor(offset: i32) -> RawComposingText {
    unsafe {
        let mut cursor: c_int = 0;

        let result = MoveCursor(offset, &mut cursor);

        RawComposingText {
            text: take_string(result),
            cursor,
        }
    }
}

fn remove_text() -> RawComposingText {
    unsafe {
        let mut cursor: c_int = 0;

        let result = RemoveText(&mut cursor);

        RawComposingText {
            text: take_string(result),
            cursor,
        }
    }
}

fn clear_text() {
    unsafe {
        ClearText();
    }
    // the converter starts a new composition
    COMPOSITION_ID.fetch_add(1, Ordering::Relaxed);
}

fn get_composed_text(rich_candidates: bool) -> Vec<Suggestion> {
    let candidates = unsafe {
        let mut length: c_int = 0;
        let result = GetComposedText(&mut length, rich_candidates);
        take_candidates(result, length)
    };

    let mut suggestions: Vec<Suggestion> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        // check if suggestions have the same text
        if suggestions.iter().any(|s| s.text == candidate.text) {
            continue;
        }
        suggestions.push(Suggestion {
            text: candidate.text,
            subtext: candidate.subtext,
            corresponding_count: candidate.corresponding_count,
            candidate_id: candidate.candidate_index,
        });
    }
    suggestions
}

fn refresh_runtime_config() {
    LIVE_CONVERSION_ENABLED.store(
        AppConfig::read().live_conversion.enabled,
        Ordering::Relaxed,
    );
}

fn composing_text_response(raw: RawComposingText, edit: CompositionEdit) -> ComposingText {
    let live_conversion = LIVE_CONVERSION_ENABLED.load(Ordering::Relaxed);
    let shown = presentation(edit, live_conversion, &raw.text);
    let is_converted = matches!(shown, Presentation::Converted { .. });
    let input_count = unsafe { GetInputCount() };
    let suggestions = if let Presentation::Converted { rich_candidates } = shown {
        get_composed_text(rich_candidates)
    } else if raw.text.is_empty() {
        vec![]
    } else {
        vec![Suggestion {
            text: raw.text.clone(),
            subtext: String::new(),
            corresponding_count: input_count,
            candidate_id: -1,
        }]
    };

    ComposingText {
        hiragana: raw.text,
        suggestions,
        is_converted,
        input_count,
        composition_id: COMPOSITION_ID.load(Ordering::Relaxed),
        input_text: unsafe { take_string(GetInputText()) },
    }
}

fn get_post_composition_predictions() -> Vec<PredictionSuggestion> {
    let candidates = unsafe {
        let mut length: c_int = 0;
        let result = GetPostCompositionPredictions(&mut length);
        take_candidates(result, length)
    };

    candidates
        .into_iter()
        .map(|candidate| PredictionSuggestion {
            text: candidate.text,
            prediction_id: candidate.candidate_index,
        })
        .collect()
}

fn commit_prediction(prediction_id: i32) -> Vec<PredictionSuggestion> {
    unsafe {
        CommitPrediction(prediction_id);
    }
    get_post_composition_predictions()
}

fn clear_post_composition_predictions() {
    unsafe {
        ClearPostCompositionPredictions();
    }
}

fn shrink_text(offset: i32) -> RawComposingText {
    unsafe {
        let result = ShrinkText(offset);

        RawComposingText {
            text: take_string(result),
            cursor: 0,
        }
    }
}

async fn request_magic_conversion(text: &str) -> Result<Vec<String>, String> {
    let config = AppConfig::read();
    let magic = config.magic_conversion;
    if !magic.enabled {
        return Err("いい感じ変換が無効です".to_string());
    }
    if text.trim().is_empty() {
        return Err("変換対象が空です".to_string());
    }

    let endpoint = reqwest::Url::parse(&magic.endpoint)
        .map_err(|error| format!("APIエンドポイントが不正です: {error}"))?;
    validate_magic_endpoint_parts(endpoint.scheme(), endpoint.host_str())?;
    if magic.model.trim().is_empty() {
        return Err("APIモデル名が空です".to_string());
    }

    // The endpoint checks above apply to this URL only. A redirect (307/308
    // re-sends the body) could move the input text to plain HTTP or another
    // host, so redirects are not followed; a 3xx is reported as an API error.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(magic.timeout_seconds.clamp(1, 60)))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;

    let body = serde_json::json!({
        "model": magic.model,
        "messages": [
            {
                "role": "system",
                "content": "日本語入力支援です。入力文をユーザーの指示に従って自然に書き換えてください。返答は {\"predictions\":[\"候補1\",\"候補2\",\"候補3\"]} のJSONだけにしてください。候補は1〜5個にしてください。"
            },
            {
                "role": "user",
                "content": format!("指示: {}\n入力: {}", magic.instruction, text)
            }
        ]
    });

    let mut request = client.post(endpoint).json(&body);
    if !magic.api_key.trim().is_empty() {
        request = request.bearer_auth(magic.api_key.trim());
    }

    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let response_text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let compact = response_text.chars().take(500).collect::<String>();
        return Err(format!("APIエラー {}: {}", status.as_u16(), compact));
    }

    let value: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|error| format!("APIレスポンスのJSON解析に失敗しました: {error}"))?;
    parse_magic_candidates(&value)
}

#[derive(Debug, Default)]
pub struct MyAzookeyService;

#[tonic::async_trait]
impl AzookeyService for MyAzookeyService {
    async fn append_text(
        &self,
        request: Request<AppendTextRequest>,
    ) -> Result<Response<AppendTextResponse>, Status> {
        let input = c_string(&request.into_inner().text_to_append)?;
        let composing_text = add_text(&input);

        Ok(Response::new(AppendTextResponse {
            composing_text: Some(composing_text_response(composing_text, CompositionEdit::Typed)),
        }))
    }

    async fn remove_text(
        &self,
        _: Request<RemoveTextRequest>,
    ) -> Result<Response<RemoveTextResponse>, Status> {
        let composing_text = remove_text();

        Ok(Response::new(RemoveTextResponse {
            composing_text: Some(composing_text_response(composing_text, CompositionEdit::Deleted)),
        }))
    }

    async fn move_cursor(
        &self,
        request: Request<MoveCursorRequest>,
    ) -> Result<Response<MoveCursorResponse>, Status> {
        let offset = request.into_inner().offset;
        let composing_text = move_cursor(offset);

        Ok(Response::new(MoveCursorResponse {
            composing_text: Some(composing_text_response(composing_text, CompositionEdit::Typed)),
        }))
    }

    async fn clear_text(
        &self,
        _: Request<ClearTextRequest>,
    ) -> Result<Response<ClearTextResponse>, Status> {
        clear_text();
        Ok(Response::new(ClearTextResponse {}))
    }

    async fn shrink_text(
        &self,
        request: Request<ShrinkTextRequest>,
    ) -> Result<Response<ShrinkTextResponse>, Status> {
        let offset = request.into_inner().offset;
        // The converter traps on a negative count.
        if offset < 0 {
            return Err(Status::invalid_argument("offset must not be negative"));
        }
        let composing_text = shrink_text(offset);

        Ok(Response::new(ShrinkTextResponse {
            composing_text: Some(composing_text_response(
                composing_text,
                CompositionEdit::ConversionRequested,
            )),
        }))
    }

    async fn request_conversion(
        &self,
        _: Request<RequestConversionRequest>,
    ) -> Result<Response<RequestConversionResponse>, Status> {
        let composing_text = move_cursor(0);
        Ok(Response::new(RequestConversionResponse {
            composing_text: Some(composing_text_response(
                composing_text,
                CompositionEdit::ConversionRequested,
            )),
        }))
    }

    async fn set_context(
        &self,
        request: Request<shared::proto::SetContextRequest>,
    ) -> Result<Response<shared::proto::SetContextResponse>, Status> {
        let context = request.into_inner().context;
        let trimmed_context = context
            .split('\r')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or_default();

        let context = c_string(trimmed_context)?;

        unsafe { SetContext(context.as_ptr()) };
        Ok(Response::new(shared::proto::SetContextResponse {}))
    }

    async fn update_config(
        &self,
        _: Request<shared::proto::UpdateConfigRequest>,
    ) -> Result<Response<shared::proto::UpdateConfigResponse>, Status> {
        refresh_runtime_config();
        unsafe { LoadConfig() };
        Ok(Response::new(shared::proto::UpdateConfigResponse {}))
    }

    async fn commit_candidate(
        &self,
        request: Request<shared::proto::CommitCandidateRequest>,
    ) -> Result<Response<shared::proto::CommitCandidateResponse>, Status> {
        let candidate_id = request.into_inner().candidate_id;
        unsafe { CommitCandidate(candidate_id) };
        Ok(Response::new(shared::proto::CommitCandidateResponse {}))
    }

    async fn flush_learning(
        &self,
        _: Request<shared::proto::FlushLearningRequest>,
    ) -> Result<Response<shared::proto::FlushLearningResponse>, Status> {
        unsafe { FlushLearning() };
        Ok(Response::new(shared::proto::FlushLearningResponse {}))
    }

    async fn reset_learning(
        &self,
        _: Request<shared::proto::ResetLearningRequest>,
    ) -> Result<Response<shared::proto::ResetLearningResponse>, Status> {
        unsafe { ResetLearning() };
        Ok(Response::new(shared::proto::ResetLearningResponse {}))
    }

    async fn magic_convert(
        &self,
        request: Request<MagicConvertRequest>,
    ) -> Result<Response<MagicConvertResponse>, Status> {
        let text = request.into_inner().text;
        let candidates = request_magic_conversion(&text)
            .await
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(MagicConvertResponse { candidates }))
    }

    async fn get_post_composition_predictions(
        &self,
        _: Request<GetPostCompositionPredictionsRequest>,
    ) -> Result<Response<GetPostCompositionPredictionsResponse>, Status> {
        Ok(Response::new(GetPostCompositionPredictionsResponse {
            suggestions: get_post_composition_predictions(),
        }))
    }

    async fn commit_prediction(
        &self,
        request: Request<CommitPredictionRequest>,
    ) -> Result<Response<CommitPredictionResponse>, Status> {
        Ok(Response::new(CommitPredictionResponse {
            suggestions: commit_prediction(request.into_inner().prediction_id),
        }))
    }

    async fn clear_post_composition_predictions(
        &self,
        _: Request<ClearPostCompositionPredictionsRequest>,
    ) -> Result<Response<ClearPostCompositionPredictionsResponse>, Status> {
        clear_post_composition_predictions();
        Ok(Response::new(ClearPostCompositionPredictionsResponse {}))
    }
}

// The converter keeps a single global composition and is not thread-safe, and
// its entry points are @MainActor. A current-thread runtime runs every handler
// on the main thread, one at a time; handlers do not await between FFI calls.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("AzookeyServer started");
    // get executable directory
    let current_exe = std::env::current_exe()?;
    let parent_dir = current_exe.parent().unwrap();
    initialize(parent_dir.to_str().unwrap());
    refresh_runtime_config();

    let service = MyAzookeyService::default();

    println!("AzookeyServer listening");

    Server::builder()
        .add_service(AzookeyServiceServer::new(service))
        .add_service(
            ReflectionBuilder::configure()
                .register_encoded_file_descriptor_set(shared::proto::FILE_DESCRIPTOR_SET)
                .build_v1()
                .unwrap(),
        )
        .serve_with_incoming(TonicNamedPipeServer::new(shared::pipe::SERVER_PIPE)?)
        .await?;

    // Serving only ends when the pipe listener failed (see
    // TonicNamedPipeServer::new); exit non-zero so the launcher restarts us.
    Err("the pipe listener stopped".into())
}