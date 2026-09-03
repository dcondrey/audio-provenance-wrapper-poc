mod assoc;
mod audio;
mod claim;
mod seal;

use std::path::Path;
use std::sync::Arc;

use apw_core::Ed25519Signer;
use apw_daemon::DaemonServices;
use apw_provenance::{
    detect_hardware_provider, detect_provider, expand_user, HardwareProvider, ProvenanceProvider,
};

pub use assoc::FeatureAssociator;
pub use audio::PcmAudioProbe;
pub use claim::C2paClaimIssuer;
pub use seal::ProviderSealer;

use crate::error::Result;

/// The algorithm a device provider reports when it is the filesystem-key
/// fallback. `apw-provenance` documents it as "not an attestation of any
/// device", and it is the only signal that separates a real module from the
/// software signer.
const SOFTWARE_INTEGRITY_ALGORITHM: &str = "hmac-sha256-local";

/// Where the engines this binary compiles in are read from.
pub struct EngineConfig<'a> {
    pub provenance_store: &'a Path,
    pub provenance_provider: Option<&'a str>,
    pub device_key_path: &'a Path,
    pub portable_private_key: &'a Path,
    pub portable_public_key: &'a Path,
}

/// The engines behind one provenance provider, shared by every command.
pub struct Engines {
    pub provenance: Arc<dyn ProvenanceProvider>,
    pub device: Arc<dyn HardwareProvider>,
    pub hardware_attested: bool,
    pub portable_signer: Option<Arc<Ed25519Signer>>,
}

impl Engines {
    pub fn load(config: &EngineConfig) -> Result<Engines> {
        let store = expand_user(config.provenance_store);
        let provenance: Arc<dyn ProvenanceProvider> =
            Arc::from(detect_provider(&store, config.provenance_provider)?);
        let device: Arc<dyn HardwareProvider> =
            Arc::from(detect_hardware_provider(&expand_user(config.device_key_path))?);
        let hardware_attested = attests_hardware(device.as_ref());

        // A daemon that cannot start because key custody failed is worse than a
        // manifest that honestly records no portable signature, which is what
        // the Python daemon does on the same failure.
        let portable_signer = match Ed25519Signer::load_or_create(
            &expand_user(config.portable_private_key),
            &expand_user(config.portable_public_key),
        ) {
            Ok(signer) => Some(Arc::new(signer)),
            Err(error) => {
                log::warn!("Portable Ed25519 signing is unavailable: {error}");
                None
            }
        };

        Ok(Engines {
            provenance,
            device,
            hardware_attested,
            portable_signer,
        })
    }

    pub fn daemon_services(&self) -> DaemonServices {
        DaemonServices {
            audio: Arc::new(PcmAudioProbe),
            associator: Arc::new(FeatureAssociator),
            claims: Arc::new(C2paClaimIssuer::new(Arc::clone(&self.provenance))),
            sealer: Arc::new(ProviderSealer::new(
                Arc::clone(&self.device),
                self.hardware_attested,
            )),
            portable_signer: self.portable_signer.clone(),
            // No statistical forgery screen and no RFC 3161 client are compiled
            // into this binary. Both stay at the honest "did not run" default
            // rather than emitting an empty result that would read as clean.
            ..DaemonServices::default()
        }
    }
}

/// IMPORTANT: an error answers "not attested". A provider whose identity cannot
/// be read must never be reported as hardware-backed.
fn attests_hardware(provider: &dyn HardwareProvider) -> bool {
    provider
        .device_identity()
        .map(|identity| identity.algorithm != SOFTWARE_INTEGRITY_ALGORITHM)
        .unwrap_or(false)
}
