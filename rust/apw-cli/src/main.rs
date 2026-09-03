fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(apw_cli::run(std::env::args_os()).clamp(0, 255) as u8)
}
