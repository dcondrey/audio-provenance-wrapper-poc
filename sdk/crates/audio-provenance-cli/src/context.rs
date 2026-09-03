//! Global options, resolved once.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use audio_provenance_registry::{
    EnvSource, LocalRegistryBackend, ProcessEnv, REGISTRY_ROOT_ENV, RegistryBackend, RegistryEntry,
    RegistryResolver,
};
use audio_provenance_trust::{AnchoredTrustStore, Instant};
use genotrace::{FileTrustStore, NullTestTable};

use crate::error::CliError;
use crate::style::Style;

/// Filenames `--trust-store=<dir>` probes for, in order. CLI_SPEC types the flag as a directory
/// while the SDK loads one JSON file, so a directory is accepted and the file it resolved to is
/// reported rather than guessed at silently.
const TRUST_STORE_FILES: [&str; 2] = ["trust-store.json", "audio-provenance-trust-store.json"];

/// The `anchors/` subdirectory that marks a directory-form `audio-provenance-trust-store-v1` store.
const TRUST_STORE_ANCHOR_DIR: &str = "anchors";

/// A loaded store and the path it actually came from, so a resolved directory is reported rather
/// than guessed at.
type ConfiguredTrustStore = (Box<dyn genotrace::TrustStore>, PathBuf);

#[derive(Debug)]
pub struct Context {
    pub json: bool,
    pub quiet: bool,
    pub style: Style,
    pub config: Option<PathBuf>,
    pub registry: Option<String>,
    pub trust_store: Option<PathBuf>,
    pub null_test: Option<PathBuf>,
    pub offline: bool,
}

impl Context {
    pub fn resolver(&self) -> Result<RegistryResolver<'static>, CliError> {
        Ok(RegistryResolver::load(self.config.as_deref(), &ProcessEnv)?)
    }

    /// The named backend, or `None` when no `--registry` was given. An unresolvable name is a usage
    /// error whose message already says which variable or config key to set.
    pub fn registry(&self) -> Result<Option<Arc<dyn RegistryBackend>>, CliError> {
        let Some(name) = &self.registry else {
            return Ok(None);
        };
        Ok(Some(self.resolver()?.resolve(name)?))
    }

    /// A backend that can be written to. Only the filesystem backend can: an HTTP registry has no
    /// publish protocol in this build, and pretending otherwise would fail after the manifest was
    /// already signed.
    pub fn writable_registry(&self, name: &str) -> Result<LocalRegistryBackend, CliError> {
        let resolver = self.resolver()?;
        if name == audio_provenance_registry::LOCAL_REGISTRY
            && let Some(root) = ProcessEnv.get(REGISTRY_ROOT_ENV)
        {
            return Ok(LocalRegistryBackend::open(name, Path::new(&root))?);
        }
        match resolver.config().get(name) {
            Some(RegistryEntry::Local { root }) => Ok(LocalRegistryBackend::open(name, root)?),
            Some(RegistryEntry::Http(entry)) => Err(CliError::usage(format!(
                "registry {name:?} is the HTTP endpoint {}; this build publishes to filesystem registries only",
                entry.url
            ))),
            // Surfaces the resolver's own refusal, whose message already names the variable or
            // config key to set. A name that resolves for reading but not for writing is an HTTP
            // entry the env promoted, which the arm above already answered.
            None => Err(match resolver.resolve(name) {
                Err(error) => CliError::from(error),
                Ok(backend) => CliError::usage(format!(
                    "registry {name:?} resolves to a {} backend, which this build cannot write to",
                    backend.source().kind().as_str()
                )),
            }),
        }
    }

    /// The configured anchors, as whichever store format the path actually holds.
    ///
    /// `audio-provenance-trust-store-v1` is the chained format: anchors, signer records and revocation
    /// lists, every one Ed25519-signed. The older flat `audio-provenance-trust-store-v0` map of signer id
    /// to name is still read, because a store on disk keeps working across this change; it asserts
    /// the binding rather than proving it, and `audio-provenance trust` only ever writes v1.
    pub fn trust_store(&self) -> Result<Option<ConfiguredTrustStore>, CliError> {
        let Some(path) = &self.trust_store else {
            return Ok(None);
        };
        if path.is_dir() && path.join(TRUST_STORE_ANCHOR_DIR).is_dir() {
            let store = audio_provenance_trust::TrustStore::load(path)?;
            return Ok(Some((
                Box::new(AnchoredTrustStore::new(store, now()?)),
                path.clone(),
            )));
        }
        let file = if path.is_dir() {
            TRUST_STORE_FILES
                .iter()
                .map(|name| path.join(name))
                .find(|candidate| candidate.is_file())
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "no trust store in {}: expected {TRUST_STORE_ANCHOR_DIR}/ or one of {}",
                        path.display(),
                        TRUST_STORE_FILES.join(", ")
                    ))
                })?
        } else {
            path.clone()
        };
        // IMPORTANT: the size bound is applied HERE, not inside the loader. Peeking at `format` to
        // choose a loader means the bytes are read before either loader's own limit can see them,
        // so without this an over-large store is fully resident before anything refuses it.
        let length = std::fs::metadata(&file)
            .map_err(CliError::io("read", file.display()))?
            .len();
        if length > audio_provenance_trust::store::MAX_STORE_BYTES {
            return Err(CliError::Trust(
                audio_provenance_trust::TrustError::StoreTooLarge {
                    path: file.display().to_string(),
                    limit: audio_provenance_trust::store::MAX_STORE_BYTES,
                    found: length,
                },
            ));
        }
        let bytes = std::fs::read(&file).map_err(CliError::io("read", file.display()))?;
        let declared = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("format")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        if declared.as_deref() == Some(genotrace::trust::TRUST_STORE_FORMAT) {
            return Ok(Some((Box::new(FileTrustStore::from_json(&bytes)?), file)));
        }
        let store = audio_provenance_trust::TrustStore::from_json(&bytes)?;
        Ok(Some((
            Box::new(AnchoredTrustStore::new(store, now()?)),
            file,
        )))
    }

    /// The bench null test. Without one no soft binding can reach `verified`, which is why the flag
    /// exists at all: the rate is measured, never assumed.
    pub fn null_test(&self) -> Result<Option<NullTestTable>, CliError> {
        let Some(path) = &self.null_test else {
            return Ok(None);
        };
        let bytes = std::fs::read(path).map_err(CliError::io("read", path.display()))?;
        Ok(Some(NullTestTable::from_bench_report(&bytes)?))
    }
}

/// The single clock reading a run evaluates trust against.
///
/// IMPORTANT: `audio-provenance-trust` reads no clock of its own, so this is the only place a chain's
/// validity windows and revocations get their "now". One reading per run means every signer in one
/// verification is judged against the same instant.
pub fn now() -> Result<Instant, CliError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CliError::usage("system clock is before 1970"))?
        .as_secs();
    let seconds =
        i64::try_from(seconds).map_err(|_| CliError::usage("system clock is unreadable"))?;
    Ok(Instant::from_unix_seconds(seconds)?)
}
