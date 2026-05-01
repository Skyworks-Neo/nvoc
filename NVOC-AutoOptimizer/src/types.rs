use nvapi_hi::Microvolts;

#[derive(Debug, Copy, Clone)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ResetSettings {
    VoltageBoost,
    SensorLimits,
    PowerLimits,
    CoolerLevels,
    VfpDeltas,
    VfpLock,
    PStateDeltas,
    Overvolt,
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
    Voltage(Microvolts),
}

pub const POSSIBLE_BOOL_OFF: &str = "off";
pub const POSSIBLE_BOOL_ON: &str = "on";
pub const POSSIBLE_BOOL: &[&str] = &[POSSIBLE_BOOL_OFF, POSSIBLE_BOOL_ON];
