//! Subprocess workers: ignored tests of the current test binary, re-run with
//! JSON arguments to observe state after a process exit without destructors.
use serde::{Serialize, de::DeserializeOwned};

const ARGS: &str = "AVEN_TEST_WORKER_ARGS";

/// Status of a worker that stopped itself with [`exit`].
pub const EXIT: i32 = 23;

/// Runs the ignored test `name` with `args` and asserts it stopped with [`exit`].
pub fn run(name: &str, args: &impl Serialize) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored"])
        .env(ARGS, serde_json::to_string(args).unwrap())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(EXIT),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The worker's arguments, or `None` when it runs as an ordinary ignored test.
pub fn args<T: DeserializeOwned>() -> Option<T> {
    let json = std::env::var(ARGS).ok()?;
    Some(serde_json::from_str(&json).unwrap())
}

/// Stops the worker without running destructors.
pub fn exit() -> ! {
    std::process::exit(EXIT)
}
