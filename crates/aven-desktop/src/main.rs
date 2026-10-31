use std::process::ExitCode;

use aven_desktop::{app, isolation::Isolation};

fn main() -> ExitCode {
    let paths_only = std::env::args().any(|argument| argument == "--check-paths");
    let validated = if paths_only {
        Isolation::paths_from_process()
    } else {
        Isolation::from_process()
    };
    let isolation = match validated {
        Ok(isolation) => isolation,
        Err(error) => {
            eprintln!("aven-desktop refused to start: {error:#}");
            return ExitCode::from(78);
        }
    };
    if paths_only || std::env::args().any(|argument| argument == "--check-isolation") {
        return ExitCode::SUCCESS;
    }
    match app::run(isolation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aven-desktop failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}
