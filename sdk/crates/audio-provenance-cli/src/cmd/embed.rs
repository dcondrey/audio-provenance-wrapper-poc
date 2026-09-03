use std::path::PathBuf;

use audio_provenance_audio::{BitDepth, wav};
use clap::Args;
use genomark::{Capabilities, GenoMark, Payload};

use crate::context::Context;
use crate::error::CliError;
use crate::exit;

#[derive(Debug, Args)]
pub struct EmbedArgs {
    pub file: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    /// Namespace selector, 0..=15. Namespace 0 is the published profile key.
    #[arg(long, default_value_t = 0)]
    pub namespace: u8,
    /// The 48-bit locator, 12 lowercase hex characters. A fresh random one when absent.
    #[arg(long)]
    pub locator: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputDepth::Int24)]
    pub bit_depth: OutputDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl OutputDepth {
    const fn depth(self) -> BitDepth {
        match self {
            Self::Int16 => BitDepth::Int16,
            Self::Int24 => BitDepth::Int24,
            Self::Int32 => BitDepth::Int32,
            Self::Float32 => BitDepth::Float32,
        }
    }
}

pub fn run(context: &Context, args: &EmbedArgs) -> Result<u8, CliError> {
    if args.out.exists() {
        return Err(CliError::WouldOverwrite {
            path: args.out.display().to_string(),
        });
    }
    let ingested = genotrace::ingest_path(&args.file, genotrace::IngestLimits::default())?;
    let audio = ingested.audio();

    let locator = match &args.locator {
        Some(text) => parse_locator(text)?,
        // Random, never content-derived. A locator taken from the audio would let anyone holding a
        // candidate master confirm it produced a given mark, and would collide with itself the
        // second time one master is embedded.
        None => random_locator()?,
    };
    let payload = Payload::new(genomark::payload::VERSION, args.namespace, locator)?;
    let mark = if args.namespace == 0 {
        GenoMark::public()
    } else {
        return Err(CliError::usage(
            "a non-zero namespace needs its profile key, and this build ships no key store; use --namespace=0",
        ));
    };

    let (marked, report) = mark.embed_measured(audio, payload)?;
    let encoded = wav::encode(&marked, args.bit_depth.depth())?;
    std::fs::write(&args.out, &encoded).map_err(CliError::io("write", args.out.display()))?;

    let capabilities = Capabilities::at(marked.sample_rate());
    if context.json {
        let rendered = serde_json::to_string_pretty(&serde_json::json!({
            "out": args.out.display().to_string(),
            "algorithm": capabilities.algorithm,
            "payload_bits": capabilities.payload_bits,
            "locator": hex::encode(&payload.to_bytes()[1..]),
            "namespace": payload.namespace(),
            "version": payload.version(),
            "block_seconds": capabilities.block_seconds,
            "guaranteed_seconds": capabilities.guaranteed_seconds,
            "strong_class_seconds": capabilities.strong_class_seconds,
            "measured_rate_range": capabilities.measured_rate_range,
            "acoustic_rerecording": capabilities.acoustic_rerecording,
            "time_stretch": capabilities.time_stretch,
            "measured_transparency": capabilities.measured_transparency,
            "measured_survival": capabilities.measured_survival,
            "blocks": report.blocks,
            "slots": report.slots,
            "punctured_slots": report.punctured_slots,
            "meets_closure_budget": report.meets_closure_budget(),
        }))
        .map_err(|source| CliError::Serialise {
            what: "embed report",
            source,
        })?;
        println!("{rendered}");
        return Ok(exit::VERIFIED);
    }
    if context.quiet {
        return Ok(exit::VERIFIED);
    }

    println!("embedded   {}", args.out.display());
    println!(
        "  algorithm  {:<27}payload {} bits",
        capabilities.algorithm, capabilities.payload_bits
    );
    println!(
        "  locator    {:<27}namespace {}{}",
        hex::encode(&payload.to_bytes()[1..]),
        payload.namespace(),
        if payload.namespace() == 0 {
            " (public, published profile key)"
        } else {
            ""
        }
    );
    // IMPORTANT: printed on every standalone embed. Nothing here writes a record, so this payload
    // resolves nothing until one exists under it, and `audio-provenance sign --mark` is what makes one.
    println!(
        "  resolves   nothing: a standalone embed publishes no record. `audio-provenance sign --mark"
    );
    println!(
        "             --registry=<name>` allocates a locator and registers the record it names."
    );
    println!(
        "  block      {:<27}guaranteed decode {:.2} s",
        format!("{:.2} s", capabilities.block_seconds),
        capabilities.guaranteed_seconds
    );
    println!(
        "  blocks     {:<27}{} slots, {} punctured",
        report.blocks, report.slots, report.punctured_slots
    );
    println!(
        "  survives   lossy compression (unmeasured), resample, requantise, gain, playback rate"
    );
    println!(
        "             +-{:.1}% measured (+-{:.1}% searched)",
        capabilities.measured_rate_range * 100.0,
        capabilities.searched_rate_range * 100.0
    );
    // IMPORTANT: printed on every embed, never behind -v. A user who does not see it will assume
    // the mark survives a microphone, which is the one thing it structurally cannot do.
    println!(
        "  FAILS      acoustic re-recording ({}, by design)",
        capabilities.acoustic_rerecording
    );
    println!(
        "             time-stretch ({}), pitch shift, lowpass below 5 kHz,",
        capabilities.time_stretch
    );
    println!(
        "             audio shorter than {:.2} s",
        capabilities.block_seconds
    );
    println!(
        "  transparency  {}",
        match capabilities.measured_transparency {
            Some(value) => format!("{value:.2} ODG"),
            None => "not measured".to_string(),
        }
    );
    println!(
        "  survival      {}",
        match capabilities.measured_survival {
            Some(value) => format!("{value:.3}"),
            None => "not measured".to_string(),
        }
    );
    if !report.meets_closure_budget()
        && let Some(residual) = report.closure_residual_max_nepers
    {
        println!(
            "  warning    overlap-add closure residual {residual:.4} nepers exceeds the budget; the mark may not decode"
        );
    }
    Ok(exit::VERIFIED)
}

/// 48 fresh bits. `audio-provenance sign --mark` is the command that allocates a locator a registry will
/// answer to; this one writes a payload nothing resolves.
fn random_locator() -> Result<u64, CliError> {
    let mut raw = [0u8; 8];
    if getrandom::fill(&mut raw[2..]).is_err() {
        return Err(CliError::Entropy { bytes: 6 });
    }
    Ok(u64::from_be_bytes(raw))
}

fn parse_locator(text: &str) -> Result<u64, CliError> {
    if text.len() != 12 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CliError::usage(
            "--locator must be exactly 12 hexadecimal characters (48 bits)",
        ));
    }
    u64::from_str_radix(text, 16)
        .map_err(|_| CliError::usage("--locator is not a hexadecimal number"))
}
