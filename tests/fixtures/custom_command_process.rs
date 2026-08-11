use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

unsafe extern "C" {
    fn close(fd: i32) -> i32;
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("fixture mode");
    match mode.as_str() {
        "write-before-read" => {
            let bytes = vec![b'o'; 256 * 1024];
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr().write_all(&bytes).unwrap();
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
        }
        "never-read" => std::thread::sleep(Duration::from_secs(30)),
        "close-stdin" => {
            unsafe {
                close(0);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        "descendant-holds-stdout" => {
            spawn_descendant(&args.next().expect("PID file"), true);
        }
        "descendant-sleeps" => {
            spawn_descendant(&args.next().expect("PID file"), false);
            std::thread::sleep(Duration::from_secs(30));
        }
        "hold-output" => std::thread::sleep(Duration::from_secs(30)),
        "sleep" => {
            let mut millis = String::new();
            std::io::stdin().read_to_string(&mut millis).unwrap();
            std::thread::sleep(Duration::from_millis(millis.parse().unwrap()));
        }
        "copy-stdin" => {
            let path = args.next().expect("output path");
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            std::fs::write(path, input).unwrap();
        }
        "copy-stdin-null" => {
            std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
        }
        "failure-stderr" => {
            std::io::stderr()
                .write_all(b"first stderr line\nuseful stderr reason\n")
                .unwrap();
            std::process::exit(7);
        }
        "failure-stdout" => {
            std::io::stdout()
                .write_all(b"stdout fallback reason\n")
                .unwrap();
            std::process::exit(8);
        }
        "failure-empty" => std::process::exit(12),
        "failure-invalid-utf8" => {
            std::io::stderr()
                .write_all(b"invalid byte: \xff reason\n")
                .unwrap();
            std::process::exit(9);
        }
        "failure-unsafe" => {
            std::io::stderr()
                .write_all(b"\x1b[31mred\x1b[0m\x07\r\n\x1b]0;secret title\x07safe\n")
                .unwrap();
            std::process::exit(10);
        }
        "failure-large" => {
            std::io::stderr().write_all(&vec![b'x'; 64 * 1024]).unwrap();
            std::io::stderr()
                .write_all(b"\nlarge output tail reason\n")
                .unwrap();
            std::process::exit(11);
        }
        "success-large" => {
            std::io::stdout().write_all(&vec![b'o'; 64 * 1024]).unwrap();
            std::io::stderr().write_all(&vec![b'e'; 64 * 1024]).unwrap();
        }
        "record-pid-and-sleep" => {
            let path = args.next().expect("PID file");
            std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
            std::fs::write(path, std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        other => panic!("unknown fixture mode {other}"),
    }
}

fn spawn_descendant(pid_file: &str, inherit_stdout: bool) {
    let executable = std::env::current_exe().unwrap();
    let mut command = Command::new(executable);
    command
        .arg("hold-output")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if !inherit_stdout {
        command.stdout(Stdio::null());
    }
    let child = command.spawn().unwrap();
    std::fs::write(Path::new(pid_file), child.id().to_string()).unwrap();
}
