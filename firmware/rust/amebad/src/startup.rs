//! Versioned startup records, formatted without allocation or device writes.

use core::ffi::CStr;
use core::fmt::{self, Write};

pub const BUILD_ID: &str = env!("PETLIBRE_BUILD_ID");

/// Raw live mapping registers and the factory-preserved reset flags.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub control: u32,
    pub start: u32,
    pub offset: u32,
    pub reset: u32,
}

/// Read the live KM0 image mapping and the factory-preserved ROM reset flags.
#[cfg(all(target_os = "none", feature = "km4"))]
pub fn snapshot() -> Snapshot {
    let entry = crate::sdk::flash_mmu_entry0();
    Snapshot {
        control: entry.control,
        start: entry.start,
        offset: entry.offset,
        reset: crate::sdk::boot_reason(),
    }
}

impl Snapshot {
    /// Zero-based research-board bank, or unknown for an unrecognized mapping.
    pub fn bank(self) -> Option<u8> {
        if self.control & 1 == 0 || self.start != 0x0c00_0000 {
            return None;
        }
        let physical = if self.control & 2 != 0 {
            self.start.checked_sub(self.offset)?
        } else {
            self.start.checked_add(self.offset)?
        };
        match physical {
            0x0800_6000 => Some(0),
            0x0818_6000 => Some(1),
            _ => None,
        }
    }
}

/// Enough for the fixed schema with a full SHA-256 build ID and raw registers.
pub struct Report {
    bytes: [u8; 192],
    used: usize,
}

impl Default for Report {
    fn default() -> Self {
        Self::new()
    }
}

impl Report {
    pub const fn new() -> Self {
        Self {
            bytes: [0; 192],
            used: 0,
        }
    }

    pub fn start(&mut self, snapshot: Snapshot) -> Result<&CStr, fmt::Error> {
        let bank = match snapshot.bank() {
            Some(0) => "0",
            Some(1) => "1",
            _ => "unknown",
        };
        self.record(format_args!(
            concat!(
                "PLFW1 start build=",
                env!("PETLIBRE_BUILD_ID"),
                " bank={} reset={:08x} mmu={:08x},{:08x},{:08x}\r\n"
            ),
            bank, snapshot.reset, snapshot.control, snapshot.start, snapshot.offset
        ))
    }

    /// Format a complete bounded record, replacing any previous buffer contents.
    pub fn record(&mut self, args: fmt::Arguments<'_>) -> Result<&CStr, fmt::Error> {
        self.used = 0;
        self.write_fmt(args)?;
        self.bytes[self.used] = 0;
        CStr::from_bytes_with_nul(&self.bytes[..=self.used]).map_err(|_| fmt::Error)
    }
}

impl Write for Report {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.used.checked_add(value.len()).ok_or(fmt::Error)?;
        if end >= self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.used..end].copy_from_slice(value.as_bytes());
        self.used = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_factory_mappings_and_unknown_cases() {
        let mut s = Snapshot {
            control: 3,
            start: 0x0c00_0000,
            offset: 0x3ff_a000,
            reset: 0x10_0000,
        };
        assert_eq!(s.bank(), Some(0));
        s.offset = 0x3e7_a000;
        assert_eq!(s.bank(), Some(1));
        s.control = 0;
        assert_eq!(s.bank(), None);
        s.control = 1;
        s.offset = u32::MAX;
        assert_eq!(s.bank(), None);
        s.control = 3;
        assert_eq!(s.bank(), None);
        s.offset = 0;
        assert_eq!(s.bank(), None);
        s.start = 0;
        assert_eq!(s.bank(), None);
    }

    #[test]
    fn report_retains_raw_reason_and_reuses_buffer_without_stale_bytes() {
        let mut report = Report::new();
        let mut s = Snapshot {
            control: 0,
            start: 0x0c00_0000,
            offset: 0x3e7_a000,
            reset: 0x10_0000,
        };
        let line = report.start(s).unwrap().to_str().unwrap();
        assert!(line.contains(" bank=unknown reset=00100000 mmu=00000000,0c000000,03e7a000\r\n"));
        s.control = 3;
        s.reset = 0;
        let line = report.start(s).unwrap().to_str().unwrap();
        assert!(line.contains(" bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000\r\n"));
        assert!(line.starts_with(&format!("PLFW1 start build={BUILD_ID} ")));
    }
}
