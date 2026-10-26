//! The storage region as one image, in memory or in a file, carved into the
//! firmware's sector allocation ([`wellfed_board_plaf103::storage`]) so the
//! real stores run on it unchanged.
//!
//! It keeps NOR's rules, as the fake SDK does: an erase sets a whole sector
//! to `0xff`, and a program only clears bits, refusing one that would set a
//! bit back. A file-backed image is rewritten whole after every erase and
//! program (128 KiB, atomically), so a stopped simulator restarts with what
//! it last committed, as a power cut leaves the feeder.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use wellfed_amebad_sys::SECTOR_BYTES;
use wellfed_board_plaf103::STORAGE_SECTORS;
use wellfed_board_plaf103::storage::Sectors;
use wellfed_core::store::slot::SlotIo;

/// The whole storage region's size.
pub const IMAGE_BYTES: usize = (STORAGE_SECTORS * SECTOR_BYTES) as usize;

#[derive(Debug, thiserror::Error)]
pub enum FlashError {
    #[error("read the flash image {path}: {error}")]
    Read { path: PathBuf, error: io::Error },
    #[error("the flash image {path} holds {len} bytes, not {IMAGE_BYTES}")]
    Size { path: PathBuf, len: usize },
}

/// The image every [`Region`] shares.
#[derive(Debug)]
pub struct Flash {
    bytes: RefCell<Vec<u8>>,
    file: Option<PathBuf>,
    /// Erases by sector since this process opened the image, for the
    /// statistics.
    erases: RefCell<[u32; STORAGE_SECTORS as usize]>,
}

impl Flash {
    /// An erased image that lives only in memory.
    pub fn memory() -> Rc<Self> {
        Rc::new(Self {
            bytes: RefCell::new(vec![0xff; IMAGE_BYTES]),
            file: None,
            erases: RefCell::new([0; STORAGE_SECTORS as usize]),
        })
    }

    /// The image in `path`, or an erased one that will be written there.
    pub fn file(path: &Path) -> Result<Rc<Self>, FlashError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => vec![0xff; IMAGE_BYTES],
            Err(error) => {
                return Err(FlashError::Read {
                    path: path.to_owned(),
                    error,
                });
            }
        };
        if bytes.len() != IMAGE_BYTES {
            return Err(FlashError::Size {
                path: path.to_owned(),
                len: bytes.len(),
            });
        }
        Ok(Rc::new(Self {
            bytes: RefCell::new(bytes),
            file: Some(path.to_owned()),
            erases: RefCell::new([0; STORAGE_SECTORS as usize]),
        }))
    }

    /// Erases by sector since the image was opened.
    pub(crate) fn erases(&self) -> [u32; STORAGE_SECTORS as usize] {
        *self.erases.borrow()
    }

    /// Commit the image to its file; false on failure, which the store
    /// sees as a failed write. The image in memory keeps the change, as a
    /// failed NOR write may still have landed; the file catches up on the
    /// next commit that succeeds.
    fn persist(&self) -> bool {
        let Some(path) = &self.file else {
            return true;
        };
        let temp = path.with_extension("tmp");
        fs::write(&temp, &*self.bytes.borrow()).is_ok() && fs::rename(&temp, path).is_ok()
    }
}

/// One store's sectors, each an erase unit.
#[derive(Clone, Debug)]
pub(crate) struct Region {
    flash: Rc<Flash>,
    sectors: Sectors,
}

impl Region {
    pub(crate) fn new(flash: &Rc<Flash>, sectors: Sectors) -> Self {
        Self {
            flash: Rc::clone(flash),
            sectors,
        }
    }

    /// The byte range `offset..offset + len` of unit `slot`, in the image.
    fn range(&self, slot: u32, offset: u32, len: usize) -> Option<std::ops::Range<usize>> {
        if slot >= self.sectors.count {
            return None;
        }
        let len = u32::try_from(len).ok()?;
        if offset.checked_add(len)? > SECTOR_BYTES {
            return None;
        }
        let start = (self.sectors.first + slot) * SECTOR_BYTES + offset;
        Some(start as usize..(start + len) as usize)
    }
}

impl SlotIo for Region {
    fn unit_bytes(&self) -> u32 {
        SECTOR_BYTES
    }

    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        let Some(range) = self.range(slot, offset, bytes.len()) else {
            return false;
        };
        bytes.copy_from_slice(&self.flash.bytes.borrow()[range]);
        true
    }

    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        let Some(range) = self.range(slot, offset, bytes.len()) else {
            return false;
        };
        {
            let mut image = self.flash.bytes.borrow_mut();
            let target = &mut image[range];
            if target
                .iter()
                .zip(bytes)
                .any(|(&old, &new)| old & new != new)
            {
                return false;
            }
            target.copy_from_slice(bytes);
        }
        self.flash.persist()
    }

    fn erase(&self, slot: u32) -> bool {
        let Some(range) = self.range(slot, 0, SECTOR_BYTES as usize) else {
            return false;
        };
        self.flash.bytes.borrow_mut()[range].fill(0xff);
        let sector = usize::try_from(self.sectors.first + slot).unwrap_or(usize::MAX);
        if let Some(count) = self.flash.erases.borrow_mut().get_mut(sector) {
            *count += 1;
        }
        self.flash.persist()
    }
}
