//! The wake channel: a Unix datagram socket per database, through which the
//! user's other Aven processes ask the database's host for a round.
//!
//! The socket sits beside the database as `<db>.host.sock`, next to the host
//! lock, so it is as private as the database's directory: another user can
//! neither reach it nor take its name, and every database has its own. Where
//! that path is too long for a socket address, or its directory can't hold a
//! socket, it lives in the user's own temporary directory instead, named after
//! the database. Only the holder of the host lock binds it, so a socket file a
//! crashed host left behind is the new holder's to replace.
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram as StdUnixDatagram;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::net::UnixDatagram;
use tracing::{debug, warn};

/// A wake after a local change.
pub const CHANGED: &[u8] = b"1";
/// A wake that also bypasses the host's failure backoff.
pub const RETRY: &[u8] = b"r";

/// Where the host of the database at `db` may listen, in the order a host
/// tries them and a waker sends to them.
pub fn paths(db: &Path) -> Vec<PathBuf> {
    let mut paths = vec![crate::paths::beside_db(db, ".host.sock")];
    if let Some(dir) = user_temp_dir() {
        paths.push(dir.join(format!("aven-{}.sock", database_key(db))));
    }
    paths.retain(|path| fits_socket_address(path));
    paths
}

/// Sends `payload` to the host of the database at `db`. Returns whether a
/// host listens. A host whose queue is full has wakes waiting already, so a
/// wake it can't take counts as delivered.
pub fn send(db: &Path, payload: &[u8]) -> bool {
    let socket = match StdUnixDatagram::unbound().and_then(|socket| {
        socket.set_nonblocking(true)?;
        Ok(socket)
    }) {
        Ok(socket) => socket,
        Err(error) => {
            warn!(%error, "could not open a wake socket");
            return false;
        }
    };
    for path in paths(db) {
        match socket.send_to(payload, &path) {
            Ok(_) => {
                debug!(path = %path.display(), "host wake sent");
                return true;
            }
            Err(error) if queue_full(&error) => {
                debug!(path = %path.display(), "host wake queue full");
                return true;
            }
            Err(error) => debug!(path = %path.display(), %error, "no host at wake path"),
        }
    }
    false
}

fn queue_full(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(libc::ENOBUFS)
}

/// The host's end of the wake channel. Without a socket it never receives,
/// and the host runs rounds on its interval only.
#[derive(Debug)]
pub struct Listener {
    db: PathBuf,
    bound: Option<Bound>,
}

#[derive(Debug)]
struct Bound {
    socket: UnixDatagram,
    path: PathBuf,
    /// The socket file's device and inode, to tell it from a replacement.
    file: (u64, u64),
}

impl Listener {
    /// Listens at the first of [`paths`] that takes a socket. Only the holder
    /// of the database's host lock may call this.
    pub(crate) fn bind(db: &Path) -> Self {
        let mut listener = Self {
            db: db.to_path_buf(),
            bound: None,
        };
        listener.bound = listener.bind_first();
        if listener.bound.is_none() {
            warn!(
                db = %db.display(),
                "no wake socket could be bound; local changes sync on the interval"
            );
        }
        listener
    }

    fn bind_first(&self) -> Option<Bound> {
        paths(&self.db)
            .into_iter()
            .find_map(|path| match bind_at(&path) {
                Ok(bound) => Some(bound),
                Err(error) => {
                    debug!(path = %path.display(), %error, "could not bind the wake socket");
                    None
                }
            })
    }

    /// The socket's path, when there is one.
    pub fn path(&self) -> Option<&Path> {
        self.bound.as_ref().map(|bound| bound.path.as_path())
    }

    /// Waits for a wake and returns its length.
    pub(crate) async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        match &self.bound {
            Some(bound) => bound.socket.recv(buf).await,
            None => std::future::pending().await,
        }
    }

    /// Takes a wake that has arrived, if one has. Reads the socket itself,
    /// so it sees every queued wake whether or not the runtime has been told
    /// the socket is readable yet.
    pub(crate) fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(bound) = &self.bound else {
            return Err(io::ErrorKind::WouldBlock.into());
        };
        // SAFETY: the socket stays open for the call, and `buf` is writable
        // for its whole length.
        let len = unsafe {
            libc::recv(
                bound.socket.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::MSG_DONTWAIT,
            )
        };
        usize::try_from(len).map_err(|_| io::Error::last_os_error())
    }

    /// Binds again when the socket file is gone or was replaced, as when a
    /// cleanup of the temporary directory removed it, or binds for the first
    /// time when no path took a socket before.
    pub(crate) fn refresh(&mut self) {
        let current = self.bound.as_ref().is_some_and(|bound| {
            std::fs::symlink_metadata(&bound.path)
                .is_ok_and(|meta| (meta.dev(), meta.ino()) == bound.file)
        });
        if current {
            return;
        }
        // Dropping a `Bound` closes its socket and leaves the file, which is
        // gone or no longer this socket's.
        if let Some(bound) = self.bound.take() {
            warn!(path = %bound.path.display(), "wake socket file went away; binding again");
        }
        self.bound = self.bind_first();
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(bound) = &self.bound
            && std::fs::symlink_metadata(&bound.path)
                .is_ok_and(|meta| (meta.dev(), meta.ino()) == bound.file)
        {
            let _ = std::fs::remove_file(&bound.path);
        }
    }
}

fn bind_at(path: &Path) -> io::Result<Bound> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a file that is not a socket has the wake socket's name",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let socket = UnixDatagram::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let meta = std::fs::symlink_metadata(path)?;
    Ok(Bound {
        socket,
        path: path.to_path_buf(),
        file: (meta.dev(), meta.ino()),
    })
}

/// Whether `path` fits in a Unix socket address with its terminating NUL.
fn fits_socket_address(path: &Path) -> bool {
    // SAFETY: sockaddr_un is plain data, for which all zeroes is valid.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    path.as_os_str().len() < address.sun_path.len()
}

/// Names the database at `db` the same way from every path to its directory.
fn database_key(db: &Path) -> String {
    let resolved = match (db.parent(), db.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map_or_else(|_| db.to_path_buf(), |parent| parent.join(name)),
        _ => db.to_path_buf(),
    };
    let digest = Sha256::digest(resolved.as_os_str().as_encoded_bytes());
    hex::encode(&digest[..8])
}

/// A directory only this user can use, with a short path.
#[cfg(target_os = "macos")]
fn user_temp_dir() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;

    let mut buf = vec![0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: the buffer is writable for its whole length.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), buf.len()) };
    if len == 0 || len > buf.len() {
        return None;
    }
    // SAFETY: confstr wrote a NUL-terminated string within the buffer.
    let dir = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(OsStr::from_bytes(dir.to_bytes())))
}

/// A directory only this user can use, with a short path.
#[cfg(not(target_os = "macos"))]
fn user_temp_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recv_now(listener: &Listener) -> Option<Vec<u8>> {
        let mut buf = [0; 16];
        listener
            .try_recv(&mut buf)
            .ok()
            .map(|len| buf[..len].to_vec())
    }

    async fn recv_soon(listener: &Listener) -> Vec<u8> {
        let mut buf = [0; 16];
        let len = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv(&mut buf))
            .await
            .expect("a wake arrives")
            .unwrap();
        buf[..len].to_vec()
    }

    #[test]
    fn the_socket_sits_beside_the_database_lock() {
        assert_eq!(
            paths(Path::new("/data/aven.sqlite"))[0],
            Path::new("/data/aven.sqlite.host.sock")
        );
    }

    #[test]
    fn a_database_too_deep_for_a_socket_address_uses_the_user_temp_dir() {
        let deep = PathBuf::from("/").join("d".repeat(200)).join("aven.sqlite");
        let paths = paths(&deep);
        assert!(paths.iter().all(|path| fits_socket_address(path)));
        if let Some(dir) = user_temp_dir().filter(|dir| fits_socket_address(&dir.join("x"))) {
            assert_eq!(paths.len(), 1);
            assert!(paths[0].starts_with(dir), "{paths:?}");
        }
    }

    #[tokio::test]
    async fn a_wake_reaches_the_host_of_its_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        assert!(!send(&db, CHANGED), "nothing listens yet");
        let listener = Listener::bind(&db);
        let path = listener.path().unwrap().to_path_buf();
        assert_eq!(path, dir.path().join("db.sqlite.host.sock"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        assert!(send(&db, RETRY));
        assert_eq!(recv_soon(&listener).await, RETRY);
        drop(listener);
        assert!(!path.exists(), "the host removes its socket");
        assert!(!send(&db, CHANGED));
    }

    #[tokio::test]
    async fn a_socket_left_by_a_crashed_host_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let stale = paths(&db)[0].clone();
        drop(StdUnixDatagram::bind(&stale).unwrap());
        assert!(stale.exists());

        let listener = Listener::bind(&db);
        assert_eq!(listener.path(), Some(stale.as_path()));
        assert!(send(&db, CHANGED));
        assert_eq!(recv_soon(&listener).await, CHANGED);
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_socket_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let beside = paths(&db)[0].clone();
        std::fs::write(&beside, "keep").unwrap();

        let listener = Listener::bind(&db);
        assert_ne!(listener.path(), Some(beside.as_path()));
        assert_eq!(std::fs::read_to_string(&beside).unwrap(), "keep");
    }

    #[tokio::test]
    async fn a_removed_socket_file_is_bound_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        let mut listener = Listener::bind(&db);
        let path = listener.path().unwrap().to_path_buf();
        std::fs::remove_file(&path).unwrap();
        assert!(!send(&db, CHANGED));

        listener.refresh();
        assert_eq!(listener.path(), Some(path.as_path()));
        assert!(send(&db, CHANGED));
        assert_eq!(recv_soon(&listener).await, CHANGED);
        listener.refresh();
        assert!(recv_now(&listener).is_none(), "an intact socket stays");
    }
}
