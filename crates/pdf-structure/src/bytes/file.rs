//! A file read by position ([`FileBytes`]), and what tells whether it's
//! the file it was ([`Identity`]).

use std::io;
use std::path::Path;
#[cfg(any(unix, windows))]
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(any(unix, windows))]
use same_file::Handle;

/// What tells a file from another, or from itself changed, without reading
/// it: the file itself, by a handle it holds open, its device and inode on
/// Unix and its volume serial number and file index on Windows; and its
/// length and modification time. It's opaque and compared whole, and a
/// clone holds the same handle.
///
/// It tells a file changed in place by a write that changes its length or
/// its modification time; not one that keeps both, as a write within the
/// file system's grain of time can, or one that sets the time back. And it
/// tells a file replaced by another, as a save by a new file renamed into
/// its place is, or one removed and another made at its path, by another
/// inode on Unix and another file index on Windows: held open, the file's
/// can't be another file's while the identity lives, as it could once the
/// file was gone. Not by its change time, which Unix sets on a file
/// replaced so as it's unlinked, though what's held open of it reads as it
/// did. On ReFS a file index of 64 bits needn't be unique, which the length
/// and time narrow. Elsewhere than Unix and Windows it's the length and
/// modification time alone, and holds nothing open. Nothing in it is read
/// again unless it's asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(any(unix, windows))]
    file: Arc<Handle>,
}

impl Identity {
    /// The identity of the file at `path`, as it is now: to tell, before
    /// writing to the path, whether it's still the file a reading opened
    /// ([`FileBytes::identity`]). On Unix and Windows it opens the file to
    /// read, without waiting, and holds it open while it lives, so it needs
    /// leave to read the file, where its length and time alone need none.
    /// A path that's neither a file nor a folder, a FIFO, a socket or a
    /// device, is refused (`InvalidInput`), never opened: opening a FIFO
    /// would wait for a writer, and a device's opening can do what the
    /// device does. One that becomes such a thing as it's opened is opened
    /// without waiting, then refused. On Windows, what it holds open still
    /// lets others write the file, rename it and remove it.
    pub fn of(path: impl AsRef<Path>) -> io::Result<Identity> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path)?;
        plain(&metadata)?;
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            let file = (std::fs::OpenOptions::new().read(true))
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            Handle::from_file(file)?
        };
        #[cfg(windows)]
        let file = Handle::from_path(path)?;
        #[cfg(any(unix, windows))]
        let identity = {
            plain(&file.as_file().metadata()?)?;
            Identity::held(Arc::new(file))?
        };
        #[cfg(not(any(unix, windows)))]
        let identity = Identity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        Ok(identity)
    }

    /// The identity of the file `file` holds open, its length and
    /// modification time as they are now.
    #[cfg(any(unix, windows))]
    fn held(file: Arc<Handle>) -> io::Result<Identity> {
        let metadata = file.as_file().metadata()?;
        Ok(Identity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            file,
        })
    }
}

/// Nothing, where `metadata` is a file's or a folder's; else an error
/// saying it's neither, and on Unix what it is.
fn plain(metadata: &std::fs::Metadata) -> io::Result<()> {
    let kind = metadata.file_type();
    if kind.is_file() || kind.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    let what = {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_fifo() {
            ": a FIFO"
        } else if kind.is_socket() {
            ": a socket"
        } else if kind.is_block_device() || kind.is_char_device() {
            ": a device"
        } else {
            ""
        }
    };
    #[cfg(not(unix))]
    let what = "";
    let message = format!("neither a file nor a folder{what}");
    Err(io::Error::new(io::ErrorKind::InvalidInput, message))
}

/// A file read by position, a window at a time from where a read starts
/// (`pread`), never whole but where the reader needs it whole: a file
/// whose cross-reference sections don't read, scanned for its objects. Its
/// length and [`Identity`] are taken as it's made, and it reads through
/// the handle its identity holds; a file that changes after is read as it
/// then is, which [`FileBytes::unchanged`] tells.
#[cfg(any(unix, windows))]
#[derive(Debug)]
pub struct FileBytes {
    len: usize,
    identity: Identity,
}

#[cfg(any(unix, windows))]
impl FileBytes {
    /// How many bytes a read at an offset reads first.
    const WINDOW: usize = 4096;

    /// `file`, as long as it is now, read by position. A file longer than
    /// the target addresses (past 4 GiB on 32 bits) isn't read.
    pub fn new(file: std::fs::File) -> io::Result<FileBytes> {
        let identity = Identity::held(Arc::new(Handle::from_file(file)?))?;
        let len = usize::try_from(identity.len).map_err(|_| {
            let message = format!(
                "the file's {} bytes are more than this target addresses",
                identity.len
            );
            io::Error::new(io::ErrorKind::FileTooLarge, message)
        })?;
        Ok(FileBytes { len, identity })
    }

    /// The file's identity as it was made. It holds the file open, and so
    /// does a clone of it kept after this is dropped.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Whether the file, the one it holds open, is as it was made: its
    /// length and modification time the same. A file changed in place
    /// isn't; one another replaced at its path still is, as reads go on
    /// reading the one it holds, which [`Identity::of`] the path tells.
    pub fn unchanged(&self) -> io::Result<bool> {
        let metadata = self.identity.file.as_file().metadata()?;
        let now = (metadata.len(), metadata.modified().ok());
        Ok(now == (self.identity.len, self.identity.modified))
    }
}

#[cfg(any(unix, windows))]
impl super::ByteSource for FileBytes {
    fn len(&self) -> usize {
        self.len
    }

    fn read_at(&self, range: std::ops::Range<usize>) -> io::Result<std::borrow::Cow<'_, [u8]>> {
        let mut bytes = vec![0; range.len()];
        let offset = u64::try_from(range.start).map_err(io::Error::other)?;
        read_exact_at(self.identity.file.as_file(), &mut bytes, offset)?;
        Ok(std::borrow::Cow::Owned(bytes))
    }

    fn first_window(&self) -> usize {
        FileBytes::WINDOW
    }
}

/// `bytes.len()` bytes of `file` from `offset`, without moving its cursor.
#[cfg(unix)]
fn read_exact_at(file: &std::fs::File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

/// `bytes.len()` bytes of `file` from `offset`: `seek_read` reads as many
/// as it can, and is asked again for the rest.
#[cfg(windows)]
fn read_exact_at(file: &std::fs::File, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                bytes = &mut bytes[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;

    use super::super::tests::{clean, root};
    use super::super::{ByteSource, Options, Reading, read};
    use super::*;
    use crate::write::{self, Xref};

    /// A file of the test's own, in a folder of its own in the temp
    /// folder, removed with the file when it's dropped.
    struct Temp(PathBuf, TempDir);

    impl Temp {
        fn new(test: &str, bytes: &[u8]) -> Temp {
            let dir = tempfile::Builder::new()
                .prefix(&format!("pdf-structure-{test}-"))
                .tempdir()
                .unwrap();
            let path = dir.path().join(format!("{test}.pdf"));
            fs::write(&path, bytes).unwrap();
            Temp(path, dir)
        }

        fn open(&self) -> FileBytes {
            FileBytes::new(File::open(&self.0).unwrap()).unwrap()
        }
    }

    /// A file read by position gives its bytes, and a reading of it, read
    /// on demand from the last object back, is the whole read.
    #[test]
    fn a_file_read_by_position_gives_its_bytes_and_reads_as_whole() {
        let bytes = write::file("1.7", &clean(), root(), Xref::Stream);
        let temp = Temp::new("bytes", &bytes);
        let file = temp.open();
        assert_eq!(file.len(), bytes.len());
        assert!(file.whole().is_none());
        let end = bytes.len();
        for range in [0..0, 0..1, 9..100, end - 5..end, 0..end] {
            assert_eq!(file.read_at(range.clone()).unwrap(), &bytes[range]);
        }
        assert!(file.read_at(end..end + 1).is_err());
        let reading = Reading::open(file, &[], &Options::DEFAULT).unwrap();
        let ids: Vec<_> = reading.ids().collect();
        assert_eq!(ids.len(), 6);
        ids.iter()
            .rev()
            .for_each(|&id| assert!(reading.get(id).is_ok()));
        assert_eq!(reading.into_document().unwrap(), read(&bytes));
    }

    /// A file changed in place after it's opened isn't unchanged, by a
    /// byte appended, which changes its length, and by bytes of the same
    /// length whose modification time is set apart, which doesn't lean on
    /// the clock's grain; its path's identity then differs. A file replaced
    /// at its path by a rename is: what's held open is the file it was,
    /// read as it was, and the path's identity is another's.
    #[test]
    fn a_file_changed_after_its_opened_is_told() {
        let temp = Temp::new("appended", b"%PDF-1.7\n");
        let file = temp.open();
        assert!(file.unchanged().unwrap());
        assert_eq!(&Identity::of(&temp.0).unwrap(), file.identity());
        let mut appending = OpenOptions::new().append(true).open(&temp.0).unwrap();
        appending.write_all(b"%").unwrap();
        assert!(!file.unchanged().unwrap());
        assert_ne!(&Identity::of(&temp.0).unwrap(), file.identity());

        let temp = Temp::new("rewritten", b"%PDF-1.7\n");
        let file = temp.open();
        let mut writing = OpenOptions::new().write(true).open(&temp.0).unwrap();
        writing.write_all(b"%PDF-2.0\n").unwrap();
        let apart = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        writing.set_modified(apart).unwrap();
        assert_eq!(fs::metadata(&temp.0).unwrap().len(), 9);
        assert!(!file.unchanged().unwrap());
        assert_ne!(&Identity::of(&temp.0).unwrap(), file.identity());

        let temp = Temp::new("replaced", b"%PDF-1.7\n");
        let file = temp.open();
        let other = temp.1.path().join("replacing.pdf");
        fs::write(&other, b"%PDF-2.0\n").unwrap();
        fs::rename(&other, &temp.0).unwrap();
        assert!(file.unchanged().unwrap());
        assert_eq!(file.read_at(0..9).unwrap(), &b"%PDF-1.7\n"[..]);
        assert_ne!(&Identity::of(&temp.0).unwrap(), file.identity());
    }

    /// An identity holds its file open: one kept from a `FileBytes` since
    /// dropped, and one of the path, aren't another file's, made at the
    /// path once the first is removed, of the same length and modification
    /// time; and each one's handle reads the first file's bytes still.
    /// Where the temp folder's file system gives a removed file's inode to
    /// the next file made, as ext4 does, only the hold tells the two apart.
    /// On tmpfs, which doesn't, they differ held or not, so the test can't
    /// show an inode reused there: what's read through the handle shows the
    /// hold.
    #[test]
    fn an_identity_holds_its_file() {
        let temp = Temp::new("held", b"%PDF-1.7\n");
        let file = temp.open();
        let kept = file.identity().clone();
        drop(file);
        let taken = Identity::of(&temp.0).unwrap();
        assert_eq!(taken, kept);
        let modified = fs::metadata(&temp.0).unwrap().modified().unwrap();
        fs::remove_file(&temp.0).unwrap();
        let mut other = File::create(&temp.0).unwrap();
        other.write_all(b"%PDF-2.0\n").unwrap();
        other.set_modified(modified).unwrap();
        drop(other);
        let now = Identity::of(&temp.0).unwrap();
        assert_eq!((now.len, now.modified), (kept.len, kept.modified));
        assert_ne!(now, kept);
        assert_ne!(now, taken);
        for identity in [&kept, &taken] {
            let mut bytes = [0; 9];
            read_exact_at(identity.file.as_file(), &mut bytes, 0).unwrap();
            assert_eq!(&bytes, b"%PDF-1.7\n");
        }
    }

    /// The identity of a folder is taken, but not of a FIFO, whose opening
    /// would wait for a writer, nor of a socket: each is refused at once,
    /// saying what it is. Each is asked on a thread of its own, so that one
    /// that waits fails the test rather than hanging it.
    #[cfg(unix)]
    #[test]
    fn an_identity_is_of_a_file_or_a_folder() {
        use std::os::unix::net::UnixListener;
        use std::process::Command;
        use std::sync::mpsc;
        use std::thread;

        let temp = Temp::new("special", b"%PDF-1.7\n");
        let folder = temp.1.path();
        assert_eq!(Identity::of(folder).unwrap(), Identity::of(folder).unwrap());
        let fifo = folder.join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let socket = folder.join("socket");
        let _listening = UnixListener::bind(&socket).unwrap();
        for (path, what) in [(fifo, "a FIFO"), (socket, "a socket")] {
            let (send, receive) = mpsc::channel();
            thread::spawn(move || {
                let got = Identity::of(&path).map_err(|e| (e.kind(), e.to_string()));
                send.send(got.map(drop)).unwrap();
            });
            let got = (receive.recv_timeout(Duration::from_secs(10)))
                .unwrap_or_else(|_| panic!("{what}: still waiting after 10 s"));
            let message = format!("neither a file nor a folder: {what}");
            assert_eq!(got, Err((io::ErrorKind::InvalidInput, message)));
        }
    }
}
