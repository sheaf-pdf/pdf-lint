//! A file read by position ([`FileBytes`]), and what tells whether it's
//! the file it was ([`Identity`]).

use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// What tells a file from another, or from itself changed, without reading
/// it: its length and modification time; on Unix its device and inode; on
/// Windows its creation time. Taken by one `stat` of the file, it's opaque
/// and compared whole.
///
/// It tells a file changed in place by a write that changes its length or
/// its modification time; not one that keeps both, as a write within the
/// file system's grain of time can, or one that sets the time back. And it
/// tells a file replaced by another, as a save by a new file renamed into
/// its place is, by another inode on Unix. Not by its change time, which
/// Unix sets on a file replaced so as it's unlinked, though what's held
/// open of it reads as it did. On Windows, std gives no file ID, and NTFS
/// gives a file made where one was deleted, within 15 seconds, the old
/// one's creation time, so a file replaced so, of the same length and
/// modification time, isn't told there. Nothing in it is read again unless
/// it's asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    unix: (u64, u64),
    #[cfg(windows)]
    created: u64,
}

impl Identity {
    /// The identity of the file at `path`, as it is now: to tell, before
    /// writing to the path, whether it's still the file a reading opened
    /// ([`FileBytes::identity`]).
    pub fn of(path: impl AsRef<Path>) -> io::Result<Identity> {
        std::fs::metadata(path).map(|metadata| Identity::given(&metadata))
    }

    /// The identity `metadata` gives.
    fn given(metadata: &Metadata) -> Identity {
        #[cfg(unix)]
        let unix = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(windows)]
        let created = std::os::windows::fs::MetadataExt::creation_time(metadata);
        Identity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            unix,
            #[cfg(windows)]
            created,
        }
    }
}

/// A file read by position, a window at a time from where a read starts
/// (`pread`), never whole but where the reader needs it whole: a file
/// whose cross-reference sections don't read, scanned for its objects. Its
/// length and [`Identity`] are taken as it's made, by one `stat` of the
/// file; a file that changes after is read as it then is, which
/// [`FileBytes::unchanged`] tells.
#[cfg(any(unix, windows))]
#[derive(Debug)]
pub struct FileBytes {
    file: std::fs::File,
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
        let metadata = file.metadata()?;
        let len = usize::try_from(metadata.len()).map_err(|_| {
            let message = format!(
                "the file's {} bytes are more than this target addresses",
                metadata.len()
            );
            io::Error::new(io::ErrorKind::FileTooLarge, message)
        })?;
        Ok(FileBytes {
            file,
            len,
            identity: Identity::given(&metadata),
        })
    }

    /// The file's identity as it was made.
    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// Whether the file, the one it holds open, is as it was made: its
    /// identity the same. A file changed in place isn't; one another
    /// replaced at its path still is, as reads go on reading the one it
    /// holds, which [`Identity::of`] the path tells.
    pub fn unchanged(&self) -> io::Result<bool> {
        Ok(Identity::given(&self.file.metadata()?) == self.identity)
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
        read_exact_at(&self.file, &mut bytes, offset)?;
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

    use super::super::tests::{clean, root};
    use super::super::{ByteSource, Options, Reading, read};
    use super::*;
    use crate::write::{self, Xref};

    /// A file of the test's own in the temp folder, removed when it's
    /// dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(test: &str, bytes: &[u8]) -> Temp {
            let name = format!("pdf-structure-{}-{test}.pdf", std::process::id());
            let path = std::env::temp_dir().join(name);
            fs::write(&path, bytes).unwrap();
            Temp(path)
        }

        fn open(&self) -> FileBytes {
            FileBytes::new(File::open(&self.0).unwrap()).unwrap()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
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
        assert_eq!(Identity::of(&temp.0).unwrap(), file.identity());
        let mut appending = OpenOptions::new().append(true).open(&temp.0).unwrap();
        appending.write_all(b"%").unwrap();
        assert!(!file.unchanged().unwrap());
        assert_ne!(Identity::of(&temp.0).unwrap(), file.identity());

        let temp = Temp::new("rewritten", b"%PDF-1.7\n");
        let file = temp.open();
        let mut writing = OpenOptions::new().write(true).open(&temp.0).unwrap();
        writing.write_all(b"%PDF-2.0\n").unwrap();
        let apart = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        writing.set_modified(apart).unwrap();
        assert_eq!(fs::metadata(&temp.0).unwrap().len(), 9);
        assert!(!file.unchanged().unwrap());
        assert_ne!(Identity::of(&temp.0).unwrap(), file.identity());

        let temp = Temp::new("replaced", b"%PDF-1.7\n");
        let file = temp.open();
        let other = Temp::new("replacing", b"%PDF-2.0\n");
        fs::rename(&other.0, &temp.0).unwrap();
        assert!(file.unchanged().unwrap());
        assert_eq!(file.read_at(0..9).unwrap(), &b"%PDF-1.7\n"[..]);
        assert_ne!(Identity::of(&temp.0).unwrap(), file.identity());
    }
}
