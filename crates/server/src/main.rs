use azookey_server::TonicNamedPipeServer;
use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

use shared::proto::azookey_service_server::{AzookeyService, AzookeyServiceServer};
use shared::proto::{
    AppendTextRequest, AppendTextResponse, ClearTextRequest, ClearTextResponse, ComposingText,
    MagicConvertRequest, MagicConvertResponse, MoveCursorRequest, MoveCursorResponse,
    RemoveTextRequest, RemoveTextResponse, ShrinkTextRequest, ShrinkTextResponse, Suggestion,
};
use shared::AppConfig;

use std::ffi::{c_char, c_int, CStr, CString};
use std::time::Duration;

const USE_ZENZAI: bool = true;

struct RawComposingText {
    text: String,
    cursor: i8,
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
    fn GetComposedText(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn CommitCandidate(candidateIndex: c_int);
    fn FlushLearning();
    fn ResetLearning();
    fn LoadConfig();
}

fn initialize(path: &str) {
    unsafe {
        let path = CString::new(path).expect("CString::new failed");
        Initialize(path.as_ptr(), USE_ZENZAI);
    }
}

fn add_text(input: &str) -> RawComposingText {
    unsafe {
        let input = CString::new(input).expect("CString::new failed");
        let mut cursor: c_int = 0;

        let result = AppendText(input.as_ptr(), &mut cursor);

        let text = CStr::from_ptr(&*result as *const c_char).to_str().unwrap();

        RawComposingText {
            text: text.to_string(),
            cursor: cursor as i8,
        }
    }
}

fn move_cursor(offset: i8) -> RawComposingText {
    unsafe {
        let offset = c_int::from(offset);
        println!("Offset: {}", offset);
        let mut cursor: c_int = 0;

        let result = MoveCursor(offset, &mut cursor);

        let text = CStr::from_ptr(&*result as *const c_char).to_str().unwrap();

        RawComposingText {
            text: text.to_string(),
            cursor: cursor as i8,
        }
    }
}

fn remove_text() -> RawComposingText {
    unsafe {
        let mut cursor: c_int = 0;

        let result = RemoveText(&mut cursor);

        let text = CStr::from_ptr(&*result as *const c_char).to_str().unwrap();

        RawComposingText {
            text: text.to_string(),
            cursor: cursor as i8,
        }
    }
}

fn clear_text() {
    unsafe {
        ClearText();
    }
}

fn get_composed_text() -> Vec<Suggestion> {
    unsafe {
        let mut length: c_int = 0;
        let result = GetComposedText(&mut length);
        let mut suggestions = Vec::with_capacity(length as usize);

        for index in 0..length as usize {
            let candidate = (**result.add(index)).clone();
            let text = CStr::from_ptr(candidate.text)
                .to_string_lossy()
                .into_owned();
            let subtext = CStr::from_ptr(candidate.subtext)
                .to_string_lossy()
                .into_owned();
            let corresponding_count = candidate.corresponding_count;
            let candidate_id = candidate.candidate_index;

            let suggestion = Suggestion {
                text,
                subtext,
                corresponding_count,
                candidate_id,
            };

            // check if suggestions have the same text
            if suggestions
                .iter()
                .any(|s: &Suggestion| s.text == suggestion.text)
            {
                continue;
            }
            suggestions.push(suggestion);
        }

        suggestions
    }
}

fn shrink_text(offset: i8) -> RawComposingText {
    unsafe {
        let offset = c_int::from(offset);
        let result = ShrinkText(offset);

        let text = CStr::from_ptr(&*result as *const c_char).to_str().unwrap();

        RawComposingText {
            text: text.to_string(),
            cursor: 0,
        }
    }
}

fn parse_magic_candidates(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let choices = value
        .get("choices")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "APIレスポンスにchoicesがありません".to_string())?;

    let mut candidates = Vec::new();
    for choice in choices {
        let Some(content) = choice
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(predictions) = parsed.get("predictions").and_then(|value| value.as_array()) {
                for prediction in predictions {
                    if let Some(text) = prediction.as_str() {
                        let text = text.trim();
                        if !text.is_empty() && !candidates.iter().any(|item| item == text) {
                            candidates.push(text.to_string());
                        }
                    }
                }
                continue;
            }
        }

        let text = content.trim();
        if !text.is_empty() && !candidates.iter().any(|item| item == text) {
            candidates.push(text.to_string());
        }
    }

    candidates.truncate(5);
    if candidates.is_empty() {
        Err("APIレスポンスから候補を取得できませんでした".to_string())
    } else {
        Ok(candidates)
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
    if endpoint.scheme() != "https" && endpoint.scheme() != "http" {
        return Err("APIエンドポイントはHTTPまたはHTTPSで指定してください".to_string());
    }
    if magic.model.trim().is_empty() {
        return Err("APIモデル名が空です".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(magic.timeout_seconds.clamp(1, 60)))
        .build()
        .map_err(|error| error.to_string())?;

    let body = serde_json::json!({
        "model": magic.model,
        "messages": [
            {
                "role": "system",
                "content": "日本語入力支援です。入力文をユーザーの指示に従って自然に書き換え、3〜5個の候補だけを返してください。"
            },
            {
                "role": "user",
                "content": format!("指示: {}\n入力: {}", magic.instruction, text)
            }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "azookey_magic_conversion",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "predictions": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "maxItems": 5
                        }
                    },
                    "required": ["predictions"],
                    "additionalProperties": false
                }
            }
        }
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
        let input = request.into_inner().text_to_append;
        let composing_text = add_text(&input);

        Ok(Response::new(AppendTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions: get_composed_text().to_vec(),
            }),
        }))
    }

    async fn remove_text(
        &self,
        _: Request<RemoveTextRequest>,
    ) -> Result<Response<RemoveTextResponse>, Status> {
        let composing_text = remove_text();

        Ok(Response::new(RemoveTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions: get_composed_text().to_vec(),
            }),
        }))
    }

    async fn move_cursor(
        &self,
        request: Request<MoveCursorRequest>,
    ) -> Result<Response<MoveCursorResponse>, Status> {
        let offset = request.into_inner().offset as i8;
        let composing_text = move_cursor(offset);

        Ok(Response::new(MoveCursorResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions: get_composed_text().to_vec(),
            }),
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
        let offset = request.into_inner().offset as i8;
        let composing_text = shrink_text(offset);

        Ok(Response::new(ShrinkTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions: get_composed_text().to_vec(),
            }),
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

        let context = CString::new(trimmed_context).expect("CString::new failed");

        unsafe { SetContext(context.as_ptr()) };
        Ok(Response::new(shared::proto::SetContextResponse {}))
    }

    async fn update_config(
        &self,
        _: Request<shared::proto::UpdateConfigRequest>,
    ) -> Result<Response<shared::proto::UpdateConfigResponse>, Status> {
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("AzookeyServer started");
    // get executable directory
    let current_exe = std::env::current_exe()?;
    let parent_dir = current_exe.parent().unwrap();
    initialize(parent_dir.to_str().unwrap());

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
        .serve_with_incoming(TonicNamedPipeServer::new("azookey_server"))
        .await?;

    Ok(())
}