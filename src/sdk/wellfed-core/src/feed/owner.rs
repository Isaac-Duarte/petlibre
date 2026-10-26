//! The feeding owner's published state and fault, as its `PLOWN1 state=`
//! records number them. The owner itself is the HAL's; the numbers live
//! here so the bench decodes the same table the firmware prints.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OwnerState {
    #[default]
    Starting = 0,
    Idle = 1,
    Queued = 2,
    Preparing = 3,
    Feeding = 4,
    Reporting = 5,
    Terminal = 6,
    Fault = 7,
    Stopped = 8,
}

impl OwnerState {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// The variant a record's number names; `None` for any other number.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Starting),
            1 => Some(Self::Idle),
            2 => Some(Self::Queued),
            3 => Some(Self::Preparing),
            4 => Some(Self::Feeding),
            5 => Some(Self::Reporting),
            6 => Some(Self::Terminal),
            7 => Some(Self::Fault),
            8 => Some(Self::Stopped),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OwnerFault {
    #[default]
    Ok = 0,
    Setup = 1,
    Recovery = 2,
    Storage = 3,
    Hardware = 4,
    /// The owner's own beat fell overdue: a step blocked past its gap.
    Progress = 5,
    Flash = 6,
    /// The sensors could not be read (an ADC conversion that never
    /// completed): held until a restart, with the outputs stopped.
    Sensor = 7,
}

impl OwnerFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// The variant a record's number names; `None` for any other number.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Ok),
            1 => Some(Self::Setup),
            2 => Some(Self::Recovery),
            3 => Some(Self::Storage),
            4 => Some(Self::Hardware),
            5 => Some(Self::Progress),
            6 => Some(Self::Flash),
            7 => Some(Self::Sensor),
            _ => None,
        }
    }
}
