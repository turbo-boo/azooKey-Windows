mod ipc;

use serde::{Deserialize, Serialize};
use shared::{
    decode_dictionary_bytes, dictionary_extension, export_user_dictionary_json,
    export_user_dictionary_tsv, export_user_dictionary_tsv_utf16le, parse_user_dictionary,
    validate_tuner_directory, AppConfig, UserDictionaryEntry,
};
use parking_lot::Mutex;
use std::path::PathBuf;

#[derive(Debug)]
pub struct AppState {
    settings: Mutex<AppConfig>,
    // Connected on demand: the settings window must open even when the
    // converter server is not running (e.g. to fix the backend setting).
    ipc: Mutex<Option<ipc::IPCService>>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            settings: Mutex::new(AppConfig::new()),
            ipc: Mutex::new(None),
        }
    }

    fn ipc(&self) -> anyhow::Result<ipc::IPCService> {
        let mut ipc = self.ipc.lock();
        if let Some(service) = ipc.as_ref() {
            return Ok(service.clone());
        }
        let service = ipc::IPCService::new()?;
        *ipc = Some(service.clone());
        Ok(service)
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> AppConfig {
    let config = state.settings.lock();
    config.clone()
}

#[tauri::command]
fn update_config(state: tauri::State<AppState>, new_config: AppConfig) -> Result<(), String> {
    // Save first: the cached settings must keep matching the file when the
    // save fails.
    new_config
        .write()
        .map_err(|error| format!("設定を保存できませんでした: {error}"))?;
    *state.settings.lock() = new_config;

    // A server that is not running reads the saved file when it starts.
    let Ok(mut ipc) = state.ipc() else {
        return Ok(());
    };
    ipc.update_config().map_err(|error| error.to_string())
}

#[tauri::command]
fn reset_learning(state: tauri::State<AppState>) -> Result<(), String> {
    state
        .ipc()
        .and_then(|mut ipc| ipc.reset_learning())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn import_dictionary_file(path: String) -> Result<Vec<UserDictionaryEntry>, String> {
    let path = PathBuf::from(path);
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("辞書ファイルを読み込めませんでした: {error}"))?;
    let source = decode_dictionary_bytes(&bytes)?;
    parse_user_dictionary(&source, dictionary_extension(&path))
}

#[tauri::command]
fn export_dictionary_file(state: tauri::State<AppState>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let config = state.settings.lock();
    let extension = dictionary_extension(&path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let output: Vec<u8> = match extension.as_str() {
        "json" => export_user_dictionary_json(&config.dictionary.entries)?.into_bytes(),
        "txt" => export_user_dictionary_tsv_utf16le(&config.dictionary.entries),
        _ => export_user_dictionary_tsv(&config.dictionary.entries).into_bytes(),
    };
    std::fs::write(&path, output)
        .map_err(|error| format!("辞書ファイルを書き込めませんでした: {error}"))
}

#[tauri::command]
fn validate_tuner_model(path: String) -> Result<(), String> {
    validate_tuner_directory(&PathBuf::from(path))
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Capability {
    cpu: bool,
    cuda: bool,
    vulkan: bool,
}

#[tauri::command]
fn check_capability() -> Capability {
    // cuda:
    // cudart64_12.dll
    // cublas64_12.dll

    // vulkan:
    // vulkan-1.dllの存在確認

    let mut capability = Capability {
        cpu: true,
        cuda: false,
        vulkan: false,
    };

    // Check for CUDA availability
    let cuda_files = ["cudart64_12.dll", "cublas64_12.dll"];
    let cuda_available = cuda_files.iter().all(|file| {
        // Check if the file exists in system path or in the current directory
        std::env::var("PATH")
            .unwrap_or_default()
            .split(';')
            .map(PathBuf::from)
            .chain(std::iter::once(std::env::current_dir().unwrap_or_default()))
            .any(|path| path.join(file).exists())
    });
    capability.cuda = cuda_available;

    // Check for Vulkan availability
    let vulkan_file = "vulkan-1.dll";
    let vulkan_available = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .map(PathBuf::from)
        .chain(std::iter::once(std::env::current_dir().unwrap_or_default()))
        .any(|path| path.join(vulkan_file).exists());
    capability.vulkan = vulkan_available;

    capability
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState::new();

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            update_config,
            reset_learning,
            import_dictionary_file,
            export_dictionary_file,
            validate_tuner_model,
            check_capability
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}