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

/// Owner-approved replacement-firmware storage: 32 sectors at
/// 0x340000-0x360000. The owner approved the first 16, then extended it with the
/// owner's approval (October 23, 2026) for client keys, plans and the 7-day
/// event ring. A static pass over stock 1.1.15 found no fixed or bounded
/// computed flash access in 0x306000-0x3e8000, and every full dump of the
/// research unit is blank there. Clear of bank 0, of a full-size bank 1
/// (ends no later than 0x306000) and of the stock settings (0x3e8000).
/// Callers name a sector index; nothing takes a flash address.
pub const STORAGE_OFFSET: u32 = 0x34_0000;
pub const STORAGE_SECTORS: u32 = 32;

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
    /// Reserved for feeding plans.
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

    /// Every allocation, in region order; sectors 29-31 are free (planned
    /// for the per-feed trace ring).
    pub const ALL: [Sectors; 10] = [
        JOURNAL, SETTINGS, CONFIRM, NET, SEED, CLIENTS, PLANS, EVENTS, UPDATE, TIME,
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
