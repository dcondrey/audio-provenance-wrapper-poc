use crate::error::PortError;
use std::fmt::Debug;

pub const DEFAULT_OUTPUT_CAP: usize = 512 << 20;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Every process launch in this crate goes through this port, so the DSP and reporting layers stay
/// callable from a target that has no process API at all.
pub trait CommandRunner: Debug + Send + Sync {
    fn run(&self, program: &str, args: &[String], stdin: &[u8])
    -> Result<CommandOutput, PortError>;

    fn is_available(&self, program: &str) -> bool;
}

/// Every filesystem touch in this crate goes through this port, for the same reason.
pub trait FileStore: Debug + Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>, PortError>;
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), PortError>;
    fn list_files(&self, dir: &str, extension: &str) -> Result<Vec<String>, PortError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableRunner;

impl CommandRunner for UnavailableRunner {
    fn run(
        &self,
        program: &str,
        _args: &[String],
        _stdin: &[u8],
    ) -> Result<CommandOutput, PortError> {
        Err(PortError::Unavailable {
            program: program.to_owned(),
        })
    }

    fn is_available(&self, _program: &str) -> bool {
        false
    }
}

#[cfg(feature = "native")]
pub mod native {
    use super::{CommandOutput, CommandRunner, DEFAULT_OUTPUT_CAP, FileStore};
    use crate::error::PortError;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::process::{Command, Stdio};

    #[derive(Debug, Clone)]
    pub struct ProcessRunner {
        output_cap: usize,
    }

    impl Default for ProcessRunner {
        fn default() -> Self {
            Self {
                output_cap: DEFAULT_OUTPUT_CAP,
            }
        }
    }

    impl ProcessRunner {
        pub const fn new(output_cap: usize) -> Self {
            Self { output_cap }
        }
    }

    impl CommandRunner for ProcessRunner {
        fn run(
            &self,
            program: &str,
            args: &[String],
            stdin: &[u8],
        ) -> Result<CommandOutput, PortError> {
            let mut child = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|source| PortError::Spawn {
                    program: program.to_owned(),
                    source,
                })?;

            // IMPORTANT: the encoded payloads here run to megabytes, far past a pipe buffer. Writing
            // stdin to completion before reading stdout deadlocks, so the write runs on its own
            // thread while this one drains stdout.
            let writer = child.stdin.take().map(|mut sink| {
                let payload = stdin.to_vec();
                std::thread::spawn(move || sink.write_all(&payload))
            });

            let mut stdout = Vec::new();
            let mut stderr = String::new();
            let read_result = child.stdout.take().map_or(Ok(0), |mut source| {
                source
                    .by_ref()
                    .take(self.output_cap as u64 + 1)
                    .read_to_end(&mut stdout)
            });
            if let Some(mut source) = child.stderr.take() {
                let mut raw = Vec::new();
                let _ = source.by_ref().take(64 << 10).read_to_end(&mut raw);
                stderr = String::from_utf8_lossy(&raw).trim().to_owned();
            }
            if let Some(handle) = writer {
                // A codec that stops reading early (a truncated stream) makes the write fail with
                // EPIPE. That is the child's story to tell through its own status, not an error here.
                let _ = handle.join();
            }
            let status = child.wait().map_err(|source| PortError::Spawn {
                program: program.to_owned(),
                source,
            })?;
            read_result.map_err(|source| PortError::Io {
                path: format!("{program}:stdout"),
                source,
            })?;
            if stdout.len() > self.output_cap {
                return Err(PortError::OutputTooLarge {
                    program: program.to_owned(),
                    bytes: stdout.len(),
                    limit: self.output_cap,
                });
            }
            Ok(CommandOutput {
                status: status.code().unwrap_or(-1),
                stdout,
                stderr,
            })
        }

        fn is_available(&self, program: &str) -> bool {
            Command::new(program)
                .arg("-version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    pub struct DiskFileStore;

    impl FileStore for DiskFileStore {
        fn read(&self, path: &str) -> Result<Vec<u8>, PortError> {
            std::fs::read(path).map_err(|source| PortError::Io {
                path: path.to_owned(),
                source,
            })
        }

        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), PortError> {
            if let Some(parent) = Path::new(path).parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|source| PortError::Io {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
            std::fs::write(path, bytes).map_err(|source| PortError::Io {
                path: path.to_owned(),
                source,
            })
        }

        fn list_files(&self, dir: &str, extension: &str) -> Result<Vec<String>, PortError> {
            let entries = std::fs::read_dir(dir).map_err(|source| PortError::Io {
                path: dir.to_owned(),
                source,
            })?;
            let mut out = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|source| PortError::Io {
                    path: dir.to_owned(),
                    source,
                })?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == extension) {
                    out.push(path.display().to_string());
                }
            }
            out.sort();
            Ok(out)
        }
    }
}
