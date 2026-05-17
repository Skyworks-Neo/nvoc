use nvapi_hi::Microvolts;

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
