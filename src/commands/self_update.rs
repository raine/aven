use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Result;

use crate::cli::SelfUpdateArgs;
use crate::update::{self, CheckOutcome, Release};

pub(crate) async fn run(args: SelfUpdateArgs) -> Result<()> {
    let client = update::client()?;
    println!("Checking for aven updates...");
    let (release, cached) = match update::check_for_update(&client, true).await {
        Ok(CheckOutcome::Current { version, cached }) => {
            println!(
                "aven v{} is current (latest release v{version}){}",
                update::CURRENT_VERSION,
                cached_suffix(cached)
            );
            return Ok(());
        }
        Ok(CheckOutcome::Available { release, cached }) => (release, cached),
        Err(error) => match update::cached_update() {
            Some(release) => (release, true),
            None => return Err(error),
        },
    };

    install_release(client, release, cached, args.yes).await
}

async fn install_release(
    client: reqwest::Client,
    release: Release,
    cached: bool,
    yes: bool,
) -> Result<()> {
    let plan = update::install_plan(release.clone());
    if let Some(lines) = plan.guidance() {
        for line in lines {
            println!("{line}");
        }
        if cached {
            println!("Release information came from the local cache.");
        }
        return Ok(());
    }

    let target = plan
        .direct_target()
        .expect("direct update plan must have a target");
    if !yes {
        println!(
            "aven v{} can update to v{} at {}{}",
            update::CURRENT_VERSION,
            release.version,
            target.display(),
            cached_suffix(cached)
        );
        println!("Run `aven update --yes` to install it.");
        return Ok(());
    }

    let (progress_tx, mut progress_rx) = update::progress_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let install = update::install_direct(client, plan, progress_tx, cancelled);
    tokio::pin!(install);
    let mut phase = progress_rx.borrow().phase;
    println!("{}...", phase.label());

    let success = loop {
        tokio::select! {
            result = &mut install => break result?,
            changed = progress_rx.changed() => {
                if changed.is_err() {
                    continue;
                }
                let next = progress_rx.borrow().phase;
                if next != phase {
                    phase = next;
                    println!("{}...", phase.label());
                }
            }
        }
    };

    println!("Installed aven v{}.", success.version);
    println!("Restart any running aven processes to use the update.");
    Ok(())
}

fn cached_suffix(cached: bool) -> &'static str {
    if cached {
        " using cached release information"
    } else {
        ""
    }
}
