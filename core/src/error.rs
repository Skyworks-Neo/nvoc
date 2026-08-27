use quick_error::quick_error;
use std::io;
use std::num::{ParseFloatError, ParseIntError};

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        Nvapi(err: ::nvapi::hi::Error) {from()source(err)display("NVAPI error: {}", err)}
        VfpUnsupported {display("VFP unsupported")}
        DeviceNotFound {display("no matching device found")}
        Io(err: io::Error) {from()source(err)display("IO error: {}", err)}
        ParseInt(err: ParseIntError) {from()source(err)display("{}", err)}
        ParseFloat(err: ParseFloatError) {from()source(err)display("{}", err)}
        Str(err: &'static str) {from()display("{}", err)}
        FeatureUnsupportedErr{display("Feature unsupported")}
        Custom(err: String) { from() display("{}", err) }  // Corrected syntax
    }
}

impl From<::nvapi::hi::NvapiError> for Error {
    fn from(e: ::nvapi::hi::NvapiError) -> Self {
        Self::from(::nvapi::hi::Error::from(e))
    }
}

impl Error {
    pub fn is_allowable_nvapi_reset_error(&self) -> bool {
        matches!(
            self,
            Error::Nvapi(::nvapi::hi::Error::Nvapi(::nvapi::hi::NvapiError {
                status: ::nvapi::hi::Status::NotSupported | ::nvapi::hi::Status::NoImplementation,
                ..
            })) | Error::Nvapi(::nvapi::hi::Error::ArgumentRange(..))
        )
    }
}
