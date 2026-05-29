use clap::{Arg, ArgAction, ColorChoice, Command as ClapCommand};
use nvoc_core::{
    BackendSet, CheckVoltageFrequency, ClockDomain, ConvertEnum, CoolerPolicy, CoolerTarget,
    GpuSelector, GpuTarget, Kilohertz, KilohertzDelta, PState, Percentage, ProbeVoltageLimits,
    QueryClockOffset, QueryDomainVfpPoints, QueryFanInfo, QueryGpuInfo, QueryGpuSettings,
    QueryGpuStatus, QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta,
    QueryPowerLimits, QueryPstates, QuerySupportedApplicationsClocks, QueryTdpTempLimits,
    QueryTemperatureThresholds, QueryThrottleReasons, QueryVfpPointVoltage,
    ResetApplicationsClocks, ResetCoolerLevels, ResetFanSpeed, ResetLockedClocks,
    ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetPstateBaseVoltages,
    ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock, ResetVfpLock,
    SetApplicationsClocks, SetClockOffset, SetCoolerLevels, SetFanSpeed, SetLockedClocks,
    SetNvapiPowerLimits, SetNvapiPstateLock, SetNvapiSensorLimits, SetNvmlPstateLock,
    SetPowerLimit, SetPstateClockOffset, SetTemperatureLimit, SetVfpFrequencyLock,
    SetVfpPointDelta, SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, VfpResetDomain,
    discover_targets, nvml_pstate_to_str, parse_nvapi_locked_voltage_target,
    parse_nvml_fan_control_policy, parse_nvml_pstate, run, select_targets,
};
use serde_json::{Value, json};

mod output;
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub enum CliError {
    Message(String),
    Clap(clap::Error),
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn print_clap(&self) -> bool {
        if let Self::Clap(err) = self {
            let _ = err.print();
            true
        } else {
            false
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Message(_) => 2,
            Self::Clap(err) => err.exit_code(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Clap(err) if err.kind() == clap::error::ErrorKind::ArgumentConflict => {
                write!(f, "argument conflicts: {err}")
            }
            Self::Clap(err) => write!(f, "{err}"),
        }
    }
}

impl StdError for CliError {}

impl From<nvoc_core::Error> for CliError {
    fn from(value: nvoc_core::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        Self::new(value.to_string())
    }
}

type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Auto,
    Nvapi,
    Nvml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendAdapter {
    Nvapi,
    Nvml,
}

impl BackendAdapter {
    fn label(self) -> &'static str {
        match self {
            Self::Nvapi => "nvapi",
            Self::Nvml => "nvml",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ListGpus,
    GetInfo,
    GetStatus,
    GetSettings,
    GetVfp,
    GetVfpPointVoltageMv,
    GetPowerWatt,
    GetClockOffsetMhz,
    GetPstates,
    GetSupportedAppClocks,
    GetFanInfo,
    GetTemperatureThresholds,
    GetThrottleReasons,
    GetTdpTempLimits,
    ProbeVoltageLimits,
    CheckVoltageFrequency,
    GetLegacyOvervoltRanges,
    GetLegacyP0CoreMaxVoltageDelta,
    SetCoreOffsetMhz,
    SetMemoryOffsetMhz,
    SetPowerWatt,
    SetPowerPercent,
    SetThermalLimitC,
    SetFanPercent,
    SetLockedClocksMhz,
    SetVfpVoltageLock,
    SetVfpPointDeltaMhz,
    SetVfpRangeDeltaMhz,
    SetPstateLock,
    SetApplicationsClocksMhz,
    SetVoltageBoostPercent,
    ResetCoreOffsetMhz,
    ResetMemoryOffsetMhz,
    ResetApplicationsClocks,
    ResetLockedClocks,
    ResetFan,
    ResetVfpDeltas,
    ResetVfpLock,
    ResetPowerPercent,
    ResetThermalLimitC,
    ResetPstateBaseVoltages,
}

static NVAPI_ONLY: [BackendAdapter; 1] = [BackendAdapter::Nvapi];
static NVML_ONLY: [BackendAdapter; 1] = [BackendAdapter::Nvml];
static BOTH_BACKENDS: [BackendAdapter; 2] = [BackendAdapter::Nvapi, BackendAdapter::Nvml];

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Self::ListGpus => "list-gpus",
            Self::GetInfo => "get-info",
            Self::GetStatus => "get-status",
            Self::GetSettings => "get-settings",
            Self::GetVfp => "get-vfp",
            Self::GetVfpPointVoltageMv => "get-vfp-point-voltage-mv",
            Self::GetPowerWatt => "get-power-watt",
            Self::GetClockOffsetMhz => "get-clock-offset-mhz",
            Self::GetPstates => "get-pstates",
            Self::GetSupportedAppClocks => "get-supported-app-clocks",
            Self::GetFanInfo => "get-fan-info",
            Self::GetTemperatureThresholds => "get-temperature-thresholds",
            Self::GetThrottleReasons => "get-throttle-reasons",
            Self::GetTdpTempLimits => "get-tdp-temp-limits",
            Self::ProbeVoltageLimits => "probe-voltage-limits",
            Self::CheckVoltageFrequency => "check-voltage-frequency",
            Self::GetLegacyOvervoltRanges => "get-legacy-overvolt-ranges",
            Self::GetLegacyP0CoreMaxVoltageDelta => "get-legacy-p0-core-max-voltage-delta",
            Self::SetCoreOffsetMhz => "set-core-offset-mhz",
            Self::SetMemoryOffsetMhz => "set-memory-offset-mhz",
            Self::SetPowerWatt => "set-power-watt",
            Self::SetPowerPercent => "set-power-percent",
            Self::SetThermalLimitC => "set-thermal-limit-c",
            Self::SetFanPercent => "set-fan-percent",
            Self::SetLockedClocksMhz => "set-locked-clocks-mhz",
            Self::SetVfpVoltageLock => "set-vfp-voltage-lock",
            Self::SetVfpPointDeltaMhz => "set-vfp-point-delta-mhz",
            Self::SetVfpRangeDeltaMhz => "set-vfp-range-delta-mhz",
            Self::SetPstateLock => "set-pstate-lock",
            Self::SetApplicationsClocksMhz => "set-applications-clocks-mhz",
            Self::SetVoltageBoostPercent => "set-voltage-boost-percent",
            Self::ResetCoreOffsetMhz => "reset-core-offset-mhz",
            Self::ResetMemoryOffsetMhz => "reset-memory-offset-mhz",
            Self::ResetApplicationsClocks => "reset-applications-clocks",
            Self::ResetLockedClocks => "reset-locked-clocks",
            Self::ResetFan => "reset-fan",
            Self::ResetVfpDeltas => "reset-vfp-deltas",
            Self::ResetVfpLock => "reset-vfp-lock",
            Self::ResetPowerPercent => "reset-power-percent",
            Self::ResetThermalLimitC => "reset-thermal-limit-c",
            Self::ResetPstateBaseVoltages => "reset-pstate-base-voltages",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Self::ListGpus => "List discovered GPUs and available backends",
            Self::GetInfo => "Read NVAPI GPU identity and capability information",
            Self::GetStatus => "Read NVAPI live GPU status",
            Self::GetSettings => "Read NVAPI overclock settings",
            Self::GetVfp => "Read V-F curve points",
            Self::GetVfpPointVoltageMv => "Read one VFP point voltage in mV",
            Self::GetPowerWatt => "Read NVML power limits in watts",
            Self::GetClockOffsetMhz => "Read clock offset in MHz",
            Self::GetPstates => "Read NVML P-State clock ranges",
            Self::GetSupportedAppClocks => "Read NVML supported application clocks",
            Self::GetFanInfo => "Read NVML fan count and range",
            Self::GetTemperatureThresholds => "Read NVML temperature thresholds",
            Self::GetThrottleReasons => "Read NVML throttle reasons",
            Self::GetTdpTempLimits => "Read NVAPI TDP and temperature limits",
            Self::ProbeVoltageLimits => "Probe NVAPI voltage limit points",
            Self::CheckVoltageFrequency => "Check whether one VFP point is precise",
            Self::GetLegacyOvervoltRanges => "Read NVAPI legacy core overvolt ranges",
            Self::GetLegacyP0CoreMaxVoltageDelta => "Read NVAPI legacy P0 max voltage delta",
            Self::SetCoreOffsetMhz => "Set core clock offset in MHz",
            Self::SetMemoryOffsetMhz => "Set memory clock offset in MHz",
            Self::SetPowerWatt => "Set NVML power limit in watts",
            Self::SetPowerPercent => "Set NVAPI power limit in percent",
            Self::SetThermalLimitC => "Set thermal limit in Celsius",
            Self::SetFanPercent => "Set fan speed/cooler level in percent",
            Self::SetLockedClocksMhz => "Lock core or memory clocks to a MHz range",
            Self::SetVfpVoltageLock => "Lock VFP by point or voltage",
            Self::SetVfpPointDeltaMhz => "Set one VFP point delta in MHz",
            Self::SetVfpRangeDeltaMhz => "Set a VFP point range delta in MHz",
            Self::SetPstateLock => "Lock one NVML P-State or a contiguous range",
            Self::SetApplicationsClocksMhz => "Set NVML application clocks in MHz",
            Self::SetVoltageBoostPercent => "Set NVAPI voltage boost percent",
            Self::ResetCoreOffsetMhz => "Reset core clock offset to 0 MHz",
            Self::ResetMemoryOffsetMhz => "Reset memory clock offset to 0 MHz",
            Self::ResetApplicationsClocks => "Reset NVML application clocks",
            Self::ResetLockedClocks => "Reset core or memory locked clocks",
            Self::ResetFan => "Restore fan/cooler control",
            Self::ResetVfpDeltas => "Reset NVAPI VFP deltas",
            Self::ResetVfpLock => "Reset NVAPI VFP lock",
            Self::ResetPowerPercent => "Reset NVAPI power limits",
            Self::ResetThermalLimitC => "Reset NVAPI sensor limits",
            Self::ResetPstateBaseVoltages => "Reset NVAPI P-State base voltages",
        }
    }

    fn adapters(self) -> &'static [BackendAdapter] {
        match self {
            Self::ListGpus
            | Self::GetClockOffsetMhz
            | Self::SetCoreOffsetMhz
            | Self::SetMemoryOffsetMhz
            | Self::SetThermalLimitC
            | Self::SetFanPercent
            | Self::SetLockedClocksMhz
            | Self::SetPstateLock
            | Self::ResetCoreOffsetMhz
            | Self::ResetMemoryOffsetMhz
            | Self::ResetLockedClocks
            | Self::ResetFan => &BOTH_BACKENDS,
            Self::GetPowerWatt
            | Self::GetPstates
            | Self::GetSupportedAppClocks
            | Self::GetFanInfo
            | Self::GetTemperatureThresholds
            | Self::GetThrottleReasons
            | Self::SetPowerWatt
            | Self::SetApplicationsClocksMhz
            | Self::ResetApplicationsClocks => &NVML_ONLY,
            _ => &NVAPI_ONLY,
        }
    }

    fn arity(self) -> (usize, usize) {
        match self {
            Self::GetVfpPointVoltageMv | Self::CheckVoltageFrequency => (1, 1),
            Self::SetCoreOffsetMhz
            | Self::SetMemoryOffsetMhz
            | Self::SetPowerWatt
            | Self::SetPowerPercent
            | Self::SetThermalLimitC
            | Self::SetFanPercent
            | Self::SetVfpVoltageLock
            | Self::SetVoltageBoostPercent => (1, 1),
            Self::SetLockedClocksMhz
            | Self::SetVfpPointDeltaMhz
            | Self::SetApplicationsClocksMhz => (2, 2),
            Self::SetVfpRangeDeltaMhz => (3, 3),
            Self::SetPstateLock => (1, 2),
            _ => (0, 0),
        }
    }

    fn allowed_options(self) -> &'static [&'static str] {
        match self {
            Self::GetVfp => &[
                "domain",
                "indexed",
                "infer-missing-default",
                "no-infer-missing-default",
            ],
            Self::GetClockOffsetMhz => &["domain", "pstate"],
            Self::SetCoreOffsetMhz
            | Self::SetMemoryOffsetMhz
            | Self::ResetCoreOffsetMhz
            | Self::ResetMemoryOffsetMhz => &["pstate"],
            Self::SetFanPercent | Self::ResetFan => &["fan", "policy"],
            Self::SetLockedClocksMhz | Self::ResetLockedClocks | Self::ResetVfpDeltas => {
                &["domain"]
            }
            Self::SetVfpVoltageLock => &["feedback"],
            _ => &[],
        }
    }
}

const COMMANDS: &[Command] = &[
    Command::ListGpus,
    Command::GetInfo,
    Command::GetStatus,
    Command::GetSettings,
    Command::GetVfp,
    Command::GetVfpPointVoltageMv,
    Command::GetPowerWatt,
    Command::GetClockOffsetMhz,
    Command::GetPstates,
    Command::GetSupportedAppClocks,
    Command::GetFanInfo,
    Command::GetTemperatureThresholds,
    Command::GetThrottleReasons,
    Command::GetTdpTempLimits,
    Command::ProbeVoltageLimits,
    Command::CheckVoltageFrequency,
    Command::GetLegacyOvervoltRanges,
    Command::GetLegacyP0CoreMaxVoltageDelta,
    Command::SetCoreOffsetMhz,
    Command::SetMemoryOffsetMhz,
    Command::SetPowerWatt,
    Command::SetPowerPercent,
    Command::SetThermalLimitC,
    Command::SetFanPercent,
    Command::SetLockedClocksMhz,
    Command::SetVfpVoltageLock,
    Command::SetVfpPointDeltaMhz,
    Command::SetVfpRangeDeltaMhz,
    Command::SetPstateLock,
    Command::SetApplicationsClocksMhz,
    Command::SetVoltageBoostPercent,
    Command::ResetCoreOffsetMhz,
    Command::ResetMemoryOffsetMhz,
    Command::ResetApplicationsClocks,
    Command::ResetLockedClocks,
    Command::ResetFan,
    Command::ResetVfpDeltas,
    Command::ResetVfpLock,
    Command::ResetPowerPercent,
    Command::ResetThermalLimitC,
    Command::ResetPstateBaseVoltages,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub backend: BackendChoice,
    pub output: OutputFormat,
    pub no_color: bool,
    pub gpu_specs: Vec<String>,
    pub command: Option<Command>,
    pub positionals: Vec<String>,
    options: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct RunOutput {
    pub rendered: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
struct TargetResult {
    gpu_id: Option<u32>,
    backend: &'static str,
    ok: bool,
    output: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct Execution {
    function: &'static str,
    backend: String,
    warnings: Vec<String>,
    results: Vec<TargetResult>,
}

impl Execution {
    fn has_errors(&self) -> bool {
        self.results.iter().any(|result| !result.ok)
    }
}

pub fn parse_args<I, S>(args: I) -> CliResult<Invocation>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut argv = vec!["nvoc-cli".to_string()];
    argv.extend(args.into_iter().map(Into::into));

    let command_hint = command_hint_from_argv(&argv[1..]);
    let mut cli = cli_command(command_hint);
    if argv.iter().any(|arg| arg == "--no-color") || std::env::var_os("NO_COLOR").is_some() {
        cli = cli.color(ColorChoice::Never);
    }

    let matches = match cli.try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(err) => return Err(CliError::Clap(err)),
    };

    let (command_name, command_matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::new("missing function name"))?;
    let parsed_command = parse_command(command_name)?;
    let command = Some(parsed_command);
    let backend = if command_matches.get_flag("nvapi") {
        BackendChoice::Nvapi
    } else if command_matches.get_flag("nvml") {
        BackendChoice::Nvml
    } else {
        BackendChoice::Auto
    };
    let output = command_matches
        .get_one::<String>("output")
        .map_or(Ok(OutputFormat::Human), |raw| parse_output_format(raw))?;
    let no_color = command_matches.get_flag("no-color");
    let gpu_specs = command_matches
        .get_many::<String>("gpu")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let positionals = if parsed_command.arity().1 > 0 {
        command_matches
            .get_many::<String>("args")
            .map(|values| values.cloned().collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let options = collect_named_options(command_matches, parsed_command.allowed_options());

    let invocation = Invocation {
        backend,
        output,
        no_color,
        gpu_specs,
        command,
        positionals,
        options,
    };

    validate_invocation(&invocation)?;
    Ok(invocation)
}

fn validate_invocation(invocation: &Invocation) -> CliResult<()> {
    let command = invocation
        .command
        .ok_or_else(|| CliError::new("missing function name"))?;

    let supported = command.adapters();
    match invocation.backend {
        BackendChoice::Nvapi if !supported.contains(&BackendAdapter::Nvapi) => {
            return Err(CliError::new(format!(
                "{} does not support --nvapi",
                command.name()
            )));
        }
        BackendChoice::Nvml if !supported.contains(&BackendAdapter::Nvml) => {
            return Err(CliError::new(format!(
                "{} does not support --nvml",
                command.name()
            )));
        }
        _ => {}
    }

    let (min_args, max_args) = command.arity();
    if invocation.positionals.len() < min_args || invocation.positionals.len() > max_args {
        let expected = if min_args == max_args {
            min_args.to_string()
        } else {
            format!("{min_args}..={max_args}")
        };
        return Err(CliError::new(format!(
            "{} expects {expected} positional args, got {}",
            command.name(),
            invocation.positionals.len()
        )));
    }

    for option in invocation.options.keys() {
        if !command.allowed_options().contains(&option.as_str()) {
            return Err(CliError::new(format!(
                "--{option} is not valid for {}",
                command.name()
            )));
        }
    }

    Ok(())
}

fn parse_output_format(raw: &str) -> CliResult<OutputFormat> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        other => Err(CliError::new(format!(
            "invalid output format {other:?}; expected human or json"
        ))),
    }
}

fn parse_command(raw: &str) -> CliResult<Command> {
    COMMANDS
        .iter()
        .copied()
        .find(|command| command.name() == raw)
        .ok_or_else(|| CliError::new(format!("unknown function {raw:?}")))
}

fn command_hint_from_argv(argv: &[String]) -> Option<Command> {
    let mut index = 0;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" {
            return None;
        }
        if option_takes_value(token) {
            index += 2;
            continue;
        }
        if let Ok(command) = parse_command(token) {
            return Some(command);
        }
        index += 1;
    }
    None
}

fn option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-g" | "-O"
            | "--gpu"
            | "--output"
            | "--domain"
            | "--pstate"
            | "--fan"
            | "--policy"
            | "--infer-missing-default"
    )
}

fn cli_command(command_hint: Option<Command>) -> ClapCommand {
    let mut command = ClapCommand::new("nvoc-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Focused command-line wrapper for nvoc-core")
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("gpu")
                .short('g')
                .long("gpu")
                .value_name("GPU_ID")
                .action(ArgAction::Append)
                .global(true)
                .help("GPU selector; repeat for multiple GPUs"),
        )
        .arg(
            Arg::new("nvapi")
                .long("nvapi")
                .action(ArgAction::SetTrue)
                .conflicts_with("nvml")
                .global(true)
                .help("Force the NVAPI backend"),
        )
        .arg(
            Arg::new("nvml")
                .long("nvml")
                .action(ArgAction::SetTrue)
                .conflicts_with("nvapi")
                .global(true)
                .help("Force the NVML backend"),
        )
        .arg(
            Arg::new("output")
                .short('O')
                .long("output")
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .global(true)
                .help("Output format"),
        )
        .arg(
            Arg::new("no-color")
                .long("no-color")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Disable ANSI color output"),
        );

    if let Some(command_hint) = command_hint {
        for option in command_hint.allowed_options() {
            command = command.arg(command_specific_arg(option));
        }
    }

    for nvoc_command in COMMANDS {
        command = command.subcommand(clap_subcommand(*nvoc_command));
    }
    command
}

fn command_specific_arg(name: &'static str) -> Arg {
    match name {
        "domain" => Arg::new("domain")
            .long("domain")
            .value_name("DOMAIN")
            .action(ArgAction::Append)
            .global(true)
            .help("Clock/VFP domain: core, memory, processor, or video"),
        "pstate" => Arg::new("pstate")
            .long("pstate")
            .value_name("PSTATE")
            .action(ArgAction::Append)
            .global(true)
            .help("P-State such as P0 or P2"),
        "fan" => Arg::new("fan")
            .long("fan")
            .value_name("FAN")
            .action(ArgAction::Append)
            .global(true)
            .help("Fan/cooler target: all, 0, 1, or 2"),
        "policy" => Arg::new("policy")
            .long("policy")
            .value_name("POLICY")
            .action(ArgAction::Append)
            .global(true)
            .help("Fan policy such as manual or continuous"),
        "infer-missing-default" => Arg::new("infer-missing-default")
            .long("infer-missing-default")
            .value_name("BOOL")
            .action(ArgAction::Append)
            .global(true)
            .help("Infer missing default VFP values"),
        "indexed" => Arg::new("indexed")
            .long("indexed")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Preserve hardware VFP indices"),
        "no-infer-missing-default" => Arg::new("no-infer-missing-default")
            .long("no-infer-missing-default")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Do not infer missing default VFP values"),
        "feedback" => Arg::new("feedback")
            .long("feedback")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Enable feedback for VFP voltage lock"),
        _ => unreachable!("unknown command-specific option {name}"),
    }
}

fn clap_subcommand(command: Command) -> ClapCommand {
    let mut subcommand = ClapCommand::new(command.name()).about(command.about());
    let (min_args, max_args) = command.arity();
    if max_args > 0 {
        subcommand = subcommand.arg(
            Arg::new("args")
                .value_name("ARGS")
                .num_args(min_args..=max_args)
                .allow_hyphen_values(true),
        );
    }
    subcommand
}

fn collect_named_options(
    matches: &clap::ArgMatches,
    allowed_options: &[&'static str],
) -> BTreeMap<String, Vec<String>> {
    let mut options = BTreeMap::new();
    for name in allowed_options {
        match *name {
            "indexed" | "no-infer-missing-default" | "feedback" => {
                if matches.get_flag(name) {
                    options.insert(name.to_string(), vec!["true".to_string()]);
                }
            }
            _ => {
                if let Some(values) = matches.get_many::<String>(name) {
                    options.insert(name.to_string(), values.cloned().collect());
                }
            }
        }
    }
    options
}

pub fn run_invocation(invocation: &Invocation) -> CliResult<RunOutput> {
    let execution = execute(invocation)?;
    let rendered = match invocation.output {
        OutputFormat::Human => output::format_human(&execution),
        OutputFormat::Json => serde_json::to_string(&output::execution_to_json(&execution))?,
    };
    Ok(RunOutput {
        rendered,
        exit_code: i32::from(execution.has_errors()),
    })
}

fn execute(invocation: &Invocation) -> CliResult<Execution> {
    let command = invocation
        .command
        .ok_or_else(|| CliError::new("missing function name"))?;

    match invocation.backend {
        BackendChoice::Nvapi => execute_backend(invocation, command, BackendAdapter::Nvapi),
        BackendChoice::Nvml => execute_backend(invocation, command, BackendAdapter::Nvml),
        BackendChoice::Auto => execute_auto(invocation, command),
    }
}

fn execute_auto(invocation: &Invocation, command: Command) -> CliResult<Execution> {
    if command == Command::ListGpus {
        return execute_list_gpus_auto(invocation);
    }

    let supports_nvapi = command.adapters().contains(&BackendAdapter::Nvapi);
    let supports_nvml = command.adapters().contains(&BackendAdapter::Nvml);

    if supports_nvapi {
        let nvapi_attempt = execute_backend(invocation, command, BackendAdapter::Nvapi);
        match nvapi_attempt {
            Ok(execution) if !execution.has_errors() => return Ok(execution),
            Ok(nvapi_execution) if supports_nvml => {
                let mut nvml_execution =
                    execute_backend(invocation, command, BackendAdapter::Nvml)?;
                nvml_execution.warnings.insert(
                    0,
                    format!(
                        "NVAPI attempt for {} failed; fell back to NVML",
                        command.name()
                    ),
                );
                if nvml_execution.has_errors() {
                    nvml_execution.warnings.insert(
                        1,
                        format!(
                            "NVAPI result was also unsuccessful: {}",
                            summarize_errors(&nvapi_execution)
                        ),
                    );
                }
                return Ok(nvml_execution);
            }
            Ok(execution) => return Ok(execution),
            Err(nvapi_error) if supports_nvml => {
                let mut nvml_execution =
                    execute_backend(invocation, command, BackendAdapter::Nvml)?;
                nvml_execution.warnings.insert(
                    0,
                    format!("NVAPI attempt failed; fell back to NVML: {nvapi_error}"),
                );
                return Ok(nvml_execution);
            }
            Err(error) => return Err(error),
        }
    }

    if supports_nvml {
        return execute_backend(invocation, command, BackendAdapter::Nvml);
    }

    Err(CliError::new(format!(
        "{} has no runnable backend",
        command.name()
    )))
}

fn execute_backend(
    invocation: &Invocation,
    command: Command,
    adapter: BackendAdapter,
) -> CliResult<Execution> {
    let discovery = discovery_backend_set(command, adapter);
    let inventory = discover_targets(discovery)?;
    let all_targets = inventory.targets();

    if command == Command::ListGpus {
        return list_gpus_execution(
            command,
            adapter.label().to_string(),
            &all_targets,
            invocation,
            adapter,
        );
    }

    let selector = gpu_selector(invocation);
    let selected = select_targets(&all_targets, &selector)?;
    let filtered = selected
        .into_iter()
        .filter(|target| target_supports(*target, command, adapter))
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return Err(CliError::new(format!(
            "no selected GPUs expose the {} backend required by {}",
            adapter.label(),
            command.name()
        )));
    }

    let mut results = Vec::with_capacity(filtered.len());
    for target in filtered {
        let result = match execute_target(command, adapter, &target, invocation) {
            Ok(output) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: true,
                output: Some(output),
                error: None,
            },
            Err(error) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: false,
                output: None,
                error: Some(error.to_string()),
            },
        };
        results.push(result);
    }

    Ok(Execution {
        function: command.name(),
        backend: adapter.label().to_string(),
        warnings: Vec::new(),
        results,
    })
}

fn execute_list_gpus_auto(invocation: &Invocation) -> CliResult<Execution> {
    let inventory = discover_targets(BackendSet::Both)
        .or_else(|_| discover_targets(BackendSet::Nvapi))
        .or_else(|_| discover_targets(BackendSet::Nvml))?;
    let all_targets = inventory.targets();
    list_gpus_execution(
        Command::ListGpus,
        "auto".to_string(),
        &all_targets,
        invocation,
        BackendAdapter::Nvapi,
    )
}

fn list_gpus_execution(
    command: Command,
    backend: String,
    all_targets: &[GpuTarget<'_>],
    invocation: &Invocation,
    adapter_filter: BackendAdapter,
) -> CliResult<Execution> {
    let selector = gpu_selector(invocation);
    let selected = select_targets(all_targets, &selector)?;
    let explicit_backend = invocation.backend != BackendChoice::Auto;
    let mut results = Vec::new();

    for target in selected {
        if explicit_backend && !target_supports(target, command, adapter_filter) {
            continue;
        }

        let name = target.nvapi.and_then(|_| {
            run(&target, QueryGpuInfo)
                .ok()
                .map(|report| report.output.name)
        });

        results.push(TargetResult {
            gpu_id: Some(target.id.0),
            backend: if target.nvapi.is_some() && target.nvml.is_some() {
                "both"
            } else if target.nvapi.is_some() {
                "nvapi"
            } else {
                "nvml"
            },
            ok: true,
            output: Some(json!({
                "index": target.index,
                "gpu_id": target.id.0,
                "gpu_id_hex": format!("0x{:04X}", target.id.0),
                "pci_bus": target.id.pci_bus(),
                "backend_nvapi": target.nvapi.is_some(),
                "backend_nvml": target.nvml.is_some(),
                "name": name,
            })),
            error: None,
        });
    }

    if results.is_empty() {
        return Err(CliError::new("no GPUs matched the selector"));
    }

    Ok(Execution {
        function: command.name(),
        backend,
        warnings: Vec::new(),
        results,
    })
}

fn discovery_backend_set(command: Command, adapter: BackendAdapter) -> BackendSet {
    match (command, adapter) {
        (Command::SetPstateLock, BackendAdapter::Nvapi) => BackendSet::Both,
        (_, BackendAdapter::Nvapi) => BackendSet::Nvapi,
        (_, BackendAdapter::Nvml) => BackendSet::Nvml,
    }
}

fn gpu_selector(invocation: &Invocation) -> GpuSelector {
    if invocation.gpu_specs.is_empty() {
        GpuSelector::all()
    } else {
        GpuSelector::from_specs(invocation.gpu_specs.clone())
    }
}

fn target_supports(target: GpuTarget<'_>, command: Command, adapter: BackendAdapter) -> bool {
    match adapter {
        BackendAdapter::Nvapi => {
            target.nvapi.is_some() && (command != Command::SetPstateLock || target.nvml.is_some())
        }
        BackendAdapter::Nvml => target.nvml.is_some(),
    }
}

fn execute_target(
    command: Command,
    adapter: BackendAdapter,
    target: &GpuTarget<'_>,
    invocation: &Invocation,
) -> CliResult<Value> {
    match command {
        Command::ListGpus => unreachable!("list-gpus is handled before target execution"),
        Command::GetInfo => Ok(serde_json::to_value(run(target, QueryGpuInfo)?.output)?),
        Command::GetStatus => Ok(serde_json::to_value(run(target, QueryGpuStatus)?.output)?),
        Command::GetSettings => Ok(serde_json::to_value(run(target, QueryGpuSettings)?.output)?),
        Command::GetVfp => get_vfp(target, invocation),
        Command::GetVfpPointVoltageMv => {
            let point = parse_usize(&invocation.positionals[0], "point")?;
            let voltage = run(target, QueryVfpPointVoltage { point })?.output;
            Ok(json!({
                "point": point,
                "voltage_uv": voltage.0,
                "voltage_mv": voltage.0 as f64 / 1000.0,
            }))
        }
        Command::GetPowerWatt => {
            let power = run(target, QueryPowerLimits)?.output;
            Ok(json!({
                "min_watt": power.min_watts,
                "current_watt": power.current_watts,
                "max_watt": power.max_watts,
            }))
        }
        Command::GetClockOffsetMhz => get_clock_offset(target, adapter, invocation),
        Command::GetPstates => {
            let pstates = run(target, QueryPstates)?.output;
            Ok(Value::Array(
                pstates
                    .into_iter()
                    .map(|pstate| {
                        json!({
                            "pstate": nvml_pstate_to_str(pstate.pstate),
                            "min_core_mhz": pstate.min_core_mhz,
                            "max_core_mhz": pstate.max_core_mhz,
                            "min_memory_mhz": pstate.min_memory_mhz,
                            "max_memory_mhz": pstate.max_memory_mhz,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetSupportedAppClocks => {
            let clocks = run(target, QuerySupportedApplicationsClocks)?.output;
            Ok(Value::Array(
                clocks
                    .into_iter()
                    .map(|clock| {
                        json!({
                            "memory_mhz": clock.memory_mhz,
                            "graphics_mhz": clock.graphics_mhz,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetFanInfo => {
            let fan = run(target, QueryFanInfo)?.output;
            Ok(json!({
                "count": fan.count,
                "min_percent": fan.min_speed,
                "max_percent": fan.max_speed,
            }))
        }
        Command::GetTemperatureThresholds => {
            let thresholds = run(target, QueryTemperatureThresholds)?.output;
            Ok(Value::Array(
                thresholds
                    .into_iter()
                    .map(|item| json!({"name": item.name, "celsius": item.celsius}))
                    .collect(),
            ))
        }
        Command::GetThrottleReasons => {
            let reasons = run(target, QueryThrottleReasons)?.output;
            Ok(Value::Array(
                reasons
                    .into_iter()
                    .map(|item| json!({"name": item.name, "active": item.active}))
                    .collect(),
            ))
        }
        Command::GetTdpTempLimits => {
            let (min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp, curve) =
                run(target, QueryTdpTempLimits)?.output;
            Ok(json!({
                "min_tdp_percent": min_tdp.0,
                "default_tdp_percent": default_tdp.0,
                "max_tdp_percent": max_tdp.0,
                "min_temp_c": min_temp.0,
                "default_temp_c": default_temp.0,
                "max_temp_c": max_temp.0,
                "curve": format!("{curve:?}"),
            }))
        }
        Command::ProbeVoltageLimits => {
            let limits = run(target, ProbeVoltageLimits)?.output;
            Ok(json!({
                "lower_point": limits.lower_point,
                "upper_point": limits.upper_point,
            }))
        }
        Command::CheckVoltageFrequency => {
            let point = parse_usize(&invocation.positionals[0], "point")?;
            let check = run(target, CheckVoltageFrequency { point })?.output;
            Ok(json!({
                "point": point,
                "precise": check.precise,
                "matched_point": check.matched_point,
            }))
        }
        Command::GetLegacyOvervoltRanges => {
            let ranges = run(target, QueryLegacyCoreOvervoltRanges)?.output;
            Ok(Value::Array(
                ranges
                    .into_iter()
                    .map(|(pstate, min, current, max)| {
                        json!({
                            "pstate": pstate_label(pstate),
                            "min_uv": min.0,
                            "current_uv": current.0,
                            "max_uv": max.0,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetLegacyP0CoreMaxVoltageDelta => {
            let delta = run(target, QueryLegacyP0CoreMaxVoltageDelta)?.output;
            Ok(json!({"max_delta_uv": delta.map(|v| v.0)}))
        }
        Command::SetCoreOffsetMhz => {
            set_clock_offset(target, adapter, invocation, ClockDomain::Graphics)
        }
        Command::SetMemoryOffsetMhz => {
            set_clock_offset(target, adapter, invocation, ClockDomain::Memory)
        }
        Command::SetPowerWatt => {
            let watts = parse_u32_unit(&invocation.positionals[0], "w", "watt")?;
            run(target, SetPowerLimit { watts })?;
            Ok(json!({"applied": true, "power_watt": watts}))
        }
        Command::SetPowerPercent => {
            let percent = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
            run(
                target,
                SetNvapiPowerLimits {
                    limits: vec![Percentage(percent)],
                },
            )?;
            Ok(json!({"applied": true, "power_percent": percent}))
        }
        Command::SetThermalLimitC => {
            let celsius = parse_i32_unit(&invocation.positionals[0], "c", "celsius")?;
            match adapter {
                BackendAdapter::Nvapi => {
                    run(
                        target,
                        SetNvapiSensorLimits {
                            limits: vec![nvoc_core::Celsius(celsius).into()],
                        },
                    )?;
                }
                BackendAdapter::Nvml => {
                    run(target, SetTemperatureLimit { celsius })?;
                }
            }
            Ok(json!({"applied": true, "thermal_limit_c": celsius}))
        }
        Command::SetFanPercent => set_fan_percent(target, adapter, invocation),
        Command::SetLockedClocksMhz => set_locked_clocks(target, adapter, invocation),
        Command::SetVfpVoltageLock => {
            let voltage_target = parse_nvapi_locked_voltage_target(&invocation.positionals[0])?;
            run(
                target,
                SetVfpVoltageLock {
                    voltage_target,
                    feedback: option_bool(invocation, "feedback", false)?,
                },
            )?;
            Ok(json!({"applied": true, "target": invocation.positionals[0]}))
        }
        Command::SetVfpPointDeltaMhz => {
            let point = parse_usize(&invocation.positionals[0], "point")?;
            let mhz = parse_i32_unit(&invocation.positionals[1], "mhz", "mhz")?;
            run(
                target,
                SetVfpPointDelta {
                    point,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({"applied": true, "point": point, "delta_mhz": mhz}))
        }
        Command::SetVfpRangeDeltaMhz => {
            let start = parse_usize(&invocation.positionals[0], "start")?;
            let end = parse_usize(&invocation.positionals[1], "end")?;
            if start > end {
                return Err(CliError::new("start point must be <= end point"));
            }
            let mhz = parse_i32_unit(&invocation.positionals[2], "mhz", "mhz")?;
            run(
                target,
                SetVfpRangeDelta {
                    start,
                    end,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({"applied": true, "start": start, "end": end, "delta_mhz": mhz}))
        }
        Command::SetPstateLock => {
            let first = parse_nvml_pstate(&invocation.positionals[0])?;
            let second_raw = invocation
                .positionals
                .get(1)
                .map(String::as_str)
                .unwrap_or(&invocation.positionals[0]);
            let second = parse_nvml_pstate(second_raw)?;
            let (range, min_mhz, max_mhz) = match adapter {
                BackendAdapter::Nvapi => {
                    run(
                        target,
                        SetNvapiPstateLock {
                            first_pstate: first,
                            second_pstate: second,
                        },
                    )?
                    .output
                }
                BackendAdapter::Nvml => {
                    run(
                        target,
                        SetNvmlPstateLock {
                            first_pstate: first,
                            second_pstate: second,
                        },
                    )?
                    .output
                }
            };
            Ok(json!({
                "applied": true,
                "pstate_range": range,
                "min_lock_mhz": min_mhz,
                "max_lock_mhz": max_mhz,
            }))
        }
        Command::SetApplicationsClocksMhz => {
            let memory_mhz = parse_u32_unit(&invocation.positionals[0], "mhz", "mhz")?;
            let graphics_mhz = parse_u32_unit(&invocation.positionals[1], "mhz", "mhz")?;
            run(
                target,
                SetApplicationsClocks {
                    memory_mhz,
                    graphics_mhz,
                },
            )?;
            Ok(json!({
                "applied": true,
                "memory_mhz": memory_mhz,
                "graphics_mhz": graphics_mhz,
            }))
        }
        Command::SetVoltageBoostPercent => {
            let percent = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
            run(
                target,
                SetVoltageBoost {
                    boost: Percentage(percent),
                },
            )?;
            Ok(json!({"applied": true, "voltage_boost_percent": percent}))
        }
        Command::ResetCoreOffsetMhz => {
            reset_clock_offset(target, adapter, invocation, ClockDomain::Graphics)
        }
        Command::ResetMemoryOffsetMhz => {
            reset_clock_offset(target, adapter, invocation, ClockDomain::Memory)
        }
        Command::ResetApplicationsClocks => {
            run(target, ResetApplicationsClocks)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetLockedClocks => {
            let domain = option_domain(invocation, ClockDomain::Graphics)?;
            match adapter {
                BackendAdapter::Nvapi => {
                    run(target, ResetVfpFrequencyLock { domain })?;
                }
                BackendAdapter::Nvml => {
                    run(target, ResetLockedClocks { domain })?;
                }
            }
            Ok(json!({"applied": true, "domain": domain_label(domain)}))
        }
        Command::ResetFan => reset_fan(target, adapter, invocation),
        Command::ResetVfpDeltas => {
            let domain = option_vfp_reset_domain(invocation)?;
            run(target, ResetVfpDeltas { domain })?;
            Ok(json!({"applied": true, "domain": vfp_reset_domain_label(domain)}))
        }
        Command::ResetVfpLock => {
            run(target, ResetVfpLock)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetPowerPercent => {
            run(target, ResetNvapiPowerLimits)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetThermalLimitC => {
            run(target, ResetNvapiSensorLimits)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetPstateBaseVoltages => {
            run(target, ResetPstateBaseVoltages)?;
            Ok(json!({"applied": true}))
        }
    }
}

fn get_vfp(target: &GpuTarget<'_>, invocation: &Invocation) -> CliResult<Value> {
    let domain = option_domain(invocation, ClockDomain::Graphics)?;
    let indexed = option_bool(invocation, "indexed", true)?;
    let infer_missing_default = if option_bool(invocation, "no-infer-missing-default", false)? {
        false
    } else {
        option_bool(invocation, "infer-missing-default", true)?
    };
    let points = run(
        target,
        QueryDomainVfpPoints {
            domain,
            infer_missing_default,
            indexed,
        },
    )?
    .output;

    Ok(json!({
        "domain": domain_label(domain),
        "indexed": indexed,
        "infer_missing_default": infer_missing_default,
        "points": points
            .into_iter()
            .map(|(index, point)| {
                json!({
                    "index": index,
                    "voltage_uv": point.voltage.0,
                    "voltage_mv": point.voltage.0 as f64 / 1000.0,
                    "frequency_khz": point.frequency.0,
                    "frequency_mhz": point.frequency.0 as f64 / 1000.0,
                    "delta_khz": point.delta.0,
                    "delta_mhz": point.delta.0 as f64 / 1000.0,
                    "default_frequency_khz": point.default_frequency.0,
                    "default_frequency_mhz": point.default_frequency.0 as f64 / 1000.0,
                })
            })
            .collect::<Vec<_>>(),
    }))
}

fn get_clock_offset(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let domain = option_domain(invocation, ClockDomain::Graphics)?;
    match adapter {
        BackendAdapter::Nvapi => {
            let pstate = option_pstate_nvapi(invocation)?;
            let settings = run(target, QueryGpuSettings)?.output;
            let offset_khz = settings
                .pstate_deltas
                .get(&pstate)
                .and_then(|domains| domains.get(&domain))
                .map(|delta| delta.0)
                .unwrap_or(0);
            Ok(json!({
                "domain": domain_label(domain),
                "pstate": pstate_label(pstate),
                "offset_mhz": offset_khz as f64 / 1000.0,
                "offset_khz": offset_khz,
            }))
        }
        BackendAdapter::Nvml => {
            let pstate = option_pstate_nvml(invocation)?;
            let offset = run(target, QueryClockOffset { domain, pstate })?.output;
            Ok(json!({
                "domain": domain_label(domain),
                "pstate": nvml_pstate_to_str(pstate),
                "offset_mhz": offset.mhz,
            }))
        }
    }
}

fn set_clock_offset(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
    domain: ClockDomain,
) -> CliResult<Value> {
    let mhz = parse_i32_unit(&invocation.positionals[0], "mhz", "mhz")?;
    match adapter {
        BackendAdapter::Nvapi => {
            let pstate = option_pstate_nvapi(invocation)?;
            run(
                target,
                SetPstateClockOffset {
                    pstate,
                    domain,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({
                "applied": true,
                "backend": adapter.label(),
                "domain": domain_label(domain),
                "pstate": pstate_label(pstate),
                "offset_mhz": mhz,
            }))
        }
        BackendAdapter::Nvml => {
            let pstate = option_pstate_nvml(invocation)?;
            run(
                target,
                SetClockOffset {
                    domain,
                    pstate,
                    mhz,
                },
            )?;
            Ok(json!({
                "applied": true,
                "backend": adapter.label(),
                "domain": domain_label(domain),
                "pstate": nvml_pstate_to_str(pstate),
                "offset_mhz": mhz,
            }))
        }
    }
}

fn reset_clock_offset(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
    domain: ClockDomain,
) -> CliResult<Value> {
    match adapter {
        BackendAdapter::Nvapi => {
            let pstate = option_pstate_nvapi(invocation)?;
            run(
                target,
                ResetPstateClockOffsets {
                    offsets: vec![(pstate, domain)],
                },
            )?;
            Ok(json!({
                "applied": true,
                "domain": domain_label(domain),
                "pstate": pstate_label(pstate),
                "offset_mhz": 0,
            }))
        }
        BackendAdapter::Nvml => {
            let pstate = option_pstate_nvml(invocation)?;
            run(
                target,
                SetClockOffset {
                    domain,
                    pstate,
                    mhz: 0,
                },
            )?;
            Ok(json!({
                "applied": true,
                "domain": domain_label(domain),
                "pstate": nvml_pstate_to_str(pstate),
                "offset_mhz": 0,
            }))
        }
    }
}

fn set_fan_percent(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let level = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
    let fan = option_one(invocation, "fan").unwrap_or("all");
    let policy = option_one(invocation, "policy").unwrap_or("manual");

    match adapter {
        BackendAdapter::Nvapi => {
            let cooler_target = parse_cooler_target(fan)?;
            let policy = parse_nvapi_cooler_policy(policy)?;
            run(
                target,
                SetCoolerLevels {
                    policy,
                    level,
                    cooler_target,
                },
            )?;
            Ok(json!({
                "applied": true,
                "fan": fan,
                "policy": policy_label(policy),
                "level_percent": level,
            }))
        }
        BackendAdapter::Nvml => {
            let policy = parse_nvml_fan_control_policy(policy)?;
            let fan_indices = nvml_fan_indices(target, fan)?;
            for fan_index in &fan_indices {
                run(
                    target,
                    SetFanSpeed {
                        fan_index: *fan_index,
                        policy,
                        level,
                    },
                )?;
            }
            Ok(json!({
                "applied": true,
                "fan_indices": fan_indices,
                "policy": format!("{policy:?}"),
                "level_percent": level,
            }))
        }
    }
}

fn set_locked_clocks(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let min_mhz = parse_u32_unit(&invocation.positionals[0], "mhz", "mhz")?;
    let max_mhz = parse_u32_unit(&invocation.positionals[1], "mhz", "mhz")?;
    if min_mhz > max_mhz {
        return Err(CliError::new("min MHz must be <= max MHz"));
    }
    let domain = option_domain(invocation, ClockDomain::Graphics)?;

    match adapter {
        BackendAdapter::Nvapi => {
            run(
                target,
                SetVfpFrequencyLock {
                    domain,
                    upper: Kilohertz(max_mhz.saturating_mul(1000)),
                    lower: Some(Kilohertz(min_mhz.saturating_mul(1000))),
                },
            )?;
        }
        BackendAdapter::Nvml => {
            run(
                target,
                SetLockedClocks {
                    domain,
                    min_mhz,
                    max_mhz,
                },
            )?;
        }
    }

    Ok(json!({
        "applied": true,
        "domain": domain_label(domain),
        "min_mhz": min_mhz,
        "max_mhz": max_mhz,
    }))
}

fn reset_fan(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let fan = option_one(invocation, "fan").unwrap_or("all");
    match adapter {
        BackendAdapter::Nvapi => {
            run(target, ResetCoolerLevels)?;
            Ok(json!({"applied": true, "fan": fan}))
        }
        BackendAdapter::Nvml => {
            let fan_indices = nvml_fan_indices(target, fan)?;
            for fan_index in &fan_indices {
                run(
                    target,
                    ResetFanSpeed {
                        fan_index: *fan_index,
                    },
                )?;
            }
            Ok(json!({"applied": true, "fan_indices": fan_indices}))
        }
    }
}

fn nvml_fan_indices(target: &GpuTarget<'_>, raw: &str) -> CliResult<Vec<u32>> {
    if raw.eq_ignore_ascii_case("all") {
        let fan_count = run(target, QueryFanInfo)?.output.count;
        return Ok((0..fan_count).collect());
    }
    Ok(vec![parse_u32(raw, "fan")?])
}

fn option_one<'a>(invocation: &'a Invocation, name: &str) -> Option<&'a str> {
    invocation
        .options
        .get(name)
        .and_then(|values| values.last())
        .map(String::as_str)
}

fn option_bool(invocation: &Invocation, name: &str, default: bool) -> CliResult<bool> {
    option_one(invocation, name).map_or(Ok(default), parse_bool)
}

fn option_domain(invocation: &Invocation, default: ClockDomain) -> CliResult<ClockDomain> {
    option_one(invocation, "domain").map_or(Ok(default), parse_domain)
}

fn option_pstate_nvapi(invocation: &Invocation) -> CliResult<PState> {
    option_one(invocation, "pstate").map_or(Ok(PState::P0), parse_pstate_nvapi)
}

fn option_pstate_nvml(
    invocation: &Invocation,
) -> CliResult<nvml_wrapper::enum_wrappers::device::PerformanceState> {
    option_one(invocation, "pstate")
        .map_or_else(|| parse_nvml_pstate("P0"), parse_nvml_pstate)
        .map_err(CliError::from)
}

fn option_vfp_reset_domain(invocation: &Invocation) -> CliResult<VfpResetDomain> {
    match option_one(invocation, "domain")
        .unwrap_or("all")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "all" => Ok(VfpResetDomain::All),
        "core" | "graphics" | "gpu" => Ok(VfpResetDomain::Core),
        "mem" | "memory" => Ok(VfpResetDomain::Memory),
        other => Err(CliError::new(format!(
            "invalid VFP reset domain {other:?}; expected all, core, or memory"
        ))),
    }
}

fn parse_bool(raw: &str) -> CliResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(CliError::new(format!(
            "invalid bool {other:?}; expected on/off"
        ))),
    }
}

fn parse_domain(raw: &str) -> CliResult<ClockDomain> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "core" | "gpu" | "graphics" => Ok(ClockDomain::Graphics),
        "mem" | "memory" => Ok(ClockDomain::Memory),
        "processor" | "sm" => Ok(ClockDomain::Processor),
        "video" => Ok(ClockDomain::Video),
        other => Err(CliError::new(format!(
            "invalid domain {other:?}; expected core, memory, processor, or video"
        ))),
    }
}

fn parse_pstate_nvapi(raw: &str) -> CliResult<PState> {
    let normalized = raw.trim().to_ascii_uppercase();
    <PState as ConvertEnum>::from_str(&normalized).map_err(CliError::from)
}

fn parse_nvapi_cooler_policy(raw: &str) -> CliResult<CoolerPolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" | "continuous" => Ok(CoolerPolicy::TemperatureContinuous),
        "manual" => Ok(CoolerPolicy::Manual),
        other => <CoolerPolicy as ConvertEnum>::from_str(other).map_err(CliError::from),
    }
}

fn parse_cooler_target(raw: &str) -> CliResult<CoolerTarget> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(CoolerTarget::All),
        "0" | "1" => Ok(CoolerTarget::Cooler1),
        "2" => Ok(CoolerTarget::Cooler2),
        other => Err(CliError::new(format!(
            "invalid fan target {other:?}; expected all, 1, or 2"
        ))),
    }
}

fn parse_usize(raw: &str, label: &str) -> CliResult<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| CliError::new(format!("invalid {label} {raw:?}")))
}

fn parse_u32(raw: &str, label: &str) -> CliResult<u32> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| CliError::new(format!("invalid {label} {raw:?}")))
}

fn parse_i32_unit(raw: &str, suffix: &str, label: &str) -> CliResult<i32> {
    strip_unit(raw, suffix, label)
        .parse::<i32>()
        .map_err(|_| CliError::new(format!("invalid {label} value {raw:?}")))
}

fn parse_u32_unit(raw: &str, suffix: &str, label: &str) -> CliResult<u32> {
    strip_unit(raw, suffix, label)
        .parse::<u32>()
        .map_err(|_| CliError::new(format!("invalid {label} value {raw:?}")))
}

fn strip_unit<'a>(raw: &'a str, suffix: &str, label: &str) -> &'a str {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    for candidate in [suffix, label] {
        if let Some(without) = lower.strip_suffix(candidate) {
            return trimmed[..without.len()].trim();
        }
    }
    let plural_label = format!("{label}s");
    if let Some(without) = lower.strip_suffix(&plural_label) {
        return trimmed[..without.len()].trim();
    }
    trimmed
}

fn mhz_to_khz_i32(mhz: i32) -> CliResult<i32> {
    mhz.checked_mul(1000)
        .ok_or_else(|| CliError::new("MHz value is too large"))
}

fn domain_label(domain: ClockDomain) -> &'static str {
    match domain {
        ClockDomain::Graphics => "graphics",
        ClockDomain::Memory => "memory",
        ClockDomain::Processor => "processor",
        ClockDomain::Video => "video",
        _ => "unknown",
    }
}

fn pstate_label(pstate: PState) -> &'static str {
    <PState as ConvertEnum>::to_str(&pstate)
}

fn policy_label(policy: CoolerPolicy) -> &'static str {
    <CoolerPolicy as ConvertEnum>::to_str(&policy)
}

fn vfp_reset_domain_label(domain: VfpResetDomain) -> &'static str {
    match domain {
        VfpResetDomain::All => "all",
        VfpResetDomain::Core => "core",
        VfpResetDomain::Memory => "memory",
    }
}

fn summarize_errors(execution: &Execution) -> String {
    execution
        .results
        .iter()
        .filter_map(|result| result.error.as_ref())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_args_before_and_after_function() {
        let invocation = parse_args([
            "--domain",
            "memory",
            "get-vfp",
            "--gpu",
            "0",
            "--output=json",
            "--indexed",
        ])
        .unwrap();

        assert_eq!(invocation.command, Some(Command::GetVfp));
        assert_eq!(invocation.output, OutputFormat::Json);
        assert_eq!(invocation.gpu_specs, vec!["0"]);
        assert_eq!(option_one(&invocation, "domain"), Some("memory"));
        assert!(option_bool(&invocation, "indexed", false).unwrap());
    }

    #[test]
    fn parses_negative_positional_after_function() {
        let invocation = parse_args(["set-core-offset-mhz", "-100", "--nvml"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetCoreOffsetMhz));
        assert_eq!(invocation.backend, BackendChoice::Nvml);
        assert_eq!(invocation.positionals, vec!["-100"]);
    }

    #[test]
    fn parses_command_specific_named_args_before_function() {
        let invocation = parse_args(["--fan", "1", "set-fan-percent", "65"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetFanPercent));
        assert_eq!(invocation.positionals, vec!["65"]);
        assert_eq!(option_one(&invocation, "fan"), Some("1"));
    }

    #[test]
    fn rejects_backend_conflict() {
        let err = parse_args(["--nvapi", "--nvml", "list-gpus"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("conflicts"));
    }

    #[test]
    fn rejects_option_not_valid_for_command() {
        let err = parse_args(["get-power-watt", "--domain", "memory"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--domain"));
    }

    #[test]
    fn command_help_only_lists_supported_named_args() {
        let get_info_help = parse_args(["get-info", "--help"]).unwrap_err().to_string();
        assert!(!get_info_help.contains("--fan"));
        assert!(!get_info_help.contains("--domain"));

        let set_fan_help = parse_args(["set-fan-percent", "--help"])
            .unwrap_err()
            .to_string();
        assert!(set_fan_help.contains("--fan"));
        assert!(set_fan_help.contains("--policy"));
        assert!(!set_fan_help.contains("--domain"));
    }

    #[test]
    fn rejects_command_specific_named_args_on_other_commands() {
        let err = parse_args(["--fan", "1", "get-info"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--fan"));
    }

    #[test]
    fn parses_units() {
        assert_eq!(parse_i32_unit("-125MHz", "mhz", "mhz").unwrap(), -125);
        assert_eq!(parse_u32_unit("350W", "w", "watt").unwrap(), 350);
        assert_eq!(parse_u32_unit("350watts", "w", "watt").unwrap(), 350);
        assert_eq!(parse_u32_unit("90%", "%", "percent").unwrap(), 90);
        assert_eq!(parse_u32_unit("90percent", "%", "percent").unwrap(), 90);
        assert_eq!(parse_i32_unit("83celsius", "c", "celsius").unwrap(), 83);
        assert_eq!(mhz_to_khz_i32(150).unwrap(), 150_000);
    }

    #[test]
    fn parses_domain_aliases() {
        assert_eq!(parse_domain("core").unwrap(), ClockDomain::Graphics);
        assert_eq!(parse_domain("mem").unwrap(), ClockDomain::Memory);
    }

    #[test]
    fn command_backend_support_is_explicit() {
        assert!(
            Command::SetThermalLimitC
                .adapters()
                .contains(&BackendAdapter::Nvapi)
        );
        assert!(
            Command::SetThermalLimitC
                .adapters()
                .contains(&BackendAdapter::Nvml)
        );
        assert_eq!(Command::SetPowerWatt.adapters(), &NVML_ONLY);
        assert_eq!(Command::SetPowerPercent.adapters(), &NVAPI_ONLY);
    }
}
