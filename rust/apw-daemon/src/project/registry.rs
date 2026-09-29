//! Project-format registry: a port of `daemon/project_formats/{registry,unsupported,__init__}.py`.
//!
//! A format is `supported` (has a parser returning a snapshot) or `unsupported`
//! (recognised by extension only and reported through an explicit event).

use std::path::Path;

use serde_json::{json, Value};

use super::safe::{Limits, Result};
use super::snapshot::ProjectSnapshot;
use super::{als, ardour, dawproject, lmms, maxpat, puredata, reaper};

pub const REAL_FILES: &str = "real_files";
pub const CONSTRUCTED_ONLY: &str = "constructed_fixtures_only";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Supported,
    Unsupported,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Supported => "supported",
            Status::Unsupported => "unsupported",
        }
    }
}

type Parser = fn(&Path, &Limits) -> Result<ProjectSnapshot>;

#[derive(Clone, Copy)]
pub struct ProjectFormat {
    pub format_id: &'static str,
    pub host: &'static str,
    pub extensions: &'static [&'static str],
    pub status: Status,
    pub reason: &'static str,
    pub validation: &'static str,
    parser: Option<Parser>,
}

impl ProjectFormat {
    pub fn supported(&self) -> bool {
        self.status == Status::Supported && self.parser.is_some()
    }

    pub fn parse(&self, path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
        match self.parser {
            Some(parser) if self.status == Status::Supported => parser(path, limits),
            _ => Err(unsupported_message(self)),
        }
    }
}

/// `str(UnsupportedProjectFormat(...))`.
pub fn unsupported_message(format: &ProjectFormat) -> String {
    format!("{} projects ({}) are not supported: {}", format.host, format.format_id, format.reason)
}

const PROPRIETARY: &str = "proprietary binary format with no public specification; no parser is provided";
const PACKAGE: &str = "package/bundle with proprietary internal project data; no parser is provided";
const UNVERIFIED: &str = "internal layout not verified against a real or documented sample; no parser is provided";
const DAWPROJECT: &str = "; the host can export open .dawproject, which is parsed";

macro_rules! unsupported {
    ($id:expr, $host:expr, [$($ext:expr),+], $reason:expr) => {
        ProjectFormat {
            format_id: $id, host: $host, extensions: &[$($ext),+], status: Status::Unsupported,
            reason: $reason, validation: "", parser: None,
        }
    };
}

macro_rules! supported {
    ($id:expr, $host:expr, [$($ext:expr),+], $reason:expr, $parser:expr, $validation:expr) => {
        ProjectFormat {
            format_id: $id, host: $host, extensions: &[$($ext),+], status: Status::Supported,
            reason: $reason, validation: $validation, parser: Some($parser),
        }
    };
}

// The unsupported reasons that concatenate constants are spelled out so the
// registry stays a plain static table; `registry_matches_the_oracle` proves each
// string equals the Python one.
pub static FORMATS: &[ProjectFormat] = &[
    supported!("ableton_als", "Ableton Live", [".als"],
        "gzip XML; tests use synthetic documents, no real Live-saved file is committed",
        als::extract_snapshot, CONSTRUCTED_ONLY),
    supported!("reaper_rpp", "REAPER", [".rpp"],
        "plain-text RPP; layout is unofficial and checked only against hand-written fixtures",
        reaper::extract_reaper_snapshot, CONSTRUCTED_ONLY),
    supported!("dawproject", "DAWproject (Bitwig, Cubase, Studio One and others export it)", [".dawproject"],
        "open zip of XML per Project.xsd; the only fixture is a Bitwig Studio 5.0 project.xml from the spec README, zipped here, not a real .dawproject file",
        dawproject::extract_dawproject_snapshot, CONSTRUCTED_ONLY),
    supported!("ardour", "Ardour", [".ardour"],
        "XML layout taken from Ardour's source; checked only against fixtures constructed from that source",
        ardour::extract_ardour_snapshot, CONSTRUCTED_ONLY),
    supported!("lmms", "LMMS", [".mmp", ".mmpz"],
        "XML or qCompress-ed XML per LMMS source; committed fixtures are constructed, one real demo was checked locally",
        lmms::extract_lmms_snapshot, CONSTRUCTED_ONLY),
    supported!("pure_data", "Pure Data", [".pd"],
        "plain-text patch per Pd source; no tempo or clips, one track per canvas and one device per object box",
        puredata::extract_pd_snapshot, REAL_FILES),
    supported!("max_patcher", "Max", [".maxpat"],
        "JSON patcher with no published spec; keys observed in one Max 7 file, fixture is constructed",
        maxpat::extract_maxpat_snapshot, CONSTRUCTED_ONLY),
    unsupported!("logic_pro", "Logic Pro", [".logicx", ".logic"], PACKAGE),
    unsupported!("cubase", "Cubase/Nuendo", [".cpr", ".npr"],
        "proprietary binary format with no public specification; no parser is provided; the host can export open .dawproject, which is parsed"),
    unsupported!("fl_studio", "FL Studio", [".flp"], PROPRIETARY),
    unsupported!("pro_tools", "Pro Tools", [".ptx", ".ptf"], PROPRIETARY),
    unsupported!("bitwig", "Bitwig Studio", [".bwproject"],
        "internal layout not verified against a real or documented sample; no parser is provided; the host can export open .dawproject, which is parsed"),
    unsupported!("studio_one", "Studio One / Fender Studio Pro", [".song"],
        "internal layout not verified against a real or documented sample; no parser is provided; the host can export open .dawproject, which is parsed"),
    unsupported!("cakewalk", "Cakewalk", [".cwp"], PROPRIETARY),
    unsupported!("garageband", "GarageBand", [".band"], PACKAGE),
    unsupported!("reason", "Reason", [".reason"], PROPRIETARY),
    unsupported!("sibelius", "Sibelius", [".sib"], PROPRIETARY),
    unsupported!("dorico", "Dorico", [".dorico"],
        "zip container whose internal XML has no public specification; no parser is provided"),
    unsupported!("finale", "Finale", [".musx", ".mus"],
        "proprietary binary format with no public specification; no parser is provided; MusicXML export is the interchange route and is not parsed here"),
    unsupported!("vegas", "VEGAS Pro", [".veg"], PROPRIETARY),
    unsupported!("acid", "ACID Pro", [".acd"], PROPRIETARY),
    unsupported!("premiere", "Premiere Pro", [".prproj"],
        "compressed XML with no public specification; no parser is provided"),
    unsupported!("audition", "Audition", [".sesx"], "XML with no published schema; no parser is provided"),
    unsupported!("resolve", "DaVinci Resolve", [".drp"], PROPRIETARY),
    unsupported!("audiomulch", "AudioMulch", [".amh"],
        "XML that its developer states is undocumented and may change without notice; no parser is provided"),
    unsupported!("reaktor", "Reaktor", [".ens", ".rkplr"], PROPRIETARY),
    unsupported!("vcv_rack", "VCV Rack", [".vcv"],
        "Rack 2 patches are a tar compressed with Zstandard; the Python standard library in use has no zstd support and no dependency is added"),
    unsupported!("renoise", "Renoise", [".xrns"],
        "zip of Song.xml, but no primary-source schema or format description was available to ground extraction"),
    unsupported!("audacity", "Audacity", [".aup3"],
        "SQLite database whose schema Audacity does not publish; no parser is provided"),
    unsupported!("tracktion_waveform", "Tracktion Waveform", [".tracktionedit"],
        "believed to be a JUCE ValueTree serialised as XML, but the layout is not verified at primary level; no parser is provided"),
    unsupported!("milkytracker", "MilkyTracker / module trackers", [".xm", ".mod"],
        "documented module formats, but no parser is implemented"),
];

const _: (&str, &str, &str) = (UNVERIFIED, DAWPROJECT, PACKAGE);

/// Look up by extension only; never reads the file.
pub fn detect_format(path: &Path) -> Option<&'static ProjectFormat> {
    let extension = path.extension()?.to_str()?;
    // Python `Path.suffix.lower()`.
    let dotted = format!(".{}", extension.to_lowercase());
    FORMATS.iter().find(|format| format.extensions.contains(&dotted.as_str()))
}

/// Parse `path` with its registered parser; an unrecognised extension keeps the
/// historical behaviour of attempting the Ableton `.als` parser.
pub fn parse_project(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    match detect_format(path) {
        Some(format) => format.parse(path, limits),
        None => als::extract_snapshot(path, limits),
    }
}

/// Evidence record stating that a watched project could not be structurally
/// observed. `timestamp_ms` and `daemon_observed_monotonic_ms` are supplied by the
/// caller.
pub fn unsupported_format_event(
    format: &ProjectFormat,
    path: &Path,
    timestamp_ms: i64,
    monotonic_ms: i64,
) -> Value {
    let suffix = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map_or_else(String::new, |extension| format!(".{}", extension.to_lowercase()));
    json!({
        "event_type": "project_format_unsupported",
        "proof_level": "unknown_unobserved",
        "project_format": format.format_id,
        "host": format.host,
        "file_extension": suffix,
        "reason": format.reason,
        "timestamp_ms": timestamp_ms,
        "daemon_observed_monotonic_ms": monotonic_ms,
    })
}
