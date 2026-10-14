//! Direct updates of the macOS `Aven.app` bundle.
//!
//! A bundled install is replaced as a whole: the executable's signature seals
//! `Info.plist` and the resources, so changing only `Contents/MacOS/aven`
//! would break it. A bare install is migrated by installing the bundle under
//! `~/.local/share/aven` and replacing the bare executable with a symlink.

use std::collections::HashSet;
use std::ffi::CString;
use std::io::{Cursor, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use tokio::sync::watch;

use super::eligibility::can_stage_beside;
use super::install::{download_verified, set_phase, validate_staged_binary};
use super::{InstallSuccess, Release, UpdatePhase, UpdateProgress, ensure_not_cancelled};
use crate::app_bundle::{BUNDLE_EXECUTABLE, BUNDLE_NAME, direct_install_bundle, enclosing_bundle};

const ENTRY_LIMIT: usize = 4096;
const EXTRACTED_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BundleLayout {
    /// Replace the bundle that contains the running executable.
    Update { bundle: PathBuf },
    /// Install `bundle` and replace the bare executable at `link` with a
    /// symlink to the bundle's executable.
    Migrate { link: PathBuf, bundle: PathBuf },
}

impl BundleLayout {
    /// `None` keeps the bare-executable update: the release has no app
    /// archive or there is no home directory to hold the bundle.
    pub(super) fn for_target(
        target: &Path,
        release: &Release,
        home: Option<&Path>,
    ) -> Result<Option<Self>> {
        if let Some(bundle) = enclosing_bundle(target) {
            if release.app.is_none() {
                bail!("release v{} has no Aven.app archive", release.version);
            }
            return Ok(Some(Self::Update { bundle }));
        }
        match (&release.app, home) {
            (Some(_), Some(home)) => Ok(Some(Self::Migrate {
                link: target.to_path_buf(),
                bundle: direct_install_bundle(home),
            })),
            _ => Ok(None),
        }
    }

    fn bundle(&self) -> &Path {
        match self {
            Self::Update { bundle } | Self::Migrate { bundle, .. } => bundle,
        }
    }
}

pub(super) async fn install(
    client: &reqwest::Client,
    release: &Release,
    layout: BundleLayout,
    progress: &watch::Sender<UpdateProgress>,
    cancelled: &Arc<AtomicBool>,
) -> Result<InstallSuccess> {
    let app = release
        .app
        .as_ref()
        .context("release has no Aven.app archive")?;
    let archive = download_verified(
        client,
        &app.url,
        &app.checksum_url,
        &app.name,
        progress,
        cancelled,
    )
    .await?;

    ensure_not_cancelled(cancelled)?;
    set_phase(progress, UpdatePhase::Extracting);
    let entries = read_app_archive(&archive)?;

    ensure_not_cancelled(cancelled)?;
    set_phase(progress, UpdatePhase::Staging);
    let bundle = layout.bundle();
    let parent = bundle
        .parent()
        .context("Aven.app has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".aven-update-")
        .tempdir_in(parent)
        .context("stage update beside Aven.app")?;
    write_app_entries(staging.path(), &entries)?;
    let staged = staging.path().join(BUNDLE_NAME);
    validate_staged_binary(&staged.join(BUNDLE_EXECUTABLE), &release.version).await?;

    ensure_not_cancelled(cancelled)?;
    if let BundleLayout::Migrate { link, .. } = &layout
        && !can_stage_beside(link)
    {
        bail!("the install directory is not writable");
    }
    set_phase(progress, UpdatePhase::Installing);
    let executable = bundle.join(BUNDLE_EXECUTABLE);
    match &layout {
        BundleLayout::Update { bundle } => replace_bundle(&staged, bundle, || Ok(()))?,
        BundleLayout::Migrate { link, bundle } => {
            replace_bundle(&staged, bundle, || link_executable(link, &executable))?
        }
    }
    // The previous bundle now sits in the staging directory.
    if let Err(error) = staging.close() {
        tracing::debug!(%error, "failed to remove the previous Aven.app");
    }

    Ok(InstallSuccess {
        version: release.version.clone(),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum AppEntry {
    Directory(PathBuf),
    File {
        path: PathBuf,
        bytes: Vec<u8>,
        executable: bool,
    },
}

/// Reads an archive that may contain only directories and regular files
/// under `Aven.app/`, including its main executable and `Info.plist`.
fn read_app_archive(archive: &[u8]) -> Result<Vec<AppEntry>> {
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(archive)));
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut remaining = EXTRACTED_LIMIT;
    for entry in archive.entries().context("read Aven.app archive")? {
        let entry = entry.context("read Aven.app archive entry")?;
        if entries.len() == ENTRY_LIMIT {
            bail!("Aven.app archive has too many entries");
        }
        let raw = entry.path().context("read Aven.app archive path")?;
        let path = bundle_entry_path(&raw)
            .with_context(|| format!("Aven.app archive contains {}", raw.display()))?;
        if !seen.insert(path.clone()) {
            bail!("Aven.app archive repeats {}", path.display());
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            entries.push(AppEntry::Directory(path));
            continue;
        }
        if !entry_type.is_file() || path.components().count() < 2 {
            bail!(
                "Aven.app archive entry {} is not a regular file or directory",
                path.display()
            );
        }
        let size = entry.size();
        if size > remaining {
            bail!("Aven.app archive is unexpectedly large");
        }
        let executable = entry.header().mode().context("read Aven.app file mode")? & 0o111 != 0;
        let mut bytes = Vec::with_capacity(size as usize);
        entry
            .take(remaining + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("extract {}", path.display()))?;
        remaining = remaining
            .checked_sub(bytes.len() as u64)
            .context("Aven.app archive is unexpectedly large")?;
        entries.push(AppEntry::File {
            path,
            bytes,
            executable,
        });
    }

    let is_file = |wanted: &Path| {
        entries
            .iter()
            .any(|entry| matches!(entry, AppEntry::File { path, .. } if path == wanted))
    };
    let executable = Path::new(BUNDLE_NAME).join(BUNDLE_EXECUTABLE);
    if !is_file(&executable) {
        bail!("Aven.app archive does not contain {}", executable.display());
    }
    let info = Path::new(BUNDLE_NAME).join("Contents/Info.plist");
    if !is_file(&info) {
        bail!("Aven.app archive does not contain {}", info.display());
    }
    Ok(entries)
}

/// Accepts only plain relative paths whose first component is `Aven.app`.
fn bundle_entry_path(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::Normal(BUNDLE_NAME.as_ref())) {
        bail!("an entry outside {BUNDLE_NAME}/");
    }
    if !components.all(|component| matches!(component, Component::Normal(_))) {
        bail!("an entry with a non-normal path component");
    }
    Ok(path.components().collect())
}

/// Writes validated entries below `root`, which must be a new empty directory.
fn write_app_entries(root: &Path, entries: &[AppEntry]) -> Result<()> {
    let mut directories = std::fs::DirBuilder::new();
    directories.recursive(true).mode(0o755);
    for entry in entries {
        match entry {
            AppEntry::Directory(path) => {
                let path = root.join(path);
                directories
                    .create(&path)
                    .with_context(|| format!("create {}", path.display()))?;
            }
            AppEntry::File {
                path,
                bytes,
                executable,
            } => {
                let path = root.join(path);
                if let Some(parent) = path.parent() {
                    directories
                        .create(parent)
                        .with_context(|| format!("create {}", parent.display()))?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(if *executable { 0o755 } else { 0o644 })
                    .open(&path)
                    .with_context(|| format!("create {}", path.display()))?;
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .with_context(|| format!("write {}", path.display()))?;
            }
        }
    }
    Ok(())
}

/// Moves `staged` to `destination`, atomically swapping it with an existing
/// bundle, then runs `finish`. If `finish` fails, the swap is undone. After
/// success, the previous bundle (if any) is left at `staged`.
fn replace_bundle(
    staged: &Path,
    destination: &Path,
    finish: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let swapped = match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() => {
            rename_with(staged, destination, libc::RENAME_SWAP).context("swap Aven.app")?;
            true
        }
        Ok(_) => bail!("{} is not a directory", destination.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            rename_with(staged, destination, libc::RENAME_EXCL).context("install Aven.app")?;
            false
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", destination.display()));
        }
    };
    let Err(error) = finish() else {
        return Ok(());
    };
    let restored = if swapped {
        rename_with(staged, destination, libc::RENAME_SWAP)
    } else {
        rename_with(destination, staged, libc::RENAME_EXCL)
    };
    match restored {
        Ok(()) => Err(error),
        Err(restore) => Err(error.context(format!(
            "restoring {} also failed: {restore}",
            destination.display()
        ))),
    }
}

fn rename_with(from: &Path, to: &Path, flags: libc::c_uint) -> std::io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Atomically replaces `link` with a symlink to `executable`.
fn link_executable(link: &Path, executable: &Path) -> Result<()> {
    let parent = link
        .parent()
        .context("aven executable has no parent directory")?;
    let staged = tempfile::Builder::new()
        .prefix(".aven-update-")
        .make_in(parent, |path| std::os::unix::fs::symlink(executable, path))
        .context("stage aven symlink")?;
    staged
        .persist(link)
        .map_err(|error| error.error)
        .with_context(|| format!("replace {} with a symlink", link.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use flate2::{Compression, write::GzEncoder};
    use semver::Version;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::update::{InstallMethod, InstallPlan, ReleaseArchive};

    #[derive(Clone, Copy)]
    enum Kind {
        Dir,
        File,
        Symlink,
        Hardlink,
        Fifo,
    }

    fn tar_gz(entries: &[(&str, Kind, &[u8], u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (path, kind, body, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(*mode);
            match kind {
                Kind::Dir => header.set_entry_type(tar::EntryType::Directory),
                Kind::File => header.set_entry_type(tar::EntryType::Regular),
                Kind::Symlink => header.set_entry_type(tar::EntryType::Symlink),
                Kind::Hardlink => header.set_entry_type(tar::EntryType::Link),
                Kind::Fifo => header.set_entry_type(tar::EntryType::Fifo),
            }
            if matches!(kind, Kind::Symlink | Kind::Hardlink) {
                header.set_size(0);
                header
                    .set_link_name(std::str::from_utf8(body).unwrap())
                    .unwrap();
                header.set_path(path).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            } else {
                header.set_size(body.len() as u64);
                // `set_path` rejects `..`; write raw bytes to model hostile archives.
                let name = &mut header.as_old_mut().name;
                name[..path.len()].copy_from_slice(path.as_bytes());
                header.set_cksum();
                builder.append(&header, *body).unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn script(version: &str, daemon_log: &Path) -> Vec<u8> {
        format!(
            "#!/bin/sh\nif [ \"$1\" = daemon ]; then printf '%s\\n' \"$0 $*\" >> '{}'; exit 0; fi\nprintf 'aven {version}\\n'\n",
            daemon_log.display()
        )
        .into_bytes()
    }

    fn app_archive(executable: &[u8]) -> Vec<u8> {
        tar_gz(&[
            ("Aven.app/", Kind::Dir, b"", 0o755),
            ("Aven.app/Contents/", Kind::Dir, b"", 0o755),
            (
                "Aven.app/Contents/Info.plist",
                Kind::File,
                b"<plist/>",
                0o644,
            ),
            ("Aven.app/Contents/MacOS/", Kind::Dir, b"", 0o755),
            (
                "Aven.app/Contents/MacOS/aven",
                Kind::File,
                executable,
                0o755,
            ),
            ("Aven.app/Contents/Resources/", Kind::Dir, b"", 0o755),
            (
                "Aven.app/Contents/Resources/Aven.icns",
                Kind::File,
                b"icon",
                0o644,
            ),
        ])
    }

    fn write_bundle(bundle: &Path, executable: &[u8]) {
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), b"old plist").unwrap();
        let path = bundle.join(BUNDLE_EXECUTABLE);
        std::fs::write(&path, executable).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn checksum(archive: &[u8], name: &str) -> String {
        format!("{}  {name}\n", hex::encode(Sha256::digest(archive)))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    async fn loopback_install(
        archive: Vec<u8>,
        checksum: String,
        target: &Path,
        home: &Path,
        version: &str,
    ) -> Result<InstallSuccess> {
        use axum::{Router, routing::get};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = Router::new()
            .route(
                "/archive",
                get(move || {
                    let body = archive.clone();
                    async move { body }
                }),
            )
            .route(
                "/checksum",
                get(move || {
                    let body = checksum.clone();
                    async move { body }
                }),
            )
            .route("/bare", get(|| async { "unused" }));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (progress, _receiver) = watch::channel(UpdateProgress {
            phase: UpdatePhase::Downloading,
        });
        let plan = InstallPlan {
            release: Release {
                version: Version::parse(version)?,
                tag: format!("v{version}"),
                archive_name: "aven-test.tar.gz".to_string(),
                archive_url: format!("http://{address}/bare"),
                checksum_url: format!("http://{address}/bare"),
                app: Some(Box::new(ReleaseArchive {
                    name: "aven-test-app.tar.gz".to_string(),
                    url: format!("http://{address}/archive"),
                    checksum_url: format!("http://{address}/checksum"),
                })),
            },
            method: InstallMethod::Direct {
                target: target.to_path_buf(),
            },
        };
        let result = crate::update::install::install_direct_for_home(
            reqwest::Client::builder().no_proxy().build()?,
            plan,
            progress,
            Arc::new(AtomicBool::new(false)),
            Some(home.to_path_buf()),
        )
        .await;
        server.abort();
        result
    }

    fn release(app: bool) -> Release {
        Release {
            version: Version::new(1, 2, 3),
            tag: "v1.2.3".to_string(),
            archive_name: "a.tar.gz".to_string(),
            archive_url: String::new(),
            checksum_url: String::new(),
            app: app.then(|| {
                Box::new(ReleaseArchive {
                    name: "a-app.tar.gz".to_string(),
                    url: String::new(),
                    checksum_url: String::new(),
                })
            }),
        }
    }

    #[test]
    fn layout_follows_the_running_executable() {
        let home = Path::new("/Users/me");
        assert_eq!(
            BundleLayout::for_target(
                Path::new("/Users/me/.local/share/aven/Aven.app/Contents/MacOS/aven"),
                &release(true),
                Some(home),
            )
            .unwrap(),
            Some(BundleLayout::Update {
                bundle: PathBuf::from("/Users/me/.local/share/aven/Aven.app")
            })
        );
        assert_eq!(
            BundleLayout::for_target(Path::new("/usr/local/bin/aven"), &release(true), Some(home))
                .unwrap(),
            Some(BundleLayout::Migrate {
                link: PathBuf::from("/usr/local/bin/aven"),
                bundle: PathBuf::from("/Users/me/.local/share/aven/Aven.app")
            })
        );
        assert_eq!(
            BundleLayout::for_target(
                Path::new("/usr/local/bin/aven"),
                &release(false),
                Some(home)
            )
            .unwrap(),
            None,
            "releases without an app archive keep the bare update"
        );
        assert_eq!(
            BundleLayout::for_target(Path::new("/usr/local/bin/aven"), &release(true), None)
                .unwrap(),
            None
        );
        assert!(
            BundleLayout::for_target(
                Path::new("/x/Aven.app/Contents/MacOS/aven"),
                &release(false),
                Some(home),
            )
            .is_err(),
            "a bundle must never be updated from the bare archive"
        );
    }

    #[test]
    fn reads_only_files_and_directories_inside_the_bundle() {
        let entries = read_app_archive(&app_archive(b"binary")).unwrap();
        assert!(entries.contains(&AppEntry::File {
            path: PathBuf::from("Aven.app/Contents/MacOS/aven"),
            bytes: b"binary".to_vec(),
            executable: true,
        }));
        assert!(entries.contains(&AppEntry::File {
            path: PathBuf::from("Aven.app/Contents/Info.plist"),
            bytes: b"<plist/>".to_vec(),
            executable: false,
        }));
        assert!(entries.contains(&AppEntry::Directory(PathBuf::from("Aven.app/Contents"))));
    }

    fn expected_rejection(case: &str) -> &'static str {
        match case {
            "outside" => "outside Aven.app/",
            "other bundle" => "outside Aven.app/",
            "parent dir" => "non-normal path component",
            "absolute" => "outside Aven.app/",
            "current dir" => "outside Aven.app/",
            "symlink" => "not a regular file",
            "symlinked executable" => "not a regular file",
            "hardlink" => "not a regular file",
            "fifo" => "not a regular file",
            "duplicate" => "repeats",
            "missing executable" => "does not contain Aven.app/Contents/MacOS/aven",
            "missing plist" => "does not contain Aven.app/Contents/Info.plist",
            "executable directory" => "does not contain Aven.app/Contents/MacOS/aven",
            "root file" => "not a regular file",
            _ => unreachable!("{case}"),
        }
    }

    type Entry<'a> = (&'a str, Kind, &'a [u8], u32);

    #[test]
    fn rejects_adversarial_app_archives() {
        let plist = ("Aven.app/Contents/Info.plist", Kind::File, &b"p"[..], 0o644);
        let exe = ("Aven.app/Contents/MacOS/aven", Kind::File, &b"x"[..], 0o755);
        let cases: Vec<(&str, Vec<Entry>)> = vec![
            (
                "outside",
                vec![plist, exe, ("evil", Kind::File, b"x", 0o644)],
            ),
            (
                "other bundle",
                vec![plist, exe, ("Other.app/x", Kind::File, b"x", 0o644)],
            ),
            (
                "parent dir",
                vec![plist, exe, ("Aven.app/../evil", Kind::File, b"x", 0o644)],
            ),
            (
                "absolute",
                vec![plist, exe, ("/Aven.app/evil", Kind::File, b"x", 0o644)],
            ),
            (
                "current dir",
                vec![plist, exe, ("./Aven.app/evil", Kind::File, b"x", 0o644)],
            ),
            (
                "symlink",
                vec![
                    plist,
                    exe,
                    ("Aven.app/Contents/link", Kind::Symlink, b"/etc", 0o755),
                ],
            ),
            (
                "symlinked executable",
                vec![
                    plist,
                    (
                        "Aven.app/Contents/MacOS/aven",
                        Kind::Symlink,
                        b"/bin/sh",
                        0o755,
                    ),
                ],
            ),
            (
                "hardlink",
                vec![
                    plist,
                    exe,
                    (
                        "Aven.app/Contents/hard",
                        Kind::Hardlink,
                        b"/etc/passwd",
                        0o644,
                    ),
                ],
            ),
            (
                "fifo",
                vec![
                    plist,
                    exe,
                    ("Aven.app/Contents/fifo", Kind::Fifo, b"", 0o644),
                ],
            ),
            ("duplicate", vec![plist, exe, exe]),
            ("missing executable", vec![plist]),
            ("missing plist", vec![exe]),
            (
                "executable directory",
                vec![
                    plist,
                    ("Aven.app/Contents/MacOS/aven", Kind::Dir, b"", 0o755),
                ],
            ),
            (
                "root file",
                vec![plist, exe, ("Aven.app", Kind::File, b"x", 0o644)],
            ),
        ];
        for (name, entries) in cases {
            let error = read_app_archive(&tar_gz(&entries)).unwrap_err();
            assert!(
                format!("{error:#}").contains(expected_rejection(name)),
                "{name}: {error:#}"
            );
        }
        assert!(read_app_archive(b"not gzip").is_err());
    }

    #[test]
    fn accepts_a_bundle_archived_by_system_tar() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        write_bundle(&source.join(BUNDLE_NAME), b"binary");
        std::fs::create_dir_all(source.join("Aven.app/Contents/Resources")).unwrap();
        std::fs::write(source.join("Aven.app/Contents/Resources/Aven.icns"), b"i").unwrap();
        let archive = temp.path().join("app.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .arg("--no-mac-metadata")
            .arg("-C")
            .arg(&source)
            .arg("-czf")
            .arg(&archive)
            .arg(BUNDLE_NAME)
            .status()
            .unwrap();
        assert!(status.success());
        let entries = read_app_archive(&std::fs::read(&archive).unwrap()).unwrap();
        let root = temp.path().join("out");
        std::fs::create_dir(&root).unwrap();
        write_app_entries(&root, &entries).unwrap();
        let executable = root.join("Aven.app").join(BUNDLE_EXECUTABLE);
        assert_eq!(std::fs::read(&executable).unwrap(), b"binary");
        assert_eq!(
            std::fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn failed_finish_restores_the_previous_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("Aven.app");
        let staged = temp.path().join("stage/Aven.app");
        write_bundle(&destination, b"old");
        write_bundle(&staged, b"new");

        let error = replace_bundle(&staged, &destination, || bail!("link failed")).unwrap_err();

        assert_eq!(error.to_string(), "link failed");
        assert_eq!(
            std::fs::read(destination.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(staged.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"new"
        );

        let fresh = temp.path().join("fresh/Aven.app");
        std::fs::create_dir(fresh.parent().unwrap()).unwrap();
        replace_bundle(&staged, &fresh, || bail!("link failed")).unwrap_err();
        assert!(!fresh.exists());
        assert_eq!(
            std::fs::read(staged.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"new"
        );
    }

    #[test]
    fn refuses_to_replace_a_non_directory_bundle_path() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("stage/Aven.app");
        write_bundle(&staged, b"new");
        let real = temp.path().join("real.app");
        write_bundle(&real, b"real");
        let destination = temp.path().join("Aven.app");
        std::os::unix::fs::symlink(&real, &destination).unwrap();

        assert!(replace_bundle(&staged, &destination, || Ok(())).is_err());
        assert_eq!(
            std::fs::read(real.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"real"
        );
        assert!(
            std::fs::symlink_metadata(&destination)
                .unwrap()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn update_swaps_the_whole_bundle_and_leaves_the_daemon_alone() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let apps = temp.path().join("apps");
        let bundle = apps.join("Aven.app");
        write_bundle(&bundle, b"old executable");
        std::fs::write(bundle.join("Contents/stale"), b"old only").unwrap();
        let log = temp.path().join("daemon.log");
        let archive = app_archive(&script("1.2.3", &log));
        let sum = checksum(&archive, "aven-test-app.tar.gz");

        let target = bundle.join(BUNDLE_EXECUTABLE);
        let installed = loopback_install(archive, sum, &target, &home, "1.2.3")
            .await
            .unwrap();

        assert_eq!(installed.version, Version::new(1, 2, 3));
        assert_eq!(names(&apps), vec!["Aven.app"]);
        assert_eq!(
            std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap(),
            script("1.2.3", &log)
        );
        assert_eq!(
            std::fs::read(bundle.join("Contents/Resources/Aven.icns")).unwrap(),
            b"icon"
        );
        assert!(!bundle.join("Contents/stale").exists());
        assert!(
            !home.exists(),
            "an update must not create the migration path"
        );
        // An installed daemon restarts on the new executable by itself.
        assert!(!log.exists());
    }

    #[tokio::test]
    async fn rejected_updates_preserve_the_existing_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let apps = temp.path().join("apps");
        let bundle = apps.join("Aven.app");
        write_bundle(&bundle, b"old executable");
        let target = bundle.join(BUNDLE_EXECUTABLE);
        let log = temp.path().join("daemon.log");
        let good = app_archive(&script("1.2.3", &log));
        let hostile = tar_gz(&[
            ("Aven.app/Contents/Info.plist", Kind::File, b"p", 0o644),
            ("Aven.app/Contents/MacOS/aven", Kind::File, b"x", 0o755),
            ("Aven.app/../escaped", Kind::File, b"x", 0o644),
        ]);
        for (archive, sum, version, expected) in [
            (
                good.clone(),
                format!("{}  aven-test-app.tar.gz\n", "0".repeat(64)),
                "1.2.3",
                "release checksum does not match",
            ),
            (
                good.clone(),
                checksum(&good, "aven-test.tar.gz"),
                "1.2.3",
                "checksum file names an unexpected archive",
            ),
            (
                good.clone(),
                checksum(&good, "aven-test-app.tar.gz"),
                "1.2.4",
                "expected v1.2.4",
            ),
            (
                hostile.clone(),
                checksum(&hostile, "aven-test-app.tar.gz"),
                "1.2.3",
                "Aven.app archive contains",
            ),
        ] {
            let error = loopback_install(archive, sum, &target, &home, version)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(
                std::fs::read(&target).unwrap(),
                b"old executable",
                "{expected}"
            );
            assert_eq!(names(&apps), vec!["Aven.app"], "{expected}");
            assert!(!temp.path().join("escaped").exists());
            assert!(!log.exists());
        }
    }

    #[tokio::test]
    async fn swap_failure_preserves_the_existing_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let apps = temp.path().join("apps");
        let bundle = apps.join("Aven.app");
        write_bundle(&bundle, b"old executable");
        // The staged executable locks the bundle directory after validation,
        // so the swap fails like it would on a read-only volume.
        let executable = format!(
            "#!/bin/sh\nchmod 555 '{}'\nprintf 'aven 1.2.3\\n'\n",
            apps.display()
        );
        let archive = app_archive(executable.as_bytes());
        let sum = checksum(&archive, "aven-test-app.tar.gz");

        let result = loopback_install(
            archive,
            sum,
            &bundle.join(BUNDLE_EXECUTABLE),
            &temp.path().join("home"),
            "1.2.3",
        )
        .await;
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o755)).unwrap();

        let error = result.unwrap_err();
        assert!(format!("{error:#}").contains("swap Aven.app"), "{error:#}");
        assert_eq!(
            std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"old executable"
        );
        assert_eq!(
            std::fs::read(bundle.join("Contents/Info.plist")).unwrap(),
            b"old plist"
        );
    }

    #[tokio::test]
    async fn migration_installs_the_bundle_and_links_the_bare_executable() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("aven");
        std::fs::write(&target, b"bare executable").unwrap();
        let log = temp.path().join("daemon.log");
        let archive = app_archive(&script("1.2.3", &log));
        let sum = checksum(&archive, "aven-test-app.tar.gz");

        loopback_install(archive, sum, &target, &home, "1.2.3")
            .await
            .unwrap();

        let bundle = direct_install_bundle(&home);
        let executable = bundle.join(BUNDLE_EXECUTABLE);
        assert_eq!(std::fs::read_link(&target).unwrap(), executable);
        assert_eq!(names(&bin), vec!["aven"]);
        assert_eq!(names(bundle.parent().unwrap()), vec!["Aven.app"]);
        let output = std::process::Command::new(&target)
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"aven 1.2.3\n");
        assert!(!log.exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires a signed macOS baseline binary and a signed candidate app archive"]
    async fn signed_macos_app_round_trip() {
        let baseline = std::env::var_os("AVEN_MACOS_QUALIFICATION_BASELINE")
            .expect("set AVEN_MACOS_QUALIFICATION_BASELINE to the signed baseline binary");
        let archive_path = std::env::var_os("AVEN_MACOS_QUALIFICATION_APP_ARCHIVE")
            .expect("set AVEN_MACOS_QUALIFICATION_APP_ARCHIVE to the signed candidate app archive");
        let verifier = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-macos-release");
        let verified = |path: &Path| {
            std::process::Command::new(&verifier)
                .arg(path)
                .status()
                .unwrap()
                .success()
        };
        assert!(verified(Path::new(&baseline)));
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("aven");
        let baseline_bytes = std::fs::read(&baseline).unwrap();
        std::fs::write(&target, &baseline_bytes).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let archive = std::fs::read(archive_path).unwrap();
        let sum = checksum(&archive, "aven-test-app.tar.gz");

        // A bare install migrates to the bundle and keeps its PATH entry.
        loopback_install(
            archive.clone(),
            sum.clone(),
            &target,
            &home,
            crate::update::CURRENT_VERSION,
        )
        .await
        .unwrap();
        let bundle = direct_install_bundle(&home);
        let executable = bundle.join(BUNDLE_EXECUTABLE);
        assert_eq!(std::fs::read_link(&target).unwrap(), executable);
        assert_ne!(std::fs::read(&executable).unwrap(), baseline_bytes);
        assert!(verified(&bundle));

        // A bundled install is replaced as a whole and stays signed.
        loopback_install(
            archive,
            sum,
            &executable,
            &home,
            crate::update::CURRENT_VERSION,
        )
        .await
        .unwrap();
        assert!(verified(&bundle));
        assert_eq!(names(bundle.parent().unwrap()), vec!["Aven.app"]);
        assert_eq!(names(&bin), vec!["aven"]);
        let output = std::process::Command::new(&target)
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            output.stdout,
            format!("aven {}\n", crate::update::CURRENT_VERSION).into_bytes()
        );
    }

    #[tokio::test]
    async fn migration_replaces_a_leftover_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bundle = direct_install_bundle(&home);
        write_bundle(&bundle, b"leftover");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("aven");
        std::fs::write(&target, b"bare executable").unwrap();
        let log = temp.path().join("daemon.log");
        let archive = app_archive(&script("1.2.3", &log));
        let sum = checksum(&archive, "aven-test-app.tar.gz");

        loopback_install(archive, sum, &target, &home, "1.2.3")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap(),
            script("1.2.3", &log)
        );
        assert_eq!(names(bundle.parent().unwrap()), vec!["Aven.app"]);
        assert!(std::fs::symlink_metadata(&target).unwrap().is_symlink());
    }

    #[test]
    fn failed_migration_link_keeps_the_bare_executable_and_old_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = direct_install_bundle(&temp.path().join("home"));
        write_bundle(&bundle, b"leftover");
        let staged = bundle.parent().unwrap().join(".aven-update-test/Aven.app");
        write_bundle(&staged, b"new");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("aven");
        std::fs::write(&target, b"bare executable").unwrap();

        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = replace_bundle(&staged, &bundle, || {
            link_executable(&target, &bundle.join(BUNDLE_EXECUTABLE))
        });
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let error = result.unwrap_err();
        assert!(
            format!("{error:#}").contains("stage aven symlink"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"bare executable");
        assert!(!std::fs::symlink_metadata(&target).unwrap().is_symlink());
        assert_eq!(names(&bin), vec!["aven"]);
        assert_eq!(
            std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"leftover"
        );
        assert_eq!(
            std::fs::read(staged.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"new"
        );
    }
}
