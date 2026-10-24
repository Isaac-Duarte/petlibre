//! Stock-compatible ADC calibration and 16-sample scaling.

pub const SAMPLES: usize = 16;
const SAMPLES_U32: u32 = 16;
const _: () = assert!(SAMPLES_U32 as usize == SAMPLES);
pub const MAX_RAW: u16 = 4095;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdcCalibration {
    pub offset: u16,
    pub gain: u16,
    pub default_offset: bool,
    pub default_gain: bool,
}

impl AdcCalibration {
    pub fn valid(&self) -> bool {
        self.gain != 0 && self.gain != u16::MAX && u32::from(self.offset) < u32::from(MAX_RAW) * 10
    }

    /// Four successfully read efuse bytes at 0x1d0..0x1d3, little endian.
    pub fn decode(efuse: &[u8; 4]) -> Option<Self> {
        let offset = u16::from_le_bytes([efuse[0], efuse[1]]);
        let gain = u16::from_le_bytes([efuse[2], efuse[3]]);
        // Erased (all-ones) efuse words fall back to the default offset and gain.
        let candidate = Self {
            offset: if offset == u16::MAX { 2480 } else { offset },
            gain: if gain == u16::MAX { 12050 } else { gain },
            default_offset: offset == u16::MAX,
            default_gain: gain == u16::MAX,
        };
        candidate.valid().then_some(candidate)
    }

    /// Match stock's floor-mean-then-scale order for valid, above-offset input.
    /// Below-offset input saturates to zero instead of wrapping. Units are the
    /// stock scaled quantity; current's intended mA interpretation and battery
    /// divider still need physical calibration checks.
    pub fn scale_16(&self, samples: &[u16; SAMPLES]) -> Option<u32> {
        if !self.valid() {
            return None;
        }

        let mut sum = 0u32;
        for &sample in samples {
            if sample > MAX_RAW {
                return None;
            }
            sum += u32::from(sample);
        }

        let mean_tenths = (sum / SAMPLES_U32) * 10;
        // At most 40,950,000 after multiplication; no 32-bit overflow.
        let offset = u32::from(self.offset);
        Some(if mean_tenths <= offset {
            0
        } else {
            ((mean_tenths - offset) * 1000) / u32::from(self.gain)
        })
    }
}
