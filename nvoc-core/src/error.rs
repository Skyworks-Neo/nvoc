use quick_error::quick_error;
use std::io;
use std::num::{ParseFloatError, ParseIntError};

/// Walk a clap [`Command`] tree recursively and collect every registered long
/// option / flag name (e.g. `"gpu"`, `"output-format"`, …).
fn collect_long_flags(cmd: &clap::Command, out: &mut Vec<String>) {
    for arg in cmd.get_arguments() {
        if let Some(long) = arg.get_long() {
            out.push(long.to_string());
        }
    }
    for sub in cmd.get_subcommands() {
        collect_long_flags(sub, out);
    }
}

/// Scan `std::env::args()` for single-dash long options (e.g. `-gpu=0`).
///
/// clap silently mis-parses `-gpu=0` as short flag `-g` with value `pu=0`,
/// which causes cryptic panics deep inside the program.  This function catches
/// the mistake early and returns a human-readable error that suggests the
/// correct `--` form.
///
/// Because the list of known flags is derived directly from the clap
/// [`Command`] object, it stays in sync automatically whenever new arguments
/// are added to `arg_help.rs`.
pub fn check_single_dash_args(cmd: &clap::Command) -> Result<(), Box<dyn std::error::Error>> {
    let mut known_longs: Vec<String> = Vec::new();
    collect_long_flags(cmd, &mut known_longs);

    for arg in std::env::args().skip(1) {
        // Only interested in single-dash tokens that are NOT `--` or `-`
        if !arg.starts_with('-') || arg.starts_with("--") || arg == "-" {
            continue;
        }
        // Strip the single leading dash; isolate the flag name before any `=`
        let body = arg.trim_start_matches('-');
        let flag_name = body.split('=').next().unwrap_or(body);

        if known_longs.iter().any(|l| l == flag_name) {
            return Err(format!("invalid option {:?} -- did you mean --{}?", arg, body).into());
        }
    }
    Ok(())
}

type JsonError = std::convert::Infallible;

impl From<csv::Error> for Error {
    fn from(err: csv::Error) -> Self {
        Error::Custom(format!("CSV Error: {}", err)) // Adjust based on your Error definition
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Custom(format!("JSON Error: {}", err))
    }
}

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        Nvapi(err: nvapi_hi::Error) {from()source(err)display("NVAPI error: {}", err)}
        VfpUnsupported {display("VFP unsupported")}
        DeviceNotFound {display("no matching device found")}
        Io(err: io::Error) {from()source(err)display("IO error: {}", err)}
        Json(err: JsonError) {from()source(err)display("JSON error: {}", err)}
        ParseInt(err: ParseIntError) {from()source(err)display("{}", err)}
        ParseFloat(err: ParseFloatError) {from()source(err)display("{}", err)}
        Str(err: &'static str) {from()display("{}", err)}
        FeatureUnsupportedErr{display("Feature unsupported")}
        Custom(err: String) { from() display("{}", err) }  // Corrected syntax
    }
}

impl From<nvapi_hi::NvapiError> for Error {
    fn from(e: nvapi_hi::NvapiError) -> Self {
        Self::from(nvapi_hi::Error::from(e))
    }
}
