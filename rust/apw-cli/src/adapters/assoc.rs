use std::path::Path;

use apw_daemon::ExportAssociator;
use serde_json::Value;

/// The routed/export comparison seam, backed by the bounded offset search.
///
/// IMPORTANT: a match established here is an inference from four features, never
/// an observation of the export's own production, and a failed comparison is
/// never evidence that the routed audio is absent from the export. Both facts
/// are carried in the record's own `limitations`.
pub struct FeatureAssociator;

impl ExportAssociator for FeatureAssociator {
    fn associate(&self, export_path: &Path, routed_features: &[Value]) -> Value {
        apw_assoc::associate_export(export_path, routed_features).to_value()
    }
}
