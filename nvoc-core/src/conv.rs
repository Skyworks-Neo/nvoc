use super::error::Error;
use super::types::VfpResetDomain;
use nvapi_hi::{ClockDomain, CoolerPolicy, PState};
use nvml_wrapper::enum_wrappers::device::PerformanceState;

pub fn try_parse_nvml_pstate(nvml_pstate_val: &str) -> Result<PerformanceState, Error> {
    let pstate = nvml_pstate_val
        .trim()
        .trim_start_matches('P')
        .trim_start_matches('p');

    match pstate {
        "0" => Ok(PerformanceState::Zero),
        "1" => Ok(PerformanceState::One),
        "2" => Ok(PerformanceState::Two),
        "3" => Ok(PerformanceState::Three),
        "4" => Ok(PerformanceState::Four),
        "5" => Ok(PerformanceState::Five),
        "6" => Ok(PerformanceState::Six),
        "7" => Ok(PerformanceState::Seven),
        "8" => Ok(PerformanceState::Eight),
        "9" => Ok(PerformanceState::Nine),
        "10" => Ok(PerformanceState::Ten),
        "11" => Ok(PerformanceState::Eleven),
        "12" => Ok(PerformanceState::Twelve),
        "13" => Ok(PerformanceState::Thirteen),
        "14" => Ok(PerformanceState::Fourteen),
        "15" => Ok(PerformanceState::Fifteen),
        _ => Err(Error::Custom(format!(
            "Invalid NVML PState {}",
            nvml_pstate_val
        ))),
    }
}

pub fn parse_nvml_pstate(nvml_pstate_val: &str) -> PerformanceState {
    try_parse_nvml_pstate(nvml_pstate_val).unwrap_or_else(|e| {
        eprintln!("{}, defaulting to PState 0", e);
        PerformanceState::Zero
    })
}

pub fn nvml_pstate_to_str(pstate: PerformanceState) -> &'static str {
    match pstate {
        PerformanceState::Zero => "P0",
        PerformanceState::One => "P1",
        PerformanceState::Two => "P2",
        PerformanceState::Three => "P3",
        PerformanceState::Four => "P4",
        PerformanceState::Five => "P5",
        PerformanceState::Six => "P6",
        PerformanceState::Seven => "P7",
        PerformanceState::Eight => "P8",
        PerformanceState::Nine => "P9",
        PerformanceState::Ten => "P10",
        PerformanceState::Eleven => "P11",
        PerformanceState::Twelve => "P12",
        PerformanceState::Thirteen => "P13",
        PerformanceState::Fourteen => "P14",
        PerformanceState::Fifteen => "P15",
        PerformanceState::Unknown => "Unknown",
    }
}

pub fn nvml_pstate_to_index(pstate: PerformanceState) -> Result<u8, Error> {
    match pstate {
        PerformanceState::Zero => Ok(0),
        PerformanceState::One => Ok(1),
        PerformanceState::Two => Ok(2),
        PerformanceState::Three => Ok(3),
        PerformanceState::Four => Ok(4),
        PerformanceState::Five => Ok(5),
        PerformanceState::Six => Ok(6),
        PerformanceState::Seven => Ok(7),
        PerformanceState::Eight => Ok(8),
        PerformanceState::Nine => Ok(9),
        PerformanceState::Ten => Ok(10),
        PerformanceState::Eleven => Ok(11),
        PerformanceState::Twelve => Ok(12),
        PerformanceState::Thirteen => Ok(13),
        PerformanceState::Fourteen => Ok(14),
        PerformanceState::Fifteen => Ok(15),
        PerformanceState::Unknown => Err(Error::Custom("Invalid NVML PState Unknown".to_string())),
    }
}

pub trait ConvertEnum: Sized {
    fn from_str(s: &str) -> Result<Self, Error>;
    fn to_str(&self) -> &'static str;
    fn possible_values() -> &'static [&'static str];
    fn possible_values_typed() -> &'static [Self];
}

macro_rules! enum_from_str {
    (
        $conv:ident => {
        $(
            $item:ident = $str:expr,
        )*
            _ => $err:expr,
        }
    ) => {
        impl ConvertEnum for $conv {
            fn from_str(s: &str) -> Result<Self, Error> {
                match s {
                $(
                    $str => Ok($conv::$item),
                )*
                    _ => Err(($err).into()),
                }
            }

            #[allow(unreachable_patterns)]
            fn to_str(&self) -> &'static str {
                match *self {
                $(
                    $conv::$item => $str,
                )*
                    _ => "unknown",
                }
            }

            fn possible_values() -> &'static [&'static str] {
                &[$(
                    $str,
                )*]
            }

            fn possible_values_typed() -> &'static [Self] {
                &[$(
                    $conv::$item,
                )*]
            }
        }
    };
}

enum_from_str! {
    VfpResetDomain => {
        All = "all",
        Core = "core",
        Memory = "memory",
        _ => "unknown vfp reset domain",
    }
}

enum_from_str! {
    PState => {
        P0 = "P0",
        P1 = "P1",
        P2 = "P2",
        P3 = "P3",
        P4 = "P4",
        P5 = "P5",
        P6 = "P6",
        P7 = "P7",
        P8 = "P8",
        P9 = "P9",
        P10 = "P10",
        P11 = "P11",
        P12 = "P12",
        P13 = "P13",
        P14 = "P14",
        P15 = "P15",
        _ => "unknown pstate",
    }
}

enum_from_str! {
    ClockDomain => {
        Graphics = "graphics",
        Memory = "memory",
        Processor = "processor",
        Video = "video",
        _ => "unknown clock type",
    }
}

enum_from_str! {
    CoolerPolicy => {
        None = "default",
        Manual = "manual",
        Performance = "perf",
        TemperatureDiscrete = "discrete",
        TemperatureContinuous = "continuous",
        Hybrid = "hybrid",
        TemperatureContinuousSoftware = "software",
        Default = "default32",
        _ => "unknown cooler policy",
    }
}
