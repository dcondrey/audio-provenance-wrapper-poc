use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::backend::RegistryBackend;
use crate::config::{
    EnvSource, REGISTRY_ROOT_ENV, REGISTRY_URL_ENV, RegistryConfig, RegistryEntry,
};
use crate::error::{ConfigError, ResolveError};
use crate::local::LocalRegistryBackend;

/// The name `audio-provenance verify --registry=public` uses.
pub const PUBLIC_REGISTRY: &str = "public";
/// The name `audio-provenance verify --registry=local` uses.
pub const LOCAL_REGISTRY: &str = "local";
/// Where `local` looks when neither the environment nor the config file says otherwise.
pub const DEFAULT_LOCAL_ROOT: &str = ".audio-provenance/registry";

/// Maps a CLI-facing registry name onto a backend.
///
/// Resolution order, per tier: an explicitly supplied entry, then the environment, then the
/// configuration file. A name that resolves to nothing is an error naming what to set; it is
/// never a guessed endpoint.
#[derive(Debug)]
pub struct RegistryResolver<'env> {
    config: RegistryConfig,
    config_path: PathBuf,
    env: &'env dyn EnvSource,
}

impl<'env> RegistryResolver<'env> {
    pub fn load(
        explicit_config: Option<&Path>,
        env: &'env dyn EnvSource,
    ) -> Result<Self, ConfigError> {
        let (config, config_path) = crate::config::load_config(explicit_config, env)?;
        Ok(Self {
            config,
            config_path,
            env,
        })
    }

    pub fn new(config: RegistryConfig, config_path: PathBuf, env: &'env dyn EnvSource) -> Self {
        Self {
            config,
            config_path,
            env,
        }
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Build a backend from an entry the caller supplied outright: the first tier of the
    /// resolution order.
    pub fn resolve_entry(
        &self,
        name: &str,
        entry: &RegistryEntry,
    ) -> Result<Arc<dyn RegistryBackend>, ResolveError> {
        match entry {
            RegistryEntry::Local { root } => Ok(Arc::new(LocalRegistryBackend::open(name, root)?)),
            #[cfg(feature = "http")]
            RegistryEntry::Http(http) => {
                let mut options = crate::http::HttpRegistryOptions::default();
                if let Some(timeout_ms) = http.timeout_ms {
                    options.timeout = std::time::Duration::from_millis(timeout_ms);
                }
                if let Some(max_response_bytes) = http.max_response_bytes {
                    options.max_response_bytes = max_response_bytes;
                }
                if let Some(max_retries) = http.max_retries {
                    options.max_retries = max_retries;
                }
                if let Some(backoff_ms) = http.retry_backoff_ms {
                    options.retry_backoff = std::time::Duration::from_millis(backoff_ms);
                }
                Ok(Arc::new(crate::http::HttpRegistryBackend::new(
                    name, &http.url, options,
                )?))
            }
            #[cfg(not(feature = "http"))]
            RegistryEntry::Http(_) => Err(ResolveError::HttpUnsupported { name: name.into() }),
        }
    }

    pub fn resolve(&self, name: &str) -> Result<Arc<dyn RegistryBackend>, ResolveError> {
        if let Some(entry) = self.entry_for(name)? {
            return self.resolve_entry(name, &entry);
        }
        match name {
            PUBLIC_REGISTRY => Err(ResolveError::Unconfigured {
                name: name.into(),
                env_var: REGISTRY_URL_ENV,
                value_hint: "<base URL>",
                config_path: self.config_path.display().to_string(),
            }),
            LOCAL_REGISTRY => Err(ResolveError::Unconfigured {
                name: name.into(),
                env_var: REGISTRY_ROOT_ENV,
                value_hint: "<directory>",
                config_path: self.config_path.display().to_string(),
            }),
            _ => Err(ResolveError::Unknown {
                name: name.into(),
                known: self.config.names().join(", "),
            }),
        }
    }

    fn entry_for(&self, name: &str) -> Result<Option<RegistryEntry>, ResolveError> {
        if name == PUBLIC_REGISTRY
            && let Some(url) = self.env.get(REGISTRY_URL_ENV)
        {
            return Ok(Some(RegistryEntry::Http(crate::config::HttpEntry::new(
                url,
            ))));
        }
        if name == LOCAL_REGISTRY
            && let Some(root) = self.env.get(REGISTRY_ROOT_ENV)
        {
            return Ok(Some(RegistryEntry::Local {
                root: PathBuf::from(root),
            }));
        }
        if let Some(entry) = self.config.get(name) {
            return Ok(Some(entry.clone()));
        }
        if name == LOCAL_REGISTRY
            && let Some(home) = self.env.get("HOME")
        {
            let root = PathBuf::from(home).join(DEFAULT_LOCAL_ROOT);
            if root.is_dir() {
                return Ok(Some(RegistryEntry::Local { root }));
            }
        }
        Ok(None)
    }
}
