use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
pub struct ZenzaiConfig {
    pub enable: bool,
    pub profile: String,
    pub backend: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
pub struct UserDictionaryConfig {
    pub entries: Vec<UserDictionaryEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
pub struct AppConfig {
    pub version: String,
    pub zenzai: ZenzaiConfig,
    #[serde(default)]
    pub learning: LearningConfig,
    #[serde(default)]
    pub dictionary: UserDictionaryConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub personalization: PersonalizationConfig,
    #[serde(default)]
    pub prediction: PredictionConfig,
    #[serde(default)]
    pub magic_conversion: MagicConversionConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
            zenzai: ZenzaiConfig {
                enable: false,
                profile: String::new(),
                backend: "cpu".to_string(),
            },
            learning: LearningConfig::default(),
            dictionary: UserDictionaryConfig::default(),
            theme: ThemeConfig::default(),
            personalization: PersonalizationConfig::default(),
            prediction: PredictionConfig::default(),
            magic_conversion: MagicConversionConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn write(&self) {
        let config_root = get_config_root();
        if let Err(error) = std::fs::create_dir_all(&config_root) {
            eprintln!("Failed to create config directory: {error}");
            return;
        }
        let config_path = config_root.join(SETTINGS_FILENAME);
        let Ok(config_str) = serde_json::to_string_pretty(self) else {
            eprintln!("Failed to serialize settings");
            return;
        };
        if let Err(error) = std::fs::write(config_path, config_str) {
            eprintln!("Failed to write settings: {error}");
        }
    }

    pub fn read() -> Self {
        let config_path = get_config_root().join(SETTINGS_FILENAME);
        if !config_path.exists() {
            return Self::default();
        }
        let Ok(config_str) = std::fs::read_to_string(&config_path) else {
            eprintln!("Failed to read settings from {}", config_path.display());
            return Self::default();
        };
        match serde_json::from_str(&config_str) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("Failed to parse settings: {error}");
                Self::default()
            }
        }
    }

    pub fn new() -> Self {
        let config = Self::read();
        config.write();
        config
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct UserDictionaryExport {
    version: u32,
    entries: Vec<UserDictionaryEntry>,
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
            if normalized.reading.is_empty() || normalized.word.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config.theme.mode, "system");
        assert!(!config.personalization.enabled);
        assert!(!config.prediction.enabled);
        assert_eq!(config.prediction.max_candidates, 5);
        assert!(!config.magic_conversion.enabled);
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
    fn tsv_export_contains_reading_and_word() {
        let output = export_user_dictionary_tsv(&[UserDictionaryEntry {
            reading: "あずーきー".into(),
            word: "azooKey".into(),
        }]);
        assert_eq!(output, "あずーきー\tazooKey\t固有名詞\t");
    }
}
