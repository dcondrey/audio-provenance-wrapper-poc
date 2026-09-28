use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apw_daemon::{Daemon, DaemonConfig, SourceCategory, StopSignal};
use apw_provenance::expand_user;

use crate::adapters::{EngineConfig, Engines};
use crate::cli::DaemonArgs;
use crate::error::{CliError, Result};

static TERMINATE: AtomicBool = AtomicBool::new(false);

const SHUTDOWN_POLL: Duration = Duration::from_millis(200);

pub fn run(args: &DaemonArgs) -> Result<i32> {
    let source_category = SourceCategory::parse(&args.source_category).ok_or_else(|| {
        CliError::usage(format!(
            "unsupported source category {:?}; expected one of [{}]",
            args.source_category,
            SourceCategory::ALL
                .iter()
                .map(|category| category.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
        ))
    })?;
    warn_about_uncompiled_engines(args);

    let engines = Engines::load(&EngineConfig {
        provenance_store: &args.provenance_store,
        provenance_provider: args.provenance_provider.as_deref(),
        device_key_path: &args.signing_key,
        portable_private_key: &args.portable_private_key,
        portable_public_key: &args.portable_public_key,
    })?;

    let config = DaemonConfig {
        udp_port: args.port,
        evidence_dir: expand_user(&args.evidence_dir),
        manifest_dir: expand_user(&args.manifest_dir),
        sample_dir: expand_user(&args.sample_dir),
        export_dir: args.export_dir.as_ref().map(|path| expand_user(path)),
        session_id: args.session_id.clone(),
        stem_id: args.stem_id.clone(),
        source_category,
        generate_html_report: !args.no_html_report,
        ..DaemonConfig::default()
    };

    let daemon = Daemon::new(config, engines.daemon_services(args.time_anchor.as_deref()))?;
    log::info!("Capture session: {}", daemon.session_id());
    log::info!(
        "Declared source category: {} ({})",
        source_category.as_str(),
        source_category.proof_level().as_str()
    );
    log::info!("Evidence receiver on UDP port {}", args.port);

    install_termination_handler();
    let stop = daemon.stop_signal();
    let shutdown = thread::Builder::new()
        .name("shutdown-watch".to_owned())
        .spawn(move || watch_for_termination(&stop))
        .map_err(|source| {
            CliError::usage(format!("could not start the shutdown watcher: {source}"))
        })?;

    let outcome = daemon.run();
    daemon.stop();
    TERMINATE.store(true, Ordering::SeqCst);
    let _ = shutdown.join();
    outcome?;
    Ok(0)
}

fn watch_for_termination(stop: &Arc<StopSignal>) {
    while !stop.is_stopped() {
        if TERMINATE.load(Ordering::SeqCst) {
            log::info!("Shutting down");
            stop.stop();
            return;
        }
        if stop.wait(SHUTDOWN_POLL) {
            return;
        }
    }
}

/// SIGTERM is how `scripts/run_demo.sh` ends the session, and the daemon must
/// reach its own shutdown path: the `session_end` evidence record and the joined
/// worker threads both depend on it, and a killed process can leave a partially
/// appended JSONL line behind.
fn install_termination_handler() {
    extern "C" fn on_signal(_signum: core::ffi::c_int) {
        TERMINATE.store(true, Ordering::SeqCst);
    }

    // SAFETY: `on_signal` only stores into a `static AtomicBool`. It allocates
    // nothing, takes no lock, and calls no libc function, so it is
    // async-signal-safe. Registration happens once, before any worker thread is
    // spawned.
    for signum in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: see above; `on_signal` is async-signal-safe and registration
        // happens once, before any worker thread is spawned.
        let previous = unsafe { libc::signal(signum, on_signal as *const () as libc::sighandler_t) };
        if previous == libc::SIG_ERR {
            log::warn!(
                "Could not install the handler for signal {signum}; that signal will kill the \
                 daemon before it can write its session_end record"
            );
        }
    }
}

/// A flag this binary accepts but cannot honour must say so. An accepted option
/// that silently does nothing is the failure mode this project's charter is
/// most against.
fn warn_about_uncompiled_engines(args: &DaemonArgs) {
    if args.project.is_some() {
        log::warn!(
            "--project is accepted but no saved-project watcher is compiled into the native \
             daemon; the manifest will carry no session_facts and no project sample ingredients"
        );
    }
    if args.open_artifacts {
        log::warn!(
            "--open-artifacts is accepted but the native daemon does not render the HTML fight \
             card; there is nothing to open"
        );
    }
}
