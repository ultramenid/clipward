//! Settings store (port of src/main/settings-store.ts).
//!
//! Clipward is bring-your-own-key: every provider call is made from this machine
//! with the user's own keys. Keys never touch settings.json: they live in the OS
//! keychain (macOS Keychain, Windows Credential Manager, Secret Service on Linux),
//! as Electron's safeStorage kept them. When no keychain is available, saving a key
//! fails rather than falling back to plain text. Keys never reach the renderer:
//! it only sees `<provider>Configured` flags.
use crate::logger;
use crate::shared::{is_whisper_model_id, DEFAULT_WHISPER_MODEL};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

const SETTINGS_VERSION: u32 = 1;
const KEYCHAIN_SERVICE: &str = "com.clipward.desktop";

pub const SECRET_KEYS: [&str; 3] = ["openrouterApiKey", "anthropicApiKey", "openaiApiKey"];

static AI_PROVIDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(openrouter|anthropic|openai|cli:[a-z-]{1,32})$").unwrap());
static AI_MODEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._:/@-]{0,119})?$").unwrap());

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub openrouter_api_key: String,
    pub anthropic_api_key: String,
    pub openai_api_key: String,
    /// 'openrouter' | 'anthropic' | 'openai' | 'cli:<id>' (a logged-in CLI).
    pub ai_provider: String,
    /// Optional model override; empty uses the provider's default.
    pub ai_model: String,
    pub whisper_model: String,
    pub output_directory: String,
    /// Kept for the renderer's settings shape; the Rust engine runs in-process.
    pub python_path: String,
    /// Names and jargon the speech-to-text should spell correctly, one per line.
    pub custom_vocabulary: String,
    /// Native window glass (macOS vibrancy, Windows Mica/acrylic). Not a secret;
    /// ignored where the platform has none (see window_glass.rs).
    pub window_glass: bool,
}

impl AppSettings {
    fn secret(&self, key: &str) -> &str {
        match key {
            "openrouterApiKey" => &self.openrouter_api_key,
            "anthropicApiKey" => &self.anthropic_api_key,
            _ => &self.openai_api_key,
        }
    }

    fn secret_mut(&mut self, key: &str) -> &mut String {
        match key {
            "openrouterApiKey" => &mut self.openrouter_api_key,
            "anthropicApiKey" => &mut self.anthropic_api_key,
            _ => &mut self.openai_api_key,
        }
    }

    /// The key of the selected API provider; None for a CLI provider.
    pub fn provider_key(&self) -> Option<&str> {
        match self.ai_provider.as_str() {
            "openrouter" => Some(&self.openrouter_api_key),
            "anthropic" => Some(&self.anthropic_api_key),
            "openai" => Some(&self.openai_api_key),
            _ => None,
        }
    }

    /// Whether the selected API provider has no key (a CLI is checked separately).
    pub fn provider_key_missing(&self) -> bool {
        self.provider_key().is_some_and(str::is_empty)
    }

    /// Only the selected provider's key reaches the engine. A CLI provider gets
    /// none: an ANTHROPIC_API_KEY would make Claude Code bill that key instead of
    /// the user's logged-in plan.
    pub fn engine_ai(&self, cli_path: Option<PathBuf>) -> clip_engine::AiSettings {
        let key = |provider: &str, value: &str| {
            (self.ai_provider == provider && !value.is_empty()).then(|| value.to_string())
        };
        clip_engine::AiSettings {
            provider: self.ai_provider.clone(),
            model: self.ai_model.clone(),
            openrouter_api_key: key("openrouter", &self.openrouter_api_key),
            anthropic_api_key: key("anthropic", &self.anthropic_api_key),
            openai_api_key: key("openai", &self.openai_api_key),
            cli_path,
        }
    }
}

/// The settings the renderer sees (ClipSettings in src/shared/api.ts).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    pub output_directory: String,
    pub python_path: String,
    pub custom_vocabulary: String,
    pub ai_provider: String,
    pub ai_model: String,
    pub whisper_model: String,
    pub openrouter_configured: bool,
    pub anthropic_configured: bool,
    pub openai_configured: bool,
    pub window_glass: bool,
    /// Whether this platform can draw window glass; the renderer stays opaque otherwise.
    pub window_glass_supported: bool,
}

/// The fields the renderer may change with settings:save.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableSettings {
    pub output_directory: String,
    pub python_path: String,
    pub custom_vocabulary: String,
    pub ai_provider: String,
    pub ai_model: String,
    pub whisper_model: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PersistedSettings {
    version: u32,
    ai_provider: String,
    ai_model: String,
    whisper_model: String,
    output_directory: String,
    python_path: String,
    #[serde(default)]
    custom_vocabulary: String,
    /// Which API keys are saved in the keychain, so loading never touches the
    /// keychain (and never prompts) for a key that was never saved.
    #[serde(default)]
    stored_keys: Vec<String>,
    /// Missing in files written before window glass existed: on by default.
    #[serde(default = "default_window_glass")]
    window_glass: bool,
}

fn default_window_glass() -> bool {
    true
}

pub fn public_settings(settings: &AppSettings) -> PublicSettings {
    PublicSettings {
        output_directory: settings.output_directory.clone(),
        python_path: settings.python_path.clone(),
        custom_vocabulary: settings.custom_vocabulary.clone(),
        ai_provider: settings.ai_provider.clone(),
        ai_model: settings.ai_model.clone(),
        whisper_model: settings.whisper_model.clone(),
        openrouter_configured: !settings.openrouter_api_key.is_empty(),
        anthropic_configured: !settings.anthropic_api_key.is_empty(),
        openai_configured: !settings.openai_api_key.is_empty(),
        window_glass: settings.window_glass,
        window_glass_supported: crate::window_glass::SUPPORTED,
    }
}

/// Conservative vocabulary hints for Whisper: one term per line or comma, at most
/// five words and 49 characters each, none of <>{}[]\, deduplicated
/// case-insensitively, at most 200 terms.
pub fn vocabulary_terms(value: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    for line in value.split(['\n', ',']) {
        let term = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if term.is_empty()
            || term.encode_utf16().count() > 49
            || term.split(' ').count() > 5
            || term.contains(['<', '>', '{', '}', '[', ']', '\\'])
        {
            continue;
        }
        if !seen.insert(term.to_lowercase()) {
            continue;
        }
        terms.push(term);
        if terms.len() >= 200 {
            break;
        }
    }
    terms
}

pub struct SettingsStore {
    path: PathBuf,
    legacy_path: Option<PathBuf>,
    defaults: AppSettings,
    cache: Mutex<Option<AppSettings>>,
}

impl SettingsStore {
    pub fn new(user_data: &Path, legacy_user_data: Option<&Path>, home: &Path) -> Self {
        Self {
            path: user_data.join("settings.json"),
            legacy_path: legacy_user_data.map(|dir| dir.join("settings.json")),
            defaults: AppSettings {
                openrouter_api_key: String::new(),
                anthropic_api_key: String::new(),
                openai_api_key: String::new(),
                ai_provider: "openrouter".into(),
                ai_model: String::new(),
                whisper_model: DEFAULT_WHISPER_MODEL.into(),
                output_directory: home.join("Clipward").to_string_lossy().into_owned(),
                python_path: if cfg!(windows) {
                    "python".into()
                } else {
                    "python3".into()
                },
                custom_vocabulary: String::new(),
                window_glass: default_window_glass(),
            },
            cache: Mutex::new(None),
        }
    }

    fn normalize(&self, settings: AppSettings) -> Result<AppSettings, String> {
        let fields = [
            ("openrouterApiKey", &settings.openrouter_api_key),
            ("anthropicApiKey", &settings.anthropic_api_key),
            ("openaiApiKey", &settings.openai_api_key),
            ("aiProvider", &settings.ai_provider),
            ("aiModel", &settings.ai_model),
            ("whisperModel", &settings.whisper_model),
            ("outputDirectory", &settings.output_directory),
            ("pythonPath", &settings.python_path),
            ("customVocabulary", &settings.custom_vocabulary),
        ];
        for (name, value) in fields {
            if value.encode_utf16().count() > 8192 || value.contains('\0') {
                return Err(format!("Invalid {name}"));
            }
        }
        let or_default = |value: &str, default: &str| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        };
        let normalized = AppSettings {
            openrouter_api_key: settings.openrouter_api_key.trim().into(),
            anthropic_api_key: settings.anthropic_api_key.trim().into(),
            openai_api_key: settings.openai_api_key.trim().into(),
            ai_provider: or_default(&settings.ai_provider, &self.defaults.ai_provider),
            ai_model: settings.ai_model.trim().into(),
            // A model dropped from the list falls back to the default instead of breaking settings.
            whisper_model: if is_whisper_model_id(&settings.whisper_model) {
                settings.whisper_model
            } else {
                self.defaults.whisper_model.clone()
            },
            output_directory: or_default(
                &settings.output_directory,
                &self.defaults.output_directory,
            ),
            python_path: or_default(&settings.python_path, &self.defaults.python_path),
            custom_vocabulary: vocabulary_terms(&settings.custom_vocabulary).join("\n"),
            window_glass: settings.window_glass,
        };
        if !Path::new(&normalized.output_directory).is_absolute() {
            return Err("Settings folders must be absolute paths".into());
        }
        if !AI_PROVIDER.is_match(&normalized.ai_provider) {
            return Err("Invalid AI provider".into());
        }
        if !AI_MODEL.is_match(&normalized.ai_model) {
            return Err("Invalid AI model name".into());
        }
        Ok(normalized)
    }

    /// Current settings, read once from disk and the keychain, then cached.
    pub fn load(&self) -> Result<AppSettings, String> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "Settings are unavailable".to_string())?;
        if let Some(settings) = cache.as_ref() {
            return Ok(settings.clone());
        }
        let settings = self.read_from_disk()?;
        *cache = Some(settings.clone());
        Ok(settings)
    }

    fn read_from_disk(&self) -> Result<AppSettings, String> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.migrate_legacy());
            }
            Err(_) => {
                return Err(
                    "Could not read saved settings. The settings file was kept for recovery."
                        .into(),
                )
            }
        };
        let persisted: PersistedSettings = serde_json::from_str(&raw).map_err(|_| {
            "Could not read saved settings. The settings file was kept for recovery.".to_string()
        })?;
        let mut settings = AppSettings {
            ai_provider: persisted.ai_provider,
            ai_model: persisted.ai_model,
            whisper_model: persisted.whisper_model,
            output_directory: persisted.output_directory,
            python_path: persisted.python_path,
            custom_vocabulary: persisted.custom_vocabulary,
            window_glass: persisted.window_glass,
            ..self.defaults.clone()
        };
        for key in SECRET_KEYS {
            if persisted.stored_keys.iter().any(|stored| stored == key) {
                *settings.secret_mut(key) = keychain_get(key).map_err(|_| {
                    "Could not decrypt saved API keys. Unlock the system keychain and retry."
                        .to_string()
                })?;
            }
        }
        self.normalize(settings).map_err(|_| {
            "Could not read saved settings. The settings file was kept for recovery.".to_string()
        })
    }

    /// First run after the Electron app: carry over its non-secret settings. Its
    /// API keys were encrypted with Electron's safeStorage and must be entered again.
    fn migrate_legacy(&self) -> AppSettings {
        let Some(raw) = self
            .legacy_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return self.defaults.clone();
        };
        let Ok(Value::Object(old)) = serde_json::from_str::<Value>(&raw) else {
            return self.defaults.clone();
        };
        let text = |name: &str, default: &str| {
            old.get(name)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        let candidate = AppSettings {
            ai_provider: text("aiProvider", &self.defaults.ai_provider),
            ai_model: text("aiModel", ""),
            whisper_model: text("whisperModel", &self.defaults.whisper_model),
            output_directory: text("outputDirectory", &self.defaults.output_directory),
            custom_vocabulary: text("customVocabulary", ""),
            ..self.defaults.clone()
        };
        match self.normalize(candidate) {
            Ok(settings) => {
                let had_keys = SECRET_KEYS
                    .iter()
                    .any(|key| old.get(*key).is_some_and(|v| v != "" && !v.is_null()));
                if self.write(&settings).is_ok() {
                    logger::info(
                        "settings.migrated",
                        json!({ "apiKeysNeedReentry": had_keys }),
                    );
                }
                settings
            }
            Err(_) => self.defaults.clone(),
        }
    }

    fn write(&self, settings: &AppSettings) -> Result<(), String> {
        let persisted = PersistedSettings {
            version: SETTINGS_VERSION,
            ai_provider: settings.ai_provider.clone(),
            ai_model: settings.ai_model.clone(),
            whisper_model: settings.whisper_model.clone(),
            output_directory: settings.output_directory.clone(),
            python_path: settings.python_path.clone(),
            custom_vocabulary: settings.custom_vocabulary.clone(),
            window_glass: settings.window_glass,
            stored_keys: SECRET_KEYS
                .iter()
                .filter(|key| !settings.secret(key).is_empty())
                .map(|key| key.to_string())
                .collect(),
        };
        if let Some(dir) = self.path.parent() {
            crate::paths::private_dir(dir).map_err(|_| "Could not save settings".to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&persisted)
            .map_err(|_| "Could not save settings".to_string())?;
        crate::paths::write_atomic(&self.path, &bytes)
            .map_err(|_| "Could not save settings".to_string())
    }

    fn save(&self, settings: AppSettings) -> Result<AppSettings, String> {
        let normalized = self.normalize(settings)?;
        self.write(&normalized)?;
        *self
            .cache
            .lock()
            .map_err(|_| "Settings are unavailable".to_string())? = Some(normalized.clone());
        Ok(normalized)
    }

    pub fn save_public(&self, update: EditableSettings) -> Result<PublicSettings, String> {
        let current = self.load()?;
        let saved = self.save(AppSettings {
            output_directory: update.output_directory,
            python_path: update.python_path,
            custom_vocabulary: update.custom_vocabulary,
            ai_provider: update.ai_provider,
            ai_model: update.ai_model,
            whisper_model: update.whisper_model,
            ..current
        })?;
        Ok(public_settings(&saved))
    }

    /// Remember whether the window draws native glass.
    pub fn set_window_glass(&self, enabled: bool) -> Result<PublicSettings, String> {
        let current = self.load()?;
        Ok(public_settings(&self.save(AppSettings {
            window_glass: enabled,
            ..current
        })?))
    }

    pub fn replace_api_key(&self, key: &str, value: &str) -> Result<PublicSettings, String> {
        if !SECRET_KEYS.contains(&key)
            || value.encode_utf16().count() > 8192
            || value.contains('\0')
        {
            return Err("Invalid API key update".into());
        }
        let mut next = self.load()?;
        let value = value.trim();
        if value.is_empty() {
            keychain_delete(key)?;
        } else {
            keychain_set(key, value)?;
        }
        *next.secret_mut(key) = value.to_string();
        Ok(public_settings(&self.save(next)?))
    }
}

const KEYCHAIN_UNAVAILABLE: &str =
    "Secure key storage is unavailable. Unlock or configure your operating system keychain before saving API keys.";

fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|_| KEYCHAIN_UNAVAILABLE.to_string())
}

fn keychain_get(key: &str) -> Result<String, keyring::Error> {
    match keyring::Entry::new(KEYCHAIN_SERVICE, key)?.get_password() {
        Ok(value) => Ok(value),
        Err(keyring::Error::NoEntry) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

fn keychain_set(key: &str, value: &str) -> Result<(), String> {
    entry(key)?
        .set_password(value)
        .map_err(|_| KEYCHAIN_UNAVAILABLE.to_string())
}

fn keychain_delete(key: &str) -> Result<(), String> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(KEYCHAIN_UNAVAILABLE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_is_bounded_and_deduplicated() {
        let terms = vocabulary_terms(
            "Clipward, clipward\n  Open   AI \n<bad>\none two three four five six",
        );
        assert_eq!(terms, vec!["Clipward", "Open AI"]);
    }

    #[test]
    fn window_glass_defaults_on_and_round_trips() {
        let old: PersistedSettings = serde_json::from_str(
            r#"{"version":1,"aiProvider":"openrouter","aiModel":"","whisperModel":"base","outputDirectory":"/x","pythonPath":"python3"}"#,
        )
        .unwrap();
        assert!(old.window_glass);

        let dir = std::env::temp_dir().join(format!("clipward-glass-{}", uuid::Uuid::new_v4()));
        let home = std::env::temp_dir();
        let store = SettingsStore::new(&dir, None, &home);
        assert!(store.load().unwrap().window_glass);
        let public = store.set_window_glass(false).unwrap();
        assert!(!public.window_glass);
        assert_eq!(
            public.window_glass_supported,
            crate::window_glass::SUPPORTED
        );
        // A fresh store reads the choice back from disk.
        assert!(
            !SettingsStore::new(&dir, None, &home)
                .load()
                .unwrap()
                .window_glass
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn normalize_rejects_bad_provider() {
        let store = SettingsStore::new(Path::new("/tmp/x"), None, Path::new("/home/u"));
        let mut settings = store.defaults.clone();
        settings.ai_provider = "cli:BAD".into();
        assert!(store.normalize(settings).is_err());
        let settings = AppSettings {
            whisper_model: "gone".into(),
            ..store.defaults.clone()
        };
        assert_eq!(
            store.normalize(settings).unwrap().whisper_model,
            DEFAULT_WHISPER_MODEL
        );
    }
}
