use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::ConfigError;

pub const CONFIG_FILE_NAME: &str = "audio-provenance.config.json";
pub const CONFIG_FORMAT: &str = "audio-provenance-config-v0";

/// The base URL of the `public` registry. Named in the error a caller sees when it is unset.
pub const REGISTRY_URL_ENV: &str = "AUDIO_PROVENANCE_REGISTRY_URL";
/// The root directory of the `local` registry.
pub const REGISTRY_ROOT_ENV: &str = "AUDIO_PROVENANCE_REGISTRY_ROOT";
/// An explicit configuration file path.
pub const CONFIG_PATH_ENV: &str = "AUDIO_PROVENANCE_CONFIG";

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_NAME_LEN: usize = 64;

/// Environment lookup, injected rather than read directly.
///
/// `std::env::set_var` is unsafe under edition 2024 and this workspace denies `unsafe_code`, so
/// a test could not otherwise exercise the environment tier of the resolution order. It also
/// keeps the ambient process environment out of library logic.
pub trait EnvSource: fmt::Debug {
    fn get(&self, key: &str) -> Option<String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapEnv(BTreeMap<String, String>);

impl MapEnv {
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }
}

impl EnvSource for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpEntry {
    pub url: String,
    pub timeout_ms: Option<u64>,
    pub max_response_bytes: Option<usize>,
    pub max_retries: Option<u32>,
    pub retry_backoff_ms: Option<u64>,
}

impl HttpEntry {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            timeout_ms: None,
            max_response_bytes: None,
            max_retries: None,
            retry_backoff_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryEntry {
    Local { root: PathBuf },
    Http(HttpEntry),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryConfig {
    entries: BTreeMap<String, RegistryEntry>,
}

fn validate_name(name: &str) -> Result<(), ConfigError> {
    let usable = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if usable {
        Ok(())
    } else {
        Err(ConfigError::Name { name: name.into() })
    }
}

fn required_str<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
    field: &'static str,
) -> Result<&'a str, ConfigError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::MissingField {
            name: name.into(),
            field,
        })
}

impl RegistryConfig {
    pub fn from_json_bytes(bytes: &[u8], path: &str) -> Result<Self, ConfigError> {
        let document: Value =
            serde_json::from_slice(bytes).map_err(|source| ConfigError::Json {
                path: path.into(),
                source,
            })?;
        let Some(object) = document.as_object() else {
            return Err(ConfigError::Format {
                found: "a non-object document".into(),
                expected: CONFIG_FORMAT,
            });
        };
        let format = object
            .get("config_format")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if format != CONFIG_FORMAT {
            return Err(ConfigError::Format {
                found: format.into(),
                expected: CONFIG_FORMAT,
            });
        }

        let mut config = Self::default();
        let Some(registries) = object.get("registries").and_then(Value::as_object) else {
            return Ok(config);
        };
        for (name, value) in registries {
            validate_name(name)?;
            let Some(fields) = value.as_object() else {
                return Err(ConfigError::Kind {
                    name: name.clone(),
                    found: "a non-object entry".into(),
                });
            };
            let kind = required_str(fields, name, "kind")?;
            let entry = match kind {
                "local" => RegistryEntry::Local {
                    root: PathBuf::from(required_str(fields, name, "root")?),
                },
                "http" => RegistryEntry::Http(HttpEntry {
                    url: required_str(fields, name, "url")?.into(),
                    timeout_ms: fields.get("timeout_ms").and_then(Value::as_u64),
                    max_response_bytes: fields
                        .get("max_response_bytes")
                        .and_then(Value::as_u64)
                        .and_then(|value| usize::try_from(value).ok()),
                    max_retries: fields
                        .get("max_retries")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok()),
                    retry_backoff_ms: fields.get("retry_backoff_ms").and_then(Value::as_u64),
                }),
                other => {
                    return Err(ConfigError::Kind {
                        name: name.clone(),
                        found: other.into(),
                    });
                }
            };
            config.entries.insert(name.clone(), entry);
        }
        Ok(config)
    }

    pub fn insert(
        &mut self,
        name: impl Into<String>,
        entry: RegistryEntry,
    ) -> Result<(), ConfigError> {
        let name = name.into();
        validate_name(&name)?;
        self.entries.insert(name, entry);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }
}

/// Where a configuration file is looked for, most specific first.
///
/// 1. the explicit path a caller passed
/// 2. `$AUDIO_PROVENANCE_CONFIG`
/// 3. `./audio-provenance.config.json`
/// 4. `$XDG_CONFIG_HOME/audio-provenance/audio-provenance.config.json`, else
///    `$HOME/.config/audio-provenance/audio-provenance.config.json`
pub fn config_candidates(explicit: Option<&Path>, env: &dyn EnvSource) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }
    if let Some(path) = env.get(CONFIG_PATH_ENV) {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(CONFIG_FILE_NAME));
    }
    let config_home = env.get("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| {
        env.get("HOME")
            .map(|home| PathBuf::from(home).join(".config"))
    });
    if let Some(home) = config_home {
        candidates.push(home.join("audio-provenance").join(CONFIG_FILE_NAME));
    }
    candidates
}

/// Load the first configuration file that exists, or an empty configuration.
///
/// The returned path is the file that was read, or the first candidate that would have been, so
/// an error can tell a caller exactly which file to create.
pub fn load_config(
    explicit: Option<&Path>,
    env: &dyn EnvSource,
) -> Result<(RegistryConfig, PathBuf), ConfigError> {
    let candidates = config_candidates(explicit, env);
    for candidate in &candidates {
        if !candidate.is_file() {
            continue;
        }
        let metadata = std::fs::metadata(candidate).map_err(|source| ConfigError::Io {
            path: candidate.display().to_string(),
            source,
        })?;
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::Format {
                found: format!("a {} byte file", metadata.len()),
                expected: CONFIG_FORMAT,
            });
        }
        let bytes = std::fs::read(candidate).map_err(|source| ConfigError::Io {
            path: candidate.display().to_string(),
            source,
        })?;
        let config = RegistryConfig::from_json_bytes(&bytes, &candidate.display().to_string())?;
        return Ok((config, candidate.clone()));
    }
    let fallback = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(CONFIG_FILE_NAME));
    Ok((RegistryConfig::default(), fallback))
}
