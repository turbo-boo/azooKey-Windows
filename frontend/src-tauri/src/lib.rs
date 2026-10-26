mod ipc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::{
    decode_dictionary_bytes, dictionary_extension, export_user_dictionary_json,
    export_user_dictionary_tsv, export_user_dictionary_tsv_utf16le, normalize_dictionary_entries,
    parse_user_dictionary, validate_tuner_directory, AppConfig, UserDictionaryEntry,
};
use std::{fs::File, path::PathBuf};
use tauri::{AppHandle, Manager};

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// The result of a command that saved the settings.
#[derive(Serialize)]
struct Saved<T> {
    value: T,
    /// Why the running converter server may still use the previous settings.
    warning: Option<String>,
}

#[derive(Serialize)]
struct DictionaryChange {
    entries: Vec<UserDictionaryEntry>,
    added: usize,
}

/// Holds the settings file for one read-modify-write cycle. Several settings
/// windows (separate processes) may run at once; each applies its change to
/// the file as it is now instead of saving a copy loaded earlier, which would
/// drop the changes the other windows made since.
fn lock_settings(app: &AppHandle) -> Result<File, String> {
    let lock_error =
        |error: &dyn std::fmt::Display| format!("設定ファイルをロックできませんでした: {error}");
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| lock_error(&error))?;
    std::fs::create_dir_all(&dir).map_err(|error| lock_error(&error))?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("settings.lock"))
        .map_err(|error| lock_error(&error))?;
    // Released when the file is closed.
    file.lock().map_err(|error| lock_error(&error))?;
    Ok(file)
}

fn load_settings() -> Result<AppConfig, String> {
    AppConfig::load().map_err(|error| format!("設定を読み込めませんでした: {error}"))
}

/// Applies `change` to the current settings file, saves it and tells the
/// converter server to reload it.
async fn modify_settings<T: Send + 'static>(
    app: AppHandle,
    change: impl FnOnce(&mut AppConfig) -> Result<T, String> + Send + 'static,
) -> Result<Saved<T>, String> {
    let value = tauri::async_runtime::spawn_blocking(move || {
        let _lock = lock_settings(&app)?;
        let mut config = load_settings()?;
        let value = change(&mut config)?;
        config
            .write()
            .map_err(|error| format!("設定を保存できませんでした: {error}"))?;
        Ok::<_, String>(value)
    })
    .await
    .map_err(|error| error.to_string())??;

    let warning = notify_server()
        .await
        .err()
        .map(|error| format!("設定は保存しましたが、変換サーバーへ反映できませんでした: {error}"));
    Ok(Saved { value, warning })
}

/// Asks the converter server to reload the settings file.
async fn notify_server() -> anyhow::Result<()> {
    let mut ipc = match ipc::IPCService::connect().await {
        Ok(ipc) => ipc,
        // A server that is not running reads the saved file when it starts.
        Err(ipc::ConnectError::NotRunning) => return Ok(()),
        Err(ipc::ConnectError::Failed(error)) => return Err(error),
    };
    ipc.update_config().await
}

/// Applies a JSON merge patch (RFC 7396) to `target`.
fn merge_patch(target: &mut Value, patch: Value) {
    let Value::Object(patch) = patch else {
        *target = patch;
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Default::default());
    }
    let Value::Object(target) = target else {
        unreachable!()
    };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(&key);
        } else {
            merge_patch(target.entry(key).or_insert(Value::Null), value);
        }
    }
}

// Commands that touch files or wait for the converter server are async: Tauri
// runs synchronous commands on the main thread, which would freeze the window
// while they wait.
#[tauri::command]
async fn get_config() -> Result<AppConfig, String> {
    tauri::async_runtime::spawn_blocking(load_settings)
        .await
        .map_err(|error| error.to_string())?
}

/// Changes only the settings named in `patch` (a JSON merge patch) and
/// returns all saved settings.
#[tauri::command]
async fn update_config(app: AppHandle, patch: Value) -> Result<Saved<AppConfig>, String> {
    modify_settings(app, move |config| {
        let mut value = serde_json::to_value(&*config).map_err(|error| error.to_string())?;
        merge_patch(&mut value, patch);
        *config = serde_json::from_value(value)
            .map_err(|error| format!("設定の値が正しくありません: {error}"))?;
        Ok(config.clone())
    })
    .await
}

#[tauri::command]
async fn add_dictionary_entries(
    app: AppHandle,
    entries: Vec<UserDictionaryEntry>,
) -> Result<Saved<DictionaryChange>, String> {
    modify_settings(app, move |config| {
        let current = normalize_dictionary_entries(std::mem::take(&mut config.dictionary.entries));
        let before = current.len();
        config.dictionary.entries =
            normalize_dictionary_entries(current.into_iter().chain(entries));
        Ok(DictionaryChange {
            entries: config.dictionary.entries.clone(),
            added: config.dictionary.entries.len() - before,
        })
    })
    .await
}

#[tauri::command]
async fn remove_dictionary_entry(
    app: AppHandle,
    entry: UserDictionaryEntry,
) -> Result<Saved<Vec<UserDictionaryEntry>>, String> {
    modify_settings(app, move |config| {
        config
            .dictionary
            .entries
            .retain(|current| *current != entry);
        Ok(config.dictionary.entries.clone())
    })
    .await
}

#[tauri::command]
async fn reset_learning() -> Result<(), String> {
    let mut ipc = match ipc::IPCService::connect().await {
        Ok(ipc) => ipc,
        Err(ipc::ConnectError::NotRunning) => {
            return Err("変換サーバーが起動していません".to_string())
        }
        Err(ipc::ConnectError::Failed(error)) => return Err(error.to_string()),
    };
    ipc.reset_learning()
        .await
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
fn export_dictionary_file(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let config = load_settings()?;
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            update_config,
            add_dictionary_entries,
            remove_dictionary_entry,
            reset_learning,
            import_dictionary_file,
            export_dictionary_file,
            validate_tuner_model,
            check_capability
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
