//! The repository checkout whose ignored `private/` holds every input and
//! output: reviews, reads, plans, journals and reports. An input outside it,
//! or one that only reaches it through a symlink out, is refused.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, require};

#[derive(Debug, Clone)]
pub struct Root {
    root: PathBuf,
    private: PathBuf,
}

impl Root {
    pub fn new(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let private = root.join("private");
        require(
            private.is_dir(),
            format!("{} has no private/ evidence directory", root.display()),
        )?;
        Ok(Self {
            private: private.canonicalize()?,
            root,
        })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.join(path)
    }

    /// Whether `path` resolves inside `private/`; a missing path is judged by
    /// its nearest existing ancestor.
    pub fn is_private(&self, path: &Path) -> bool {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        // Python's non-strict `resolve()`: walk the components, resolving
        // symlinks wherever the path so far exists, and `..` lexically
        // where it does not.
        let mut full = PathBuf::new();
        for part in absolute.components() {
            match part {
                Component::ParentDir => {
                    full.pop();
                }
                Component::CurDir => {}
                other => full.push(other),
            }
            if let Ok(resolved) = full.canonicalize() {
                full = resolved;
            }
        }
        full.starts_with(&self.private)
    }

    /// A regular private input file.
    pub fn private_file(&self, path: &Path) -> Result<Vec<u8>> {
        require(
            self.is_private(path) && path.is_file(),
            "Regular private input required",
        )?;
        Ok(fs::read(path)?)
    }

    /// A new private output directory.
    pub fn output_directory(&self, path: &Path) -> Result<PathBuf> {
        require(
            !path.is_symlink() && self.is_private(path),
            "Output must be a new private directory",
        )?;

        fs::DirBuilder::new()
            .recursive(false)
            .mode(0o700)
            .create(path)
            .or_else(|error| {
                // Python's mkdir(parents=True) for the reader outputs.
                if error.kind() == io::ErrorKind::NotFound {
                    fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(path)
                } else {
                    Err(error)
                }
            })?;
        Ok(std::path::absolute(path)?)
    }
}
