// ============================================================
//  net/scope.rs  —  Which files on disk a page may make us read
// ============================================================
//
//  A page's subresources — `<script src>`, `<link rel=stylesheet>`,
//  `<img src>` — are loaded on its behalf, so the page decides which URLs get
//  loaded. For `http:` and in-memory URLs that is ordinary web behaviour. For
//  `file:` it would let any page point the engine at any file on the disk, so
//  every subresource goes through one boundary, [`SubresourceLoader`], which
//  knows who is asking ([`Requester`]) and applies one rule:
//
//    • a page that is not itself a `file:` page never reads the disk;
//    • a `file:` page reads only beneath its *local root*: the directory the
//      session was opened at, when the page lies inside it, and otherwise the
//      page's own directory.
//
//  The rule is checked twice. First on the URL, which catches a page naming a
//  file outside its root. Then by [`read_within`] on the file actually opened,
//  which catches every way a path inside the root can lead out of it — a
//  symbolic link, a junction, an escaped `..` — including one swapped in while
//  the check runs. Top-level navigation does not go through here: it is the
//  user or a link choosing a page, not a page choosing what to read.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use super::fetch::Origin;
use super::{FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url};

/// The page a subresource is loaded for.
#[derive(Debug, Clone, PartialEq)]
pub struct Requester {
    document: Url,
    /// The `file:` directory, with a trailing slash, this page may read
    /// beneath; `None` for a page that may not read the disk at all.
    local_root: Option<Url>,
}

impl Requester {
    /// The page at `document`, in a session opened at `site`.
    ///
    /// `site` is the URL the session started from. A `file:` page inside that
    /// site's directory shares it as its root, so a page in `pages/` can use
    /// the site's `../css/`; any other `file:` page is confined to its own
    /// directory.
    pub fn new(document: &Url, site: Option<&Url>) -> Requester {
        let local_root = (document.scheme() == "file")
            .then(|| {
                let site_root = site
                    .filter(|site| site.scheme() == "file")
                    .and_then(|site| site.join(".").ok())
                    .filter(|root| Origin::of(root).can_fetch(document));
                site_root.or_else(|| document.join(".").ok())
            })
            .flatten();
        Requester {
            document: document.clone(),
            local_root,
        }
    }

    pub fn document(&self) -> &Url {
        &self.document
    }

    /// The directory this page may read beneath, if it may read the disk.
    pub fn local_root(&self) -> Option<&Url> {
        self.local_root.as_ref()
    }

    /// May this page load `target` as a subresource?
    ///
    /// Only `file:` targets are restricted. This is the check on the URL's
    /// spelling; [`SubresourceLoader`] checks the opened file as well.
    pub fn authorize(&self, target: &Url) -> Result<(), LoadError> {
        if target.scheme() != "file" {
            return Ok(());
        }
        match &self.local_root {
            // The root ends in `/`, so its origin is the root directory itself.
            Some(root) if Origin::of(root).can_fetch(target) => Ok(()),
            Some(root) => Err(LoadError::Blocked(format!(
                "{target} is outside this page's local root {root}"
            ))),
            None => Err(LoadError::Blocked(format!(
                "a {} page may not load local file {target}",
                self.document.scheme()
            ))),
        }
    }
}

/// A loader that loads on behalf of one page, and only what it may load.
///
/// Everything a page pulls in at load time or later goes through this, so the
/// rule lives in one place rather than at each element that names a URL.
pub struct SubresourceLoader<'a> {
    loader: &'a dyn ResourceLoader,
    requester: &'a Requester,
}

impl<'a> SubresourceLoader<'a> {
    pub fn new(loader: &'a dyn ResourceLoader, requester: &'a Requester) -> Self {
        SubresourceLoader { loader, requester }
    }
}

impl ResourceLoader for SubresourceLoader<'_> {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        self.requester.authorize(url)?;
        match self.requester.local_root() {
            Some(root) if url.scheme() == "file" => self.loader.load_within(url, root),
            _ => self.loader.load(url),
        }
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, crate::net::FetchError> {
        super::static_fetch(|url| self.load(url), request)
    }
}

// ── Reading beneath a root ────────────────────────────────────────────────────

/// Why [`read_within`] did not return the file.
#[derive(Debug)]
pub enum ReadError {
    /// The file opened is not, on disk, beneath the root.
    Outside,
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        ReadError::Io(error)
    }
}

/// Read the file at `path`, provided the file opened lives beneath `root`.
///
/// The check is made on the open handle, not on the path beforehand. The file
/// is opened first; then the path is resolved, the resolved path must be
/// beneath the resolved root, and it must still name *the same file* as the
/// handle (same device and inode, or volume and file ID on Windows). Only then
/// is the handle read. A link swapped before the open leads to a file outside
/// the root; one swapped after it no longer names the opened file. Either
/// way, nothing is read.
///
/// What this does not do: stop the open itself from following a link out of
/// the root. Opening a regular file has no effect beyond the read we then
/// decline. Opening a FIFO can block, as it always could here. A hard link
/// inside the root *is* inside the root: paths cannot tell it apart.
///
/// A missing file is `NotFound` when its deepest existing ancestor is beneath
/// the root, so a broken in-site reference is still a 404.
pub fn read_within(path: &Path, root: &Path) -> Result<Vec<u8>, ReadError> {
    let root = root.canonicalize()?;
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(if deepest_existing_ancestor_within(path, &root) {
                ReadError::Io(error)
            } else {
                ReadError::Outside
            });
        }
        Err(error) => return Err(error.into()),
    };
    let opened = identity::of_file(&file)?;
    let resolved = path.canonicalize()?;
    if !resolved.starts_with(&root) || identity::of_path(&resolved)? != opened {
        return Err(ReadError::Outside);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn deepest_existing_ancestor_within(path: &Path, root: &Path) -> bool {
    for ancestor in path.ancestors().skip(1) {
        match ancestor.canonicalize() {
            Ok(resolved) => return resolved.starts_with(root),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return false,
        }
    }
    false
}

/// Which file an open handle or a path refers to, as the OS identifies it.
mod identity {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    #[cfg(unix)]
    pub type Id = (u64, u64);

    #[cfg(unix)]
    pub fn of_file(file: &File) -> io::Result<Id> {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    #[cfg(unix)]
    pub fn of_path(path: &Path) -> io::Result<Id> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        Ok((metadata.dev(), metadata.ino()))
    }

    /// Volume serial number and 128-bit file ID.
    ///
    /// Stable Rust exposes neither for a handle, so this asks the OS directly
    /// with `GetFileInformationByHandleEx(FileIdInfo)` (Windows 8 / Server
    /// 2012 and later). The 128-bit ID is unique on ReFS as well as NTFS,
    /// unlike the older 64-bit file index.
    #[cfg(windows)]
    pub type Id = (u64, [u8; 16]);

    #[cfg(windows)]
    pub fn of_file(file: &File) -> io::Result<Id> {
        use std::ffi::c_void;
        use std::os::windows::io::AsRawHandle;

        #[repr(C)]
        struct FileIdInfo {
            volume_serial_number: u64,
            file_id: [u8; 16],
        }
        /// `FILE_INFO_BY_HANDLE_CLASS::FileIdInfo`.
        const FILE_ID_INFO: i32 = 18;

        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandleEx(
                file: *mut c_void,
                class: i32,
                info: *mut c_void,
                size: u32,
            ) -> i32;
        }

        let mut info = FileIdInfo {
            volume_serial_number: 0,
            file_id: [0; 16],
        };
        // SAFETY: the handle is open for the lifetime of `file`, and `info` is
        // a correctly sized, writable FILE_ID_INFO.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ID_INFO,
                (&mut info as *mut FileIdInfo).cast(),
                std::mem::size_of::<FileIdInfo>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((info.volume_serial_number, info.file_id))
    }

    #[cfg(windows)]
    pub fn of_path(path: &Path) -> io::Result<Id> {
        of_file(&File::open(path)?)
    }

    /// Anywhere else the engine cannot confirm which file it opened, so it
    /// refuses rather than guess.
    #[cfg(not(any(unix, windows)))]
    pub type Id = ();

    #[cfg(not(any(unix, windows)))]
    pub fn of_file(_: &File) -> io::Result<Id> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cannot confirm which file was opened on this platform",
        ))
    }

    #[cfg(not(any(unix, windows)))]
    pub fn of_path(_: &Path) -> io::Result<Id> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cannot confirm which file was opened on this platform",
        ))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_file_page_in_the_site_shares_the_site_root() {
        let site = url("file:///s/site/index.html");
        let nested = Requester::new(&url("file:///s/site/pages/a.html"), Some(&site));
        assert_eq!(nested.local_root(), Some(&url("file:///s/site/")));
        assert!(nested.authorize(&url("file:///s/site/css/a.css")).is_ok());
        assert!(nested.authorize(&url("file:///s/other/a.css")).is_err());
    }

    #[test]
    fn a_file_page_outside_the_site_keeps_its_own_directory() {
        let site = url("file:///s/site/index.html");
        let other = Requester::new(&url("file:///s/other/a.html"), Some(&site));
        assert_eq!(other.local_root(), Some(&url("file:///s/other/")));
        assert!(other.authorize(&url("file:///s/site/a.css")).is_err());

        let alone = Requester::new(&url("file:///s/site/pages/a.html"), None);
        assert_eq!(alone.local_root(), Some(&url("file:///s/site/pages/")));
    }

    #[test]
    fn a_page_that_is_not_local_never_reads_the_disk_but_loads_anything_else() {
        for document in ["http://a.test/x.html", "demo:///index.html"] {
            let requester = Requester::new(&url(document), Some(&url(document)));
            assert_eq!(requester.local_root(), None);
            assert!(matches!(
                requester.authorize(&url("file:///etc/hosts")),
                Err(LoadError::Blocked(_))
            ));
            assert!(requester.authorize(&url("http://cdn.test/a.js")).is_ok());
            assert!(requester.authorize(&url("demo:///a.css")).is_ok());
        }
    }

    #[test]
    fn a_non_file_site_gives_a_file_page_only_its_own_directory() {
        let requester = Requester::new(&url("file:///s/site/a.html"), Some(&url("http://a.test/")));
        assert_eq!(requester.local_root(), Some(&url("file:///s/site/")));
    }

    /// `root/inside/` is the root; `root/outside/` is not.
    struct Dirs {
        root: PathBuf,
    }

    impl Dirs {
        fn new(name: &str) -> Dirs {
            let root = std::env::temp_dir().join(format!(
                "browser-engine-scope-{}-{name}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("inside")).unwrap();
            fs::create_dir_all(root.join("outside")).unwrap();
            fs::write(root.join("inside/a.txt"), "synthetic inside").unwrap();
            fs::write(root.join("outside/b.txt"), "synthetic outside").unwrap();
            Dirs { root }
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn read_within_reads_inside_and_refuses_outside() {
        let dirs = Dirs::new("basic");
        let inside = dirs.root.join("inside");
        assert_eq!(
            read_within(&inside.join("a.txt"), &inside).unwrap(),
            b"synthetic inside"
        );
        assert!(matches!(
            read_within(&inside.join("../outside/b.txt"), &inside),
            Err(ReadError::Outside)
        ));
        assert!(matches!(
            read_within(&inside.join("missing.txt"), &inside),
            Err(ReadError::Io(e)) if e.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(
            read_within(&inside.join("../outside/missing.txt"), &inside),
            Err(ReadError::Outside)
        ));
    }

    /// The handle, not the path, is what has to be inside: a file opened
    /// outside the root does not pass because its path now resolves inside.
    #[test]
    fn a_handle_to_another_file_does_not_match_an_inside_path() {
        let dirs = Dirs::new("identity");
        let outside = File::open(dirs.root.join("outside/b.txt")).unwrap();
        let inside = dirs.root.join("inside/a.txt").canonicalize().unwrap();
        assert_ne!(
            identity::of_file(&outside).unwrap(),
            identity::of_path(&inside).unwrap()
        );
        let same = File::open(&inside).unwrap();
        assert_eq!(
            identity::of_file(&same).unwrap(),
            identity::of_path(&inside).unwrap()
        );
    }
}
