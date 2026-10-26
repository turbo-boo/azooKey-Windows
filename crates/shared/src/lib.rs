use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub mod pipe;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/azookey.rs"));
    include!(concat!(env!("OUT_DIR"), "/window.rs"));
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("azookey_service_descriptor");
}

fn get_config_root() -> PathBuf {
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    appdata.join("Azookey")
}

const SETTINGS_FILENAME: &str = "settings.json";

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct ZenzaiConfig {
    pub enable: bool,
    pub profile: String,
    pub backend: String,
}

impl Default for ZenzaiConfig {
    fn default() -> Self {
        Self {
            enable: false,
            profile: String::new(),
            backend: "vulkan".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct ShortcutConfig {
    pub ctrl_space_toggle: bool,
    pub alt_backquote_toggle: bool,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            ctrl_space_toggle: true,
            alt_backquote_toggle: true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct LearningConfig {
    pub mode: String,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            mode: "inputAndOutput".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
pub struct UserDictionaryEntry {
    pub reading: String,
    pub word: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct UserDictionaryConfig {
    pub entries: Vec<UserDictionaryEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct ThemeConfig {
    pub mode: String,
    pub background_color: String,
    pub text_color: String,
    pub accent_color: String,
    pub selection_background: String,
    pub selection_border: String,
    pub background_image: String,
    pub custom_css_enabled: bool,
    pub custom_css: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            mode: "system".to_string(),
            background_color: String::new(),
            text_color: String::new(),
            accent_color: String::new(),
            selection_background: String::new(),
            selection_border: String::new(),
            background_image: String::new(),
            custom_css_enabled: false,
            custom_css: String::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct PersonalizationConfig {
    pub enabled: bool,
    /// Directory containing Tuner's original_*.marisa and lm_*.marisa files.
    pub tuner_directory: String,
    /// Interpolation strength used by Zenzai personalization (0.0 ... 1.0).
    pub alpha: f32,
}

impl Default for PersonalizationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tuner_directory: String::new(),
            alpha: 0.5,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct LiveConversionConfig {
    pub enabled: bool,
}

impl Default for LiveConversionConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct PredictionConfig {
    pub enabled: bool,
    pub max_candidates: usize,
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_candidates: 5,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct MagicConversionConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub instruction: String,
    pub timeout_seconds: u64,
}

impl Default for MagicConversionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: String::new(),
            instruction: "自然で読みやすい別表現に変換してください。".to_string(),
            timeout_seconds: 10,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub version: String,
    pub zenzai: ZenzaiConfig,
    pub learning: LearningConfig,
    pub shortcuts: ShortcutConfig,
    pub dictionary: UserDictionaryConfig,
    pub theme: ThemeConfig,
    pub personalization: PersonalizationConfig,
    pub live_conversion: LiveConversionConfig,
    pub prediction: PredictionConfig,
    pub magic_conversion: MagicConversionConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
            zenzai: ZenzaiConfig::default(),
            learning: LearningConfig::default(),
            shortcuts: ShortcutConfig::default(),
            dictionary: UserDictionaryConfig::default(),
            theme: ThemeConfig::default(),
            personalization: PersonalizationConfig::default(),
            live_conversion: LiveConversionConfig::default(),
            prediction: PredictionConfig::default(),
            magic_conversion: MagicConversionConfig::default(),
        }
    }
}

/// Parses a settings file. A value of the wrong type (`"alpha": null`, a
/// fractional timeout, a malformed dictionary entry) is replaced by its
/// default on its own, so one bad value does not discard the rest of the
/// user's settings. Only text that is not a JSON object is an error.
fn parse_settings(source: &str) -> serde_json::Result<AppConfig> {
    let file: serde_json::Value = serde_json::from_str(source)?;
    if !file.is_object() {
        return Err(serde::de::Error::custom("the settings are not a JSON object"));
    }
    if let Ok(config) = AppConfig::deserialize(&file) {
        return Ok(config);
    }

    let mut merged = serde_json::to_value(AppConfig::default())?;
    merge_valid_settings(&mut merged, "", &file);
    AppConfig::deserialize(&merged)
}

/// Copies the values of `file` into the object at `pointer` in `merged`
/// (which starts as the defaults), one field at a time, keeping a field's
/// default when the file's value would make the settings invalid. Arrays are
/// merged element by element.
fn merge_valid_settings(merged: &mut serde_json::Value, pointer: &str, file: &serde_json::Value) {
    use serde_json::Value;

    fn is_valid(merged: &Value) -> bool {
        AppConfig::deserialize(merged).is_ok()
    }
    fn slot<'a>(merged: &'a mut Value, pointer: &str) -> &'a mut Value {
        merged.pointer_mut(pointer).expect("field of the defaults")
    }

    let (Some(Value::Object(defaults)), Value::Object(file)) = (merged.pointer(pointer), file)
    else {
        return;
    };
    let keys = defaults.keys().cloned().collect::<Vec<_>>();

    for key in keys {
        let Some(file_value) = file.get(&key) else {
            continue;
        };
        let child = format!("{pointer}/{key}");
        let default = slot(merged, &child);
        if default.is_object() && file_value.is_object() {
            merge_valid_settings(merged, &child, file_value);
        } else if let (true, Value::Array(items)) = (default.is_array(), file_value) {
            // Check each element on its own, so the check does not re-parse
            // the elements accepted so far.
            let mut accepted = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                *slot(merged, &child) = Value::Array(vec![item.clone()]);
                if is_valid(merged) {
                    accepted.push(item.clone());
                } else {
                    eprintln!("Ignoring the invalid setting {child}/{index}");
                }
            }
            *slot(merged, &child) = Value::Array(accepted);
        } else {
            let default = std::mem::replace(default, file_value.clone());
            if !is_valid(merged) {
                eprintln!("Ignoring the invalid setting {child}; using the default");
                *slot(merged, &child) = default;
            }
        }
    }
}

impl AppConfig {
    /// Saves the settings. Readers (IME, UI, server, settings app) may open
    /// the file at any time, so a temporary file replaces the settings in one
    /// step and nobody reads a half-written file. The temporary file is
    /// flushed to disk before the rename: otherwise a power loss can leave the
    /// renamed settings file empty.
    pub fn write(&self) -> std::io::Result<()> {
        use std::io::Write as _;

        let config_root = get_config_root();
        std::fs::create_dir_all(&config_root)?;
        let config_path = config_root.join(SETTINGS_FILENAME);
        let config_str = serde_json::to_string_pretty(self)?;
        let temp_path =
            config_root.join(format!("{SETTINGS_FILENAME}.{}.tmp", std::process::id()));
        let result = std::fs::File::create(&temp_path)
            .and_then(|mut file| {
                file.write_all(config_str.as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| replace_file(&temp_path, &config_path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result
    }

    /// Reads the settings, or the defaults when there is no settings file
    /// yet. A file that cannot be read or is not a JSON object is an error,
    /// so a caller that saves afterwards does not replace it with the
    /// defaults. Single invalid values are replaced by their defaults, see
    /// [`parse_settings`].
    pub fn load() -> std::io::Result<Self> {
        let config_path = get_config_root().join(SETTINGS_FILENAME);
        let config_str = match std::fs::read_to_string(&config_path) {
            Ok(config_str) => config_str,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error),
        };
        Ok(parse_settings(&config_str)?)
    }

    /// The settings for code that only uses them: the defaults when they
    /// cannot be read.
    pub fn read() -> Self {
        Self::load().unwrap_or_else(|error| {
            eprintln!("Failed to read settings: {error}");
            Self::default()
        })
    }

    /// Like [`AppConfig::read`], for callers on a hot path (every key press,
    /// every candidate window update): the file is parsed again only when its
    /// modification time or size changed since the last call in this process.
    pub fn read_cached() -> std::sync::Arc<Self> {
        use std::sync::{Arc, Mutex};
        type Stamp = Option<(std::time::SystemTime, u64)>;
        static CACHE: Mutex<Option<(Stamp, Arc<AppConfig>)>> = Mutex::new(None);

        let stamp: Stamp = std::fs::metadata(get_config_root().join(SETTINGS_FILENAME))
            .and_then(|metadata| Ok((metadata.modified()?, metadata.len())))
            .ok();
        let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_stamp, config)) = cache.as_ref() {
            if stamp.is_some() && *cached_stamp == stamp {
                return config.clone();
            }
        }
        let config = Arc::new(Self::read());
        *cache = Some((stamp, config.clone()));
        config
    }

    pub fn new() -> Self {
        let config = Self::read();
        // Create the file on first run only. Rewriting an existing file here
        // would replace settings that could not be read with the defaults.
        if !get_config_root().join(SETTINGS_FILENAME).exists() {
            if let Err(error) = config.write() {
                eprintln!("Failed to create the settings file: {error}");
            }
        }
        config
    }
}

/// Replaces `dest` with `source`. A reader that has `dest` open without
/// FILE_SHARE_DELETE makes the replacement fail briefly, so retry a few times.
fn replace_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(source, dest) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 10 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct UserDictionaryExport {
    version: u32,
    entries: Vec<UserDictionaryEntry>,
}

/// Whether `text` contains a tab, line break or other control character.
/// Such entries cannot be written to the TSV/TXT dictionary format, whose
/// fields are separated by tabs and entries by line breaks, so they are not
/// valid dictionary entries.
fn has_control_character(text: &str) -> bool {
    text.contains(char::is_control)
}

pub fn normalize_dictionary_entries(
    entries: impl IntoIterator<Item = UserDictionaryEntry>,
) -> Vec<UserDictionaryEntry> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter_map(|entry| {
            let normalized = UserDictionaryEntry {
                reading: entry.reading.trim().to_string(),
                word: entry.word.trim().to_string(),
            };
            if normalized.reading.is_empty()
                || normalized.word.is_empty()
                || has_control_character(&normalized.reading)
                || has_control_character(&normalized.word)
            {
                return None;
            }
            if seen.insert(normalized.clone()) {
                Some(normalized)
            } else {
                None
            }
        })
        .collect()
}

pub fn parse_user_dictionary(
    source: &str,
    extension: Option<&str>,
) -> Result<Vec<UserDictionaryEntry>, String> {
    let extension = extension.unwrap_or_default().to_ascii_lowercase();
    if extension == "json" || source.trim_start().starts_with('{') || source.trim_start().starts_with('[') {
        let entries = if source.trim_start().starts_with('[') {
            serde_json::from_str::<Vec<UserDictionaryEntry>>(source)
                .map_err(|error| format!("JSONの解析に失敗しました: {error}"))?
        } else {
            serde_json::from_str::<UserDictionaryExport>(source)
                .map_err(|error| format!("JSONの解析に失敗しました: {error}"))?
                .entries
        };
        if let Some(entry) = entries.iter().find(|entry| {
            has_control_character(entry.reading.trim()) || has_control_character(entry.word.trim())
        }) {
            return Err(format!(
                "タブや改行を含むエントリは登録できません: {}",
                entry.reading.trim().escape_debug()
            ));
        }
        let entries = normalize_dictionary_entries(entries);
        if entries.is_empty() {
            return Err("有効な辞書エントリがありません".to_string());
        }
        return Ok(entries);
    }

    let entries = source.lines().filter_map(|line| {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            return None;
        }
        let mut fields = line.split('\t');
        let reading = fields.next()?.trim();
        let word = fields.next()?.trim();
        if reading.is_empty() || word.is_empty() {
            return None;
        }
        Some(UserDictionaryEntry {
            reading: reading.to_string(),
            word: word.to_string(),
        })
    });
    let entries = normalize_dictionary_entries(entries);
    if entries.is_empty() {
        Err("TSV内に有効な辞書エントリがありません".to_string())
    } else {
        Ok(entries)
    }
}

pub fn export_user_dictionary_json(entries: &[UserDictionaryEntry]) -> Result<String, String> {
    serde_json::to_string_pretty(&UserDictionaryExport {
        version: 1,
        entries: normalize_dictionary_entries(entries.to_vec()),
    })
    .map_err(|error| error.to_string())
}

pub fn export_user_dictionary_tsv(entries: &[UserDictionaryEntry]) -> String {
    normalize_dictionary_entries(entries.to_vec())
        .into_iter()
        .map(|entry| format!("{}\t{}\t固有名詞\t", entry.reading, entry.word))
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn dictionary_extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|value| value.to_str())
}

pub fn decode_dictionary_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let payload = &bytes[2..];
        if payload.len() % 2 != 0 {
            return Err("UTF-16LE辞書のバイト数が不正です".to_string());
        }
        let units = payload
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map_err(|error| format!("UTF-16LE辞書の解析に失敗しました: {error}"));
    }

    if bytes.starts_with(&[0xFE, 0xFF]) {
        let payload = &bytes[2..];
        if payload.len() % 2 != 0 {
            return Err("UTF-16BE辞書のバイト数が不正です".to_string());
        }
        let units = payload
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map_err(|error| format!("UTF-16BE辞書の解析に失敗しました: {error}"));
    }

    String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("辞書ファイルをUTF-8として解析できませんでした: {error}"))
}

pub fn export_user_dictionary_tsv_utf16le(entries: &[UserDictionaryEntry]) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in export_user_dictionary_tsv(entries).encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

pub const TUNER_REQUIRED_SUFFIXES: [&str; 4] = [
    "_c_abc.marisa",
    "_r_xbx.marisa",
    "_u_abx.marisa",
    "_u_xbc.marisa",
];

pub fn validate_tuner_directory(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("指定されたTunerディレクトリが存在しません".to_string());
    }
    for prefix in ["original", "lm"] {
        for suffix in TUNER_REQUIRED_SUFFIXES {
            let file = path.join(format!("{prefix}{suffix}"));
            if !file.is_file() {
                return Err(format!("必要なモデルファイルがありません: {}", file.display()));
            }
        }
    }
    Ok(())
}

/// `host` is the host as written in the URL (`Url::host_str`), so an IPv6
/// address comes in brackets: `[::1]`.
pub fn validate_magic_endpoint_parts(scheme: &str, host: Option<&str>) -> Result<(), String> {
    match scheme {
        "https" => Ok(()),
        "http" => {
            let host = host.unwrap_or_default();
            let ip_address = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host)
                .parse::<std::net::IpAddr>();
            if host.eq_ignore_ascii_case("localhost")
                || ip_address.is_ok_and(|address| address.is_loopback())
            {
                Ok(())
            } else {
                Err("HTTPはlocalhostのAPIにだけ使用できます。外部APIにはHTTPSを使用してください".to_string())
            }
        }
        _ => Err("APIエンドポイントはHTTPまたはHTTPSで指定してください".to_string()),
    }
}

pub fn parse_magic_candidates(value: &serde_json::Value) -> Result<Vec<String>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_endpoint_allows_https_and_local_http_only() {
        assert!(validate_magic_endpoint_parts("https", Some("example.com")).is_ok());
        assert!(validate_magic_endpoint_parts("http", Some("localhost")).is_ok());
        assert!(validate_magic_endpoint_parts("http", Some("127.0.0.1")).is_ok());
        assert!(validate_magic_endpoint_parts("http", Some("::1")).is_ok());
        assert!(validate_magic_endpoint_parts("http", Some("example.com")).is_err());
        assert!(validate_magic_endpoint_parts("ftp", Some("localhost")).is_err());
        assert!(validate_magic_endpoint_parts("http", Some("[::1]")).is_ok());
        assert!(validate_magic_endpoint_parts("http", Some("[2001:db8::1]")).is_err());
        assert!(validate_magic_endpoint_parts("http", Some("[::1")).is_err());
        assert!(validate_magic_endpoint_parts("http", Some("127.0.0.1.example.com")).is_err());
        assert!(validate_magic_endpoint_parts("http", None).is_err());
    }

    #[test]
    fn magic_candidates_parse_json_dedupe_and_limit() {
        let value = serde_json::json!({
            "choices": [
                {"message": {"content": "{\"predictions\":[\"候補A\",\"候補B\",\"候補A\",\"3\",\"4\",\"5\",\"6\"]}"}}
            ]
        });
        assert_eq!(
            parse_magic_candidates(&value).unwrap(),
            vec![
                "候補A".to_string(),
                "候補B".to_string(),
                "3".to_string(),
                "4".to_string(),
                "5".to_string()
            ]
        );
    }

    #[test]
    fn magic_candidates_accept_plain_text_and_reject_missing_choices() {
        let value = serde_json::json!({
            "choices": [{"message": {"content": "自然な候補"}}]
        });
        assert_eq!(
            parse_magic_candidates(&value).unwrap(),
            vec!["自然な候補".to_string()]
        );
        assert!(parse_magic_candidates(&serde_json::json!({})).is_err());
    }

    #[test]
    fn default_config_prefers_vulkan_backend() {
        assert_eq!(AppConfig::default().zenzai.backend, "vulkan");
    }

    #[test]
    fn legacy_config_deserializes_with_new_defaults() {
        let config: AppConfig = serde_json::from_str(
            r#"{
                "version":"0.0.1",
                "zenzai":{"enable":false,"profile":"","backend":"cpu"}
            }"#,
        )
        .unwrap();
        assert_eq!(config.learning.mode, "inputAndOutput");
        assert!(config.shortcuts.ctrl_space_toggle);
        assert!(config.shortcuts.alt_backquote_toggle);
        assert_eq!(config.theme.mode, "system");
        assert!(!config.personalization.enabled);
        assert!(config.live_conversion.enabled);
        assert!(!config.prediction.enabled);
        assert_eq!(config.prediction.max_candidates, 5);
        assert!(!config.magic_conversion.enabled);
        assert_eq!(config.magic_conversion.timeout_seconds, 10);
    }

    #[test]
    fn invalid_settings_values_fall_back_one_by_one() {
        let config = parse_settings(
            r#"{
                "zenzai":{"enable":true,"profile":"me","backend":7},
                "personalization":{"enabled":true,"alpha":null},
                "magic_conversion":{"api_key":"key","timeout_seconds":1.5,"model":"m"},
                "dictionary":{"entries":[
                    {"reading":"あ","word":"亜"},
                    {"reading":"い"},
                    {"reading":"う","word":"宇"}
                ]},
                "theme":5,
                "shortcuts":{"ctrl_space_toggle":false}
            }"#,
        )
        .unwrap();

        assert_eq!(config.version, "0.1.0");
        assert!(config.zenzai.enable);
        assert_eq!(config.zenzai.profile, "me");
        assert_eq!(config.zenzai.backend, "vulkan");
        assert!(config.personalization.enabled);
        assert_eq!(config.personalization.alpha, 0.5);
        assert_eq!(config.magic_conversion.api_key, "key");
        assert_eq!(config.magic_conversion.model, "m");
        assert_eq!(config.magic_conversion.timeout_seconds, 10);
        assert_eq!(
            config.dictionary.entries,
            vec![
                UserDictionaryEntry { reading: "あ".into(), word: "亜".into() },
                UserDictionaryEntry { reading: "う".into(), word: "宇".into() },
            ]
        );
        assert_eq!(config.theme.mode, "system");
        assert!(!config.shortcuts.ctrl_space_toggle);
        assert!(config.shortcuts.alt_backquote_toggle);
    }

    #[test]
    fn settings_that_are_not_a_json_object_are_an_error() {
        assert!(parse_settings("").is_err());
        assert!(parse_settings("{\"version\":").is_err());
        assert!(parse_settings("[]").is_err());
    }

    #[test]
    fn empty_settings_object_gives_the_defaults() {
        let config = parse_settings("{}").unwrap();
        assert_eq!(config.version, "0.1.0");
        assert_eq!(config.zenzai.backend, "vulkan");
    }

    #[test]
    fn partial_nested_config_uses_section_defaults() {
        let config: AppConfig = serde_json::from_str(
            r#"{
                "version":"0.1.0",
                "zenzai":{"enable":true,"profile":"test","backend":"cpu"},
                "theme":{"mode":"dark"},
                "shortcuts":{"ctrl_space_toggle":false},
                "prediction":{"enabled":true},
                "magic_conversion":{"enabled":true,"endpoint":"http://127.0.0.1:8080/v1/chat/completions"}
            }"#,
        )
        .unwrap();

        assert_eq!(config.theme.mode, "dark");
        assert_eq!(config.theme.text_color, "");
        assert!(!config.shortcuts.ctrl_space_toggle);
        assert!(config.shortcuts.alt_backquote_toggle);
        assert!(config.live_conversion.enabled);
        assert!(config.prediction.enabled);
        assert_eq!(config.prediction.max_candidates, 5);
        assert!(config.magic_conversion.enabled);
        assert_eq!(config.magic_conversion.model, "gpt-4o-mini");
        assert_eq!(config.magic_conversion.timeout_seconds, 10);
    }

    #[test]
    fn dictionary_json_roundtrip_is_lossless_and_deduplicated() {
        let entries = vec![
            UserDictionaryEntry { reading: " あずーきー ".into(), word: " azooKey ".into() },
            UserDictionaryEntry { reading: "あずーきー".into(), word: "azooKey".into() },
        ];
        let json = export_user_dictionary_json(&entries).unwrap();
        let parsed = parse_user_dictionary(&json, Some("json")).unwrap();
        assert_eq!(
            parsed,
            vec![UserDictionaryEntry {
                reading: "あずーきー".into(),
                word: "azooKey".into()
            }]
        );
    }

    #[test]
    fn dictionary_tsv_supports_ime_style_columns() {
        let parsed = parse_user_dictionary(
            "!Microsoft IME Dictionary Tool\r\nあずーきー\tazooKey\t固有名詞\tcomment\r\n",
            Some("txt"),
        )
        .unwrap();
        assert_eq!(parsed[0].reading, "あずーきー");
        assert_eq!(parsed[0].word, "azooKey");
    }

    #[test]
    fn dictionary_parser_rejects_empty_input() {
        assert!(parse_user_dictionary("\n# comment\n", Some("tsv")).is_err());
    }

    #[test]
    fn dictionary_text_decoding_supports_utf8_and_utf16() {
        let source = "あずーきー\tazooKey\t固有名詞\t";
        assert_eq!(decode_dictionary_bytes(source.as_bytes()).unwrap(), source);

        let mut le = vec![0xFF, 0xFE];
        for unit in source.encode_utf16() {
            le.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_dictionary_bytes(&le).unwrap(), source);

        let mut be = vec![0xFE, 0xFF];
        for unit in source.encode_utf16() {
            be.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_dictionary_bytes(&be).unwrap(), source);
    }

    #[test]
    fn ime_text_export_has_utf16le_bom_and_roundtrips() {
        let entries = [UserDictionaryEntry {
            reading: "あずーきー".into(),
            word: "azooKey".into(),
        }];
        let bytes = export_user_dictionary_tsv_utf16le(&entries);
        assert!(bytes.starts_with(&[0xFF, 0xFE]));
        let decoded = decode_dictionary_bytes(&bytes).unwrap();
        assert!(decoded.contains("あずーきー\tazooKey"));
    }

    #[test]
    fn tuner_directory_requires_all_base_and_personal_models() {
        let root = std::env::temp_dir().join(format!("azookey-tuner-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(validate_tuner_directory(&root).is_err());

        for prefix in ["original", "lm"] {
            for suffix in TUNER_REQUIRED_SUFFIXES {
                std::fs::write(root.join(format!("{prefix}{suffix}")), b"test").unwrap();
            }
        }

        assert!(validate_tuner_directory(&root).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn malformed_json_dictionary_is_rejected() {
        assert!(parse_user_dictionary("{not-json}", Some("json")).is_err());
    }

    #[test]
    fn every_accepted_entry_survives_a_text_export_round_trip() {
        let entries = vec![
            UserDictionaryEntry { reading: "あずーきー".into(), word: "azooKey".into() },
            UserDictionaryEntry { reading: "てすと".into(), word: "a\tb".into() },
            UserDictionaryEntry { reading: "かいぎょう".into(), word: "a\r\nb".into() },
            UserDictionaryEntry { reading: "えもじ".into(), word: "😀 と 空白".into() },
        ];
        let accepted = normalize_dictionary_entries(entries);
        assert_eq!(accepted.len(), 2);

        let tsv = export_user_dictionary_tsv(&accepted);
        assert_eq!(parse_user_dictionary(&tsv, Some("tsv")).unwrap(), accepted);
        let txt = decode_dictionary_bytes(&export_user_dictionary_tsv_utf16le(&accepted)).unwrap();
        assert_eq!(parse_user_dictionary(&txt, Some("txt")).unwrap(), accepted);
    }

    #[test]
    fn json_dictionary_with_tabs_or_line_breaks_is_rejected() {
        assert!(parse_user_dictionary(
            r#"{"version":1,"entries":[{"reading":"あ","word":"a\tb"}]}"#,
            Some("json"),
        )
        .is_err());
        assert!(parse_user_dictionary(r#"[{"reading":"あ\nい","word":"亜"}]"#, None).is_err());
    }

    #[test]
    fn tsv_export_contains_reading_and_word() {
        let output = export_user_dictionary_tsv(&[UserDictionaryEntry {
            reading: "あずーきー".into(),
            word: "azooKey".into(),
        }]);
        assert_eq!(output, "あずーきー\tazooKey\t固有名詞\t");
    }
}
