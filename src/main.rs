use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match aven::run_cli().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            aven::report_cli_error(&error);
            ExitCode::FAILURE
        }
    }
}
