//! Running `aven` processes of one installation, found by the executable they
//! run. Updates and the switch to Desktop wait for them rather than stopping
//! them: an `aven` that is already running keeps its old code, and writing to
//! a database a newer version has migrated is unsafe.

use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const COMMAND_LIMIT: usize = 80;

/// A process running an installation's executable, described for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    /// `Aven` for the app, otherwise the command line with `aven` first.
    pub command: String,
    pub kind: ProcessKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    /// The app, or a launch with nothing to do outside a terminal.
    App,
    /// The background daemon, which exits by itself when its executable is
    /// replaced and is then started again by launchd.
    Daemon,
    /// A command, such as the TUI or `aven mcp`. A launch with no arguments
    /// in a terminal is the TUI.
    Command,
}

/// Processes other than this one that run `executable`, ordered by pid.
pub fn running_processes(executable: &Path) -> Vec<RunningProcess> {
    let Ok(executable) = executable.canonicalize() else {
        return Vec::new();
    };
    let own = std::process::id();
    let mut found = all_pids()
        .into_iter()
        .filter_map(|pid| u32::try_from(pid).ok().map(|id| (pid, id)))
        .filter(|(_, id)| *id != own)
        .filter(|(pid, _)| executable_of(*pid).as_deref() == Some(executable.as_path()))
        .filter_map(|(pid, id)| {
            let (kind, command) = describe(&arguments_of(pid)?, has_terminal(pid));
            Some(RunningProcess {
                pid: id,
                command,
                kind,
            })
        })
        .collect::<Vec<_>>();
    found.sort_by_key(|process| process.pid);
    found
}

/// Processes other than this one and the daemon that run `executable`: the
/// ones a replacement of the installation has to wait for.
pub fn blocking_processes(executable: &Path) -> Vec<RunningProcess> {
    let mut found = running_processes(executable);
    found.retain(|process| process.kind != ProcessKind::Daemon);
    found
}

/// What a process of the executable is, from its arguments and whether it has
/// a terminal: none is the app or an idle launch, or the TUI in a terminal,
/// `daemon` alone is the daemon, and anything else is a command, shown as
/// typed.
fn describe(arguments: &[String], has_terminal: bool) -> (ProcessKind, String) {
    let rest = arguments
        .get(1..)
        .unwrap_or_default()
        .iter()
        .filter(|argument| !argument.starts_with("-psn_"))
        .collect::<Vec<_>>();
    if rest.is_empty() && !has_terminal {
        return (ProcessKind::App, "Aven".to_string());
    }
    let mut positional = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "--db" | "--workspace") {
            words.next();
        } else if !word.starts_with('-') {
            positional.push(word.as_str());
        }
    }
    let kind = if positional == ["daemon"] {
        ProcessKind::Daemon
    } else {
        ProcessKind::Command
    };
    let mut command = std::iter::once("aven")
        .chain(rest.iter().map(|word| word.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    if command.chars().count() > COMMAND_LIMIT {
        command = command.chars().take(COMMAND_LIMIT - 1).collect();
        command.push('…');
    }
    (kind, command)
}

fn all_pids() -> Vec<libc::pid_t> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Leave room for processes started between the two calls.
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if filled <= 0 {
        return Vec::new();
    }
    pids.truncate(filled as usize);
    pids.retain(|pid| *pid > 0);
    pids
}

/// Whether the process has a controlling terminal, as `aven` decides between
/// the TUI and the app.
fn has_terminal(pid: libc::pid_t) -> bool {
    const PROC_FLAG_CONTROLT: u32 = 0x2;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    filled == size && unsafe { info.assume_init() }.pbi_flags & PROC_FLAG_CONTROLT != 0
}

fn executable_of(pid: libc::pid_t) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(OsStr::from_bytes(&buffer)))
}

/// A process's arguments, from `KERN_PROCARGS2`: the argument count, the
/// executable path and its padding, then the arguments.
fn arguments_of(pid: libc::pid_t) -> Option<Vec<String>> {
    let mut size: libc::size_t = 0;
    let mut name = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let probed = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if probed != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    parse_procargs(&buffer)
}

fn parse_procargs(buffer: &[u8]) -> Option<Vec<String>> {
    let count = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    let mut rest = &buffer[4..];
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    rest = &rest[path_end..];
    let start = rest.iter().position(|byte| *byte != 0)?;
    rest = &rest[start..];
    let mut arguments = Vec::new();
    for _ in 0..count {
        let argument = CStr::from_bytes_until_nul(rest).ok()?;
        arguments.push(argument.to_string_lossy().into_owned());
        rest = &rest[argument.to_bytes().len() + 1..];
    }
    Some(arguments)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_daemon_and_the_app_are_told_apart_from_commands() {
        let exe = "/Applications/Aven.app/Contents/MacOS/aven";
        let kind = |line: &str| describe(&words(line), false).0;
        assert_eq!(kind(exe), ProcessKind::App);
        assert_eq!(kind(&format!("{exe} -psn_0_12345")), ProcessKind::App);
        assert_eq!(
            kind(&format!("{exe} --db /Users/me/db.sqlite daemon")),
            ProcessKind::Daemon
        );
        assert_eq!(
            describe(&words("aven daemon status"), true),
            (ProcessKind::Command, "aven daemon status".to_string())
        );
        assert_eq!(
            describe(&words("/usr/local/bin/aven --db daemon tui"), true),
            (ProcessKind::Command, "aven --db daemon tui".to_string()),
            "a flag's value is not a subcommand"
        );
        assert_eq!(
            describe(&words(exe), true),
            (ProcessKind::Command, "aven".to_string()),
            "a bare launch in a terminal is the TUI"
        );
        let (_, long) = describe(&words(&format!("aven add {}", "x".repeat(200))), true);
        assert_eq!(long.chars().count(), COMMAND_LIMIT);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn procargs_yield_the_arguments_after_the_executable_path() {
        let mut buffer = 3i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/Applications/Aven.app/Contents/MacOS/aven\0\0\0\0");
        buffer.extend_from_slice(b"aven\0tui\0--all\0HOME=/Users/me\0");
        assert_eq!(parse_procargs(&buffer), Some(words("aven tui --all")));
        assert_eq!(parse_procargs(&buffer[..2]), None);
    }

    #[test]
    fn finds_a_running_copy_of_an_executable() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("aven");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        let mut child = std::process::Command::new(&executable)
            .arg("30")
            .spawn()
            .unwrap();
        let found = (0..50)
            .find_map(|_| {
                let found = blocking_processes(&executable);
                if found.is_empty() {
                    std::thread::sleep(Duration::from_millis(20));
                    None
                } else {
                    Some(found)
                }
            })
            .unwrap_or_default();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            found,
            vec![RunningProcess {
                pid: child.id(),
                command: "aven 30".to_string(),
                kind: ProcessKind::Command,
            }]
        );
    }
}
