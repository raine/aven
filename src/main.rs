use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "macos")]
    if let Some(status) = aven::run_bundle_launch() {
        return status;
    }
    #[cfg(all(target_os = "linux", feature = "desktop"))]
    if let Some(status) = aven::run_desktop_launch() {
        return status;
    }
    run_command()
}

#[tokio::main]
async fn run_command() -> ExitCode {
    match aven::run_cli().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            aven::report_cli_error(&error);
            ExitCode::FAILURE
        }
    }
}
