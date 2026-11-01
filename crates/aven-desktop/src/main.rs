//! The development launcher's guard: path listings and isolation checks for
//! scripts. The app itself always starts through `aven`.
#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;

    use aven_desktop::isolation::Isolation;

    let has = |flag: &str| std::env::args().any(|argument| argument == flag);
    let validated = if has("--check-isolation") {
        Isolation::from_process()
    } else {
        Isolation::paths_from_process()
    };
    let isolation = match validated {
        Ok(isolation) => isolation,
        Err(error) => {
            eprintln!("aven-desktop refused to start: {error:#}");
            return ExitCode::from(78);
        }
    };
    if has("--print-paths") {
        print!("{}", isolation.path_listing());
    }
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn main() {}
