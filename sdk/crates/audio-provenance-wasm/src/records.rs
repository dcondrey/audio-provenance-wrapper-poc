//! The registry the JavaScript side supplies.
//!
//! `wasm32-unknown-unknown` has no filesystem and no socket, so neither shipped backend can run
//! here. What a browser or a Node process CAN do is fetch the records itself, asynchronously, and
//! hand them over; this is the backend that serves them. It is a real backend, not a stub: the same
//! `MarkMatches` non-emptiness rule, the same ambiguity reporting, the same advisory fingerprint
//! scoring the filesystem backend applies.
//!
//! It cannot reach out. A lookup for a record the caller did not supply is a genuine
//! [`Lookup::NotFound`] over the set that was supplied, never an outage, because nothing was
//! attempted and failed. Callers who need a remote registry resolve the locator first (see
//! `locators` in [`crate`]) and supply the records for it on the second call.

use core::cmp::Ordering;

use audio_provenance_registry::{
    AdvisoryScore, ContentHash, Fingerprint, Lookup, MAX_FINGERPRINT_CANDIDATES, MarkId,
    MarkMatches, RecordId, RegistryBackend, RegistryError, RegistryKind, RegistryRecord,
    RegistrySource, ScoredCandidate, UnavailableKind,
};

#[derive(Debug)]
pub struct CallerSuppliedRecords {
    source: RegistrySource,
    records: Vec<RegistryRecord>,
}

impl CallerSuppliedRecords {
    /// Parses record envelopes exactly as the filesystem backend stores them.
    ///
    /// Every envelope is validated at this boundary: the format tag, the canonical-JSON round trip,
    /// the locator salt and the mark fields. A caller cannot inject a record whose declared mark
    /// contradicts the manifest it points at.
    pub fn from_envelopes(
        name: &str,
        envelopes: &[serde_json::Value],
    ) -> Result<Self, RegistryError> {
        let mut records = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            let bytes =
                serde_json::to_vec(envelope).map_err(|error| RegistryError::MalformedRecord {
                    reason: error.to_string(),
                })?;
            records.push(RegistryRecord::from_envelope_bytes(&bytes)?);
        }
        Ok(Self {
            source: RegistrySource::new(
                name,
                RegistryKind::Memory,
                format!("{} caller-supplied record(s)", records.len()),
            ),
            records,
        })
    }
}

impl RegistryBackend for CallerSuppliedRecords {
    fn source(&self) -> &RegistrySource {
        &self.source
    }

    fn lookup_by_mark(&self, mark: &MarkId) -> Lookup<MarkMatches> {
        let matched: Vec<RegistryRecord> = self
            .records
            .iter()
            .filter(|record| record.mark_id() == *mark)
            .cloned()
            .collect();
        if matched.is_empty() {
            return Lookup::NotFound;
        }
        match MarkMatches::new(matched) {
            Ok(matches) => Lookup::Found(matches),
            Err(error) => Lookup::unavailable(UnavailableKind::IndexCorrupt, error.to_string()),
        }
    }

    fn lookup_by_content_hash(&self, content_hash: &ContentHash) -> Lookup<RegistryRecord> {
        match self
            .records
            .iter()
            .find(|record| record.content_hash() == *content_hash)
        {
            Some(record) => Lookup::Found(record.clone()),
            None => Lookup::NotFound,
        }
    }

    fn nearest_by_fingerprint(
        &self,
        query: &Fingerprint,
        limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        let limit = limit.min(MAX_FINGERPRINT_CANDIDATES);
        if limit == 0 {
            return Lookup::NotFound;
        }
        let mut scored: Vec<(f32, RecordId)> = self
            .records
            .iter()
            .filter_map(|record| {
                let stored = record.fingerprint()?;
                Some((stored.bit_agreement(query)?, record.record_id()))
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored.truncate(limit);

        let mut candidates = Vec::with_capacity(scored.len());
        for (agreement, record_id) in scored {
            match AdvisoryScore::new(agreement) {
                Ok(score) => candidates.push(ScoredCandidate::new(record_id, score)),
                Err(error) => {
                    return Lookup::unavailable(UnavailableKind::IndexCorrupt, error.to_string());
                }
            }
        }
        if candidates.is_empty() {
            return Lookup::NotFound;
        }
        Lookup::Found(candidates)
    }

    fn fetch(&self, record_id: &RecordId) -> Lookup<RegistryRecord> {
        match self
            .records
            .iter()
            .find(|record| record.record_id() == *record_id)
        {
            Some(record) => Lookup::Found(record.clone()),
            None => Lookup::NotFound,
        }
    }
}
