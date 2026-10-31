use std::process::ExitCode;

use aven_desktop::{app, isolation::Isolation};

fn main() -> ExitCode {
    let isolation = match Isolation::from_process() {
        Ok(isolation) => isolation,
        Err(error) => {
            eprintln!("aven-desktop refused to start: {error:#}");
            return ExitCode::from(78);
        }
    };
    if std::env::args().any(|argument| argument == "--check-isolation") {
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
