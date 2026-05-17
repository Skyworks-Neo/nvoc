use nvoc_core::{ConvertEnum, Error};

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

pub const POSSIBLE_BOOL_OFF: &str = "off";
pub const POSSIBLE_BOOL_ON: &str = "on";
pub const POSSIBLE_BOOL: &[&str] = &[POSSIBLE_BOOL_OFF, POSSIBLE_BOOL_ON];

macro_rules! impl_convert_enum {
    (
        $ty:ident => {
        $(
            $variant:ident = $value:expr,
        )*
            _ => $err:expr,
        }
    ) => {
        impl ConvertEnum for $ty {
            fn from_str(s: &str) -> Result<Self, Error> {
                match s {
                $(
                    $value => Ok(Self::$variant),
                )*
                    _ => Err(($err).into()),
                }
            }

            fn to_str(&self) -> &'static str {
                match *self {
                $(
                    Self::$variant => $value,
                )*
                }
            }

            fn possible_values() -> &'static [&'static str] {
                &[$($value,)*]
            }

            fn possible_values_typed() -> &'static [Self] {
                &[$(Self::$variant,)*]
            }
        }
    };
}

impl_convert_enum! {
    OutputFormat => {
        Human = "human",
        Json = "json",
        _ => "unknown output format",
    }
}

impl_convert_enum! {
    ResetSettings => {
        VoltageBoost = "voltage-boost",
        SensorLimits = "thermal",
        PowerLimits = "power",
        CoolerLevels = "nvapi-cooler",
        VfpDeltas = "vfp",
        VfpLock = "lock",
        PStateDeltas = "pstate",
        Overvolt = "overvolt",
        _ => "unknown setting",
    }
}
