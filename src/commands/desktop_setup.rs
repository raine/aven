//! `aven internal desktop-setup`: the Desktop app's setup, run from a terminal
//! by a local install of the app. Running it is the consent the app asks for.
use anyhow::Result;

#[cfg(target_os = "macos")]
pub(crate) fn run() -> Result<()> {
    use anyhow::bail;
    use aven_host::desktop_install::{
        self, Blocked, CASK_RELINK, FORMULA_UNINSTALL, Layout, MacSystem, Misplaced, Outcome, Plan,
    };

    let mut layout = Layout::current()?;
    layout.path = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect());
    let system = MacSystem;
    let survey = desktop_install::survey(&layout, &system)?;
    let report = match desktop_install::plan(&layout, &survey) {
        Plan::Ready => desktop_install::complete(&layout, &system)?,
        Plan::Switch(switch) => match desktop_install::switch(&layout, &system, &switch)? {
            Outcome::Done(report) => report,
            Outcome::Waiting(running) => bail!(
                "error terminal-aven-running processes=\"{}\" hint=\"quit them, then run this again\"",
                running
                    .iter()
                    .map(|process| format!("{} (pid {})", process.command, process.pid))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Outcome::AwaitFormula { cask, .. } => {
                let relink = if cask {
                    format!(", then {CASK_RELINK}")
                } else {
                    String::new()
                };
                bail!(
                    "error formula-installed run=\"{FORMULA_UNINSTALL}{relink}\", then run this again"
                );
            }
        },
        Plan::MoveToApplications {
            misplaced: Misplaced::Translocated { bundle },
            ..
        } => bail!(
            "error translocated app={} hint=\"macOS runs it from a temporary location; move it \
             out of Applications and back in with Finder, then open it\"",
            bundle.display()
        ),
        Plan::MoveToApplications { running, .. } => bail!(
            "error not-in-applications running={} expected={}",
            running.display(),
            layout.executable().display()
        ),
        Plan::Blocked(Blocked::UnknownAven { paths }) => bail!(
            "error unknown-aven paths={}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        Plan::Blocked(Blocked::UnknownAgent { plist, program }) => bail!(
            "error unknown-daemon-program plist={} program={}",
            plist.display(),
            program.map_or_else(|| "unreadable".to_string(), |p| p.display().to_string())
        ),
        Plan::Blocked(Blocked::Unwritable { paths }) => bail!(
            "error unwritable paths={}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        Plan::Blocked(Blocked::UnknownState { file, error }) => {
            bail!(
                "error unreadable-setup-state file={} {error}",
                file.display()
            )
        }
    };
    let command = report
        .command
        .as_ref()
        .map_or_else(|| "none".to_string(), |path| path.display().to_string());
    let daemon = match report.agent {
        None => "none",
        Some(agent) if agent.running => "running",
        Some(agent) if agent.disabled => "disabled",
        Some(_) => "stopped",
    };
    println!(
        "desktop-setup switched={} command={command} linked={} daemon={daemon}",
        if report.switched { "yes" } else { "no" },
        if report.linked { "yes" } else { "no" },
    );
    if let Some(dir) = &report.add_to_path {
        println!("warning not-on-path dir={}", dir.display());
    }
    for path in &report.shadowed_by {
        println!("warning shadowed-by path={}", path.display());
    }
    if report.cask_unlinked {
        println!("warning cask-unlinked run=\"{CASK_RELINK}\"");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run() -> Result<()> {
    anyhow::bail!(
        "error unsupported-platform command=internal-desktop-setup platform={}",
        std::env::consts::OS
    )
}
