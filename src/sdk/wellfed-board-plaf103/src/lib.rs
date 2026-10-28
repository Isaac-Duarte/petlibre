//! The PLAF103 board, as the replacement firmware sees it: the pins and ADC
//! channels its ports use, and who owns each sector of the storage region.
//! These are facts of this feeder model, not of the AmebaD SoC, so the SoC
//! bindings (`wellfed-amebad-sys`) and every store read them from here;
//! a second model would be a second crate of the same shape, chosen at build
//! time. Everything is a compile-time constant: nothing reads the board at
//! run time.
#![no_std]

/// Every GPIO and ADC channel the ports use, in one place.
/// A GPIO is `port * 32 + bit` (PA13 is 13, PB3 is 35). Roles and
/// polarities are stock-derived and bench-checked where the cards say so;
/// [device layer](../../../../docs/firmware/device-layer.md) lists what else
/// is board-specific and where it lives.
pub mod pins {
    /// Motor driver inputs; both HIGH requests STOP (brake).
    pub const MOTOR_A: u32 = 13;
    pub const MOTOR_B: u32 = 14;
    /// Infrared emitter latches, LOW for OFF. PB3 is also SWD_CLK.
    pub const IR_PA: u32 = 15;
    pub const IR_PB: u32 = 32 + 3;
    /// Sensor inputs: outlet beam, food beam, feedback switch, adapter.
    pub const OUTLET: u32 = 16;
    pub const FOOD: u32 = 17;
    pub const FEEDBACK: u32 = 18;
    pub const ADAPTER: u32 = 19;
    /// Panel buttons, active LOW: reset, mic, dispense, lock (PB4–PB7).
    pub const BUTTONS: [u32; 4] = [32 + 4, 32 + 5, 32 + 6, 32 + 7];
    /// Indicators: connection, food, lock (PA0, PA2, PA4) and fault (PB22).
    pub const LED_CONNECTION: u32 = 0;
    pub const LED_FOOD: u32 = 2;
    pub const LED_LOCK: u32 = 4;
    pub const LED_FAULT: u32 = 32 + 22;
    /// Speaker amplifier enable, HIGH while playing.
    pub const AMP: u32 = 32 + 23;
    /// An active buzzer: HIGH beeps at its own pitch. Stock pulses it for
    /// 200 ms, and the feeder beeps it for each button press.
    pub const BUZZER: u32 = 25;
    /// ADC channels: motor current and the battery pack. Channel 10, the
    /// chip's own supply, is the AmebaD's, not the board's.
    pub const ADC_CURRENT: u32 = 4;
    pub const ADC_BATTERY: u32 = 5;
    pub const ADC_SUPPLY: u32 = 10;

    /// The pin's bit in its port register.
    pub const fn bit(pin: u32) -> u32 {
        1 << (pin & 31)
    }

    /// Motor driver inputs PA13 and PA14 (port A); both HIGH requests STOP.
    pub const MOTOR_A_BIT: u32 = bit(MOTOR_A);
    pub const MOTOR_B_BIT: u32 = bit(MOTOR_B);
    pub const MOTOR_MASK: u32 = MOTOR_A_BIT | MOTOR_B_BIT;
    /// Infrared emitter latches, LOW for OFF: PA15 on port A and PB3 (also
    /// SWD_CLK) on port B.
    pub const IR_PA_BIT: u32 = bit(IR_PA);
    pub const IR_PB_BIT: u32 = bit(IR_PB);
}

/// Owner-approved replacement-firmware storage: 48 sectors at
/// 0x340000-0x370000. The owner approved the first 16, extended them to 32
/// (October 23, 2026) for client keys, plans and the 7-day event ring, and
/// to 48 (October 26) for the lifetime statistics and plans. A static pass
/// over stock 1.1.15 found no fixed or bounded computed flash access in
/// 0x306000-0x3e8000, and every full dump of the research unit is blank
/// there. Clear of bank 0, of a full-size bank 1 (ends no later than
/// 0x306000) and of the stock settings (0x3e8000). The region only ever
/// grows upward, so a sector keeps its address and an older build, whose
/// bound is smaller, never reaches the sectors added after it.
/// Callers name a sector index; nothing takes a flash address.
pub const STORAGE_OFFSET: u32 = 0x34_0000;
pub const STORAGE_SECTORS: u32 = 48;

/// Who owns each sector of the storage region, in one place. Each
/// store keeps its sectors to itself, so a torn record is always its own.
pub mod storage {
    use super::STORAGE_SECTORS;

    /// `count` sectors from region sector `first`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Sectors {
        pub first: u32,
        pub count: u32,
    }

    impl Sectors {
        const fn end(self) -> u32 {
            self.first + self.count
        }
    }

    /// The feed journal's slot pair.
    pub const JOURNAL: Sectors = Sectors { first: 0, count: 2 };
    /// Device settings.
    pub const SETTINGS: Sectors = Sectors { first: 2, count: 2 };
    /// The boot confirmation record.
    pub const CONFIRM: Sectors = Sectors { first: 4, count: 1 };
    /// Wi-Fi credentials.
    pub const NET: Sectors = Sectors { first: 5, count: 2 };
    /// The random generator's seed carried across boots.
    pub const SEED: Sectors = Sectors { first: 7, count: 2 };
    /// The feeder's identity key and enrolled clients.
    pub const CLIENTS: Sectors = Sectors { first: 9, count: 2 };
    /// The feeding plans, their marks and the last claim.
    pub const PLANS: Sectors = Sectors {
        first: 11,
        count: 2,
    };
    /// Reserved for the 7-day event ring: 11 sectors of records and one
    /// being erased (sized by the resource budget).
    pub const EVENTS: Sectors = Sectors {
        first: 13,
        count: 12,
    };

    /// The update store: the anti-rollback floor, the give-way's
    /// candidate, the added and retired update keys, and bank 0's image.
    /// Not the confirmation sector, which is erased before each write, so
    /// a cut there would lose a floor kept in it.
    pub const UPDATE: Sectors = Sectors {
        first: 25,
        count: 2,
    };
    /// The time settings: the owner's zone and the NTP server. Free
    /// sectors before, so a build that predates the store never touches
    /// them.
    pub const TIME: Sectors = Sectors {
        first: 27,
        count: 2,
    };
    /// The per-feed trace ring: four 1 KiB traces a sector, the last 8 to
    /// 12 feeds. No build before it wrote these sectors.
    pub const TRACES: Sectors = Sectors {
        first: 29,
        count: 3,
    };

    /// The lifetime statistics: totals over every boot, resets by cause
    /// and erases by sector. The first sectors past the 32-sector region, so
    /// a build that predates them never reads or writes them.
    pub const STATS: Sectors = Sectors {
        first: 32,
        count: 2,
    };
    /// The portion calibration: what one portion weighs. Sectors no
    /// earlier build allocated, so a build that predates the store never
    /// touches them.
    pub const CALIBRATION: Sectors = Sectors {
        first: 34,
        count: 2,
    };

    /// The factory reset's marker: written before the reset erases
    /// anything and erased after it erased everything, so a boot that finds
    /// it finishes the reset. Blank in every earlier build.
    pub const RESET: Sectors = Sectors {
        first: 36,
        count: 1,
    };

    /// What the factory reset erases, in the order it erases them: the
    /// clients and identity first, then the Wi-Fi credentials, the
    /// settings, the time settings, the plans, the statistics, the traces
    /// and the event history. It keeps the feed journal (a feed is never
    /// re-run and its numbers never go back), the portion calibration (a
    /// property of the hardware), the update store and confirmation record
    /// (the anti-rollback floor and the build in trial) and the generator's
    /// seed.
    pub const FACTORY_CLEARED: [Sectors; 8] =
        [CLIENTS, NET, SETTINGS, TIME, PLANS, STATS, TRACES, EVENTS];

    /// The sectors [`FACTORY_CLEARED`] names, one by one.
    pub const FACTORY_CLEARED_SECTORS: u32 = cleared_count();

    const fn cleared_count() -> u32 {
        let mut total = 0;
        let mut i = 0;
        while i < FACTORY_CLEARED.len() {
            total += FACTORY_CLEARED[i].count;
            i += 1;
        }
        total
    }

    /// The `index`th sector the factory reset erases, in its order.
    pub const fn factory_cleared(index: u32) -> Option<u32> {
        let mut skipped = 0;
        let mut i = 0;
        while i < FACTORY_CLEARED.len() {
            let store = FACTORY_CLEARED[i];
            if index < skipped + store.count {
                return Some(store.first + (index - skipped));
            }
            skipped += store.count;
            i += 1;
        }
        None
    }

    /// Every allocation, in region order; sectors 37-47 are free.
    pub const ALL: [Sectors; 14] = [
        JOURNAL,
        SETTINGS,
        CONFIRM,
        NET,
        SEED,
        CLIENTS,
        PLANS,
        EVENTS,
        UPDATE,
        TIME,
        TRACES,
        STATS,
        CALIBRATION,
        RESET,
    ];

    const fn disjoint_and_inside() -> bool {
        let mut i = 1;
        while i < ALL.len() {
            if ALL[i - 1].end() > ALL[i].first || ALL[i].count == 0 {
                return false;
            }
            i += 1;
        }
        ALL[ALL.len() - 1].end() <= STORAGE_SECTORS
    }
    const _: () = assert!(disjoint_and_inside());
}
