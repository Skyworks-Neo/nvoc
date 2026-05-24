use nvapi_hi::{Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta};

pub type Millivolts = u32;
pub type MillivoltsDelta = i32;
pub type Megahertz = u32;
pub type MegahertzDelta = i32;

pub(crate) fn mv_to_uv(value: Millivolts) -> Microvolts {
    Microvolts(value.saturating_mul(1000))
}

pub(crate) fn uv_to_mv(value: Microvolts) -> Millivolts {
    value.0 / 1000
}

pub(crate) fn mv_delta_to_uv_delta(value: MillivoltsDelta) -> MicrovoltsDelta {
    MicrovoltsDelta(value.saturating_mul(1000))
}

pub(crate) fn uv_delta_to_mv_delta(value: MicrovoltsDelta) -> MillivoltsDelta {
    value.0 / 1000
}

pub(crate) fn mhz_to_khz(value: Megahertz) -> Kilohertz {
    Kilohertz(value.saturating_mul(1000))
}

pub(crate) fn mhz_delta_to_khz_delta(value: MegahertzDelta) -> KilohertzDelta {
    KilohertzDelta(value.saturating_mul(1000))
}

pub trait IntoMegahertzDelta {
    fn into_mhz_delta(self) -> MegahertzDelta;
}

impl IntoMegahertzDelta for MegahertzDelta {
    fn into_mhz_delta(self) -> MegahertzDelta {
        self
    }
}

impl IntoMegahertzDelta for KilohertzDelta {
    fn into_mhz_delta(self) -> MegahertzDelta {
        self.0 / 1000
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VfpResetDomain {
    All,
    Core,
    Memory,
}

#[derive(Clone, Copy, Debug)]
pub enum NvapiLockedVoltageTarget {
    Point(usize),
    Voltage(Millivolts),
}
