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
        OutputFormat::Human => format_human(&execution),
        OutputFormat::Json => serde_json::to_string_pretty(&execution_to_json(&execution))?,
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

fn execution_to_json(execution: &Execution) -> Value {
    json!({
        "function": execution.function,
        "backend": execution.backend,
        "ok": !execution.has_errors(),
        "warnings": execution.warnings,
        "results": execution.results.iter().map(|result| {
            json!({
                "gpu_id": result.gpu_id,
                "backend": result.backend,
                "ok": result.ok,
                "output": result.output,
                "error": result.error,
            })
        }).collect::<Vec<_>>(),
    })
}

fn format_human(execution: &Execution) -> String {
    let mut lines = Vec::new();
    lines.push(nvoc_cli_common::color::stylize_title(&format!(
        "{} via {}",
        execution.function, execution.backend
    )));

    for warning in &execution.warnings {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("Warning: {warning}"),
            true,
        ));
    }

    for result in &execution.results {
        let gpu = result
            .gpu_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        if result.ok {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("GPU {gpu} [{}]: ok", result.backend),
                false,
            ));
            if let Some(output) = &result.output {
                lines.extend(format_human_output(execution.function, output));
            }
        } else {
            let error = result.error.as_deref().unwrap_or("unknown error");
            lines.push(nvoc_cli_common::color::stylize(
                &format!("GPU {gpu} [{}]: error: {error}", result.backend),
                true,
            ));
        }
    }

    lines.join("\n")
}

fn format_human_output(function: &str, output: &Value) -> Vec<String> {
    match function {
        "get-settings" => format_get_settings_output(output),
        "get-vfp" => format_vfp_output(output),
        "get-pstates" => format_object_array(
            output,
            &[
                ("pstate", "P-State"),
                ("min_core_mhz", "Core Min"),
                ("max_core_mhz", "Core Max"),
                ("min_memory_mhz", "Memory Min"),
                ("max_memory_mhz", "Memory Max"),
            ],
        ),
        "get-supported-app-clocks" => format_object_array(
            output,
            &[("memory_mhz", "Memory"), ("graphics_mhz", "Graphics")],
        ),
        "get-temperature-thresholds" => {
            format_object_array(output, &[("name", "Threshold"), ("celsius", "Limit")])
        }
        "get-throttle-reasons" => {
            format_object_array(output, &[("name", "Reason"), ("active", "Active")])
        }
        "get-legacy-overvolt-ranges" => format_object_array(
            output,
            &[
                ("pstate", "P-State"),
                ("min_uv", "Min"),
                ("current_uv", "Current"),
                ("max_uv", "Max"),
            ],
        ),
        _ => format_value_block(output, 1),
    }
}

fn format_get_settings_output(output: &Value) -> Vec<String> {
    let Some(object) = output.as_object() else {
        return format_value_block(output, 1);
    };

    let mut lines = Vec::new();
    for (key, value) in sorted_object_entries(object) {
        if key == "vfp" {
            lines.extend(format_vfp_delta_summary(1, value));
            continue;
        }

        match value {
            Value::Object(child) if object_is_compact_scalar_group(child) => {
                lines.push(format_scalar_object_line(1, key, child, key));
            }
            Value::Object(child) if object_is_measurement_map(key, child) => {
                lines.push(format_measurement_map_line(1, key, child));
            }
            Value::Object(_) | Value::Array(_) => {
                lines.push(format!(
                    "{}{}",
                    indent_spaces(1),
                    nvoc_cli_common::color::stylize_title(&format_label(key))
                ));
                lines.extend(format_value_block_with_context(value, 2, key));
            }
            _ => lines.push(format_field_line(1, key, value)),
        }
    }
    lines
}

fn format_vfp_output(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(object) = output.as_object() {
        for key in ["domain", "indexed", "infer_missing_default"] {
            if let Some(value) = object.get(key) {
                lines.push(format_field_line(1, key, value));
            }
        }

        if let Some(points) = object.get("points").and_then(Value::as_array) {
            lines.push(format!(
                "  {}",
                nvoc_cli_common::color::stylize_title("V-F Points")
            ));
            for point in points {
                let index = field_text(point, "index");
                let voltage = field_text(point, "voltage_mv");
                let frequency = field_text(point, "frequency_mhz");
                let delta = field_text(point, "delta_mhz");
                let default_frequency = field_text(point, "default_frequency_mhz");
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    #{index}: {voltage}, {frequency}, delta {delta}, default {default_frequency}"
                    ),
                    false,
                ));
            }
        }
    } else {
        lines.extend(format_value_block(output, 1));
    }
    lines
}

fn format_object_array(output: &Value, fields: &[(&str, &str)]) -> Vec<String> {
    match output.as_array() {
        Some(items) if items.is_empty() => {
            vec![format!(
                "  {}",
                nvoc_cli_common::color::stylize("No entries", false)
            )]
        }
        Some(items) => items
            .iter()
            .map(|item| {
                let parts = fields
                    .iter()
                    .filter_map(|(key, label)| {
                        item.get(*key).map(|value| {
                            format!(
                                "{} {}",
                                nvoc_cli_common::color::stylize_title(label),
                                nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                format!("  {}", parts.join(" | "))
            })
            .collect(),
        None => format_value_block(output, 1),
    }
}

fn format_value_block(value: &Value, indent: usize) -> Vec<String> {
    format_value_block_with_context(value, indent, "")
}

fn format_value_block_with_context(value: &Value, indent: usize, context: &str) -> Vec<String> {
    match value {
        Value::Object(object) => {
            let compact_groups = compact_range_groups(object);
            let mut compacted_keys = compact_groups
                .iter()
                .flat_map(|group| group.keys.iter().copied())
                .collect::<Vec<_>>();
            let mut lines = compact_groups
                .iter()
                .map(|group| format_compact_group_line(indent, group))
                .collect::<Vec<_>>();

            for (key, value) in sorted_object_entries(object) {
                if compacted_keys.contains(&key.as_str()) {
                    continue;
                }

                match value {
                    Value::Object(child) if object_is_compact_scalar_group(child) => {
                        lines.push(format_scalar_object_line(
                            indent,
                            key,
                            child,
                            &join_context(context, key),
                        ));
                    }
                    Value::Object(child) if object_is_measurement_map(key, child) => {
                        lines.push(format_measurement_map_line(indent, key, child));
                    }
                    Value::Array(items) if key == "points" && array_is_pff_points(items) => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title("Points")
                        ));
                        lines.extend(format_pff_points(indent + 1, items));
                    }
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_value_block_with_context(
                            value,
                            indent + 1,
                            &join_context(context, key),
                        ));
                    }
                    _ => lines.push(format_field_line(indent, key, value)),
                }
            }

            compacted_keys.clear();
            lines
        }
        Value::Array(items) => {
            if items.is_empty() {
                return vec![format!(
                    "{}{}",
                    indent_spaces(indent),
                    nvoc_cli_common::color::stylize("No entries", false)
                )];
            }

            items
                .iter()
                .flat_map(|item| match item {
                    Value::Object(_) | Value::Array(_) => {
                        format_value_block_with_context(item, indent, context)
                    }
                    _ => vec![format!(
                        "{}- {}",
                        indent_spaces(indent),
                        nvoc_cli_common::color::stylize(&format_scalar("", item), false)
                    )],
                })
                .collect()
        }
        _ => vec![format!(
            "{}{}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize(&format_scalar("", value), false)
        )],
    }
}

fn join_context(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn sorted_object_entries<'a>(
    object: &'a serde_json::Map<String, Value>,
) -> Vec<(&'a String, &'a Value)> {
    let mut entries = object.iter().collect::<Vec<_>>();
    if entries.iter().all(|(key, _)| key.parse::<i64>().is_ok()) {
        entries.sort_by_key(|(key, _)| key.parse::<i64>().unwrap_or_default());
    }
    entries
}

struct CompactGroup<'a> {
    label_key: String,
    keys: Vec<&'a str>,
    values: Vec<(&'static str, &'a str, &'a Value)>,
}

fn compact_range_groups<'a>(object: &'a serde_json::Map<String, Value>) -> Vec<CompactGroup<'a>> {
    let mut groups: BTreeMap<String, CompactGroup<'a>> = BTreeMap::new();

    for (key, value) in object {
        if !is_scalar_value(value) {
            continue;
        }
        let Some((group_key, part_label)) = split_compact_range_key(key) else {
            continue;
        };
        let group = groups.entry(group_key.to_string()).or_insert(CompactGroup {
            label_key: strip_trailing_unit_key(group_key).to_string(),
            keys: Vec::new(),
            values: Vec::new(),
        });
        group.keys.push(key);
        group.values.push((part_label, key, value));
    }

    groups
        .into_values()
        .filter(|group| group.values.len() >= 2)
        .collect()
}

fn split_compact_range_key(key: &str) -> Option<(&str, &'static str)> {
    for (prefix, label) in [
        ("max_", "Max"),
        ("current_", "Current"),
        ("default_", "Default"),
        ("min_", "Min"),
    ] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return Some((rest, label));
        }
    }
    None
}

fn object_is_compact_scalar_group(object: &serde_json::Map<String, Value>) -> bool {
    let mut compact_count = 0;
    for (key, value) in object {
        if !is_scalar_value(value) {
            return false;
        }
        if compact_scalar_object_label(key).is_some() {
            compact_count += 1;
        }
    }
    compact_count >= 2 && compact_count == object.len()
}

fn object_is_measurement_map(key: &str, object: &serde_json::Map<String, Value>) -> bool {
    let context = key.to_ascii_lowercase();
    let is_measurement = context.contains("frequency")
        || context.contains("clock")
        || (context.contains("voltage") && !context.contains("domain"));
    is_measurement && object.len() >= 2 && object.values().all(is_scalar_value)
}

fn array_is_pff_points(items: &[Value]) -> bool {
    !items.is_empty()
        && items.iter().all(|item| {
            let Some(object) = item.as_object() else {
                return false;
            };
            object.len() == 2
                && object.get("x").and_then(Value::as_f64).is_some()
                && object.get("y").and_then(Value::as_f64).is_some()
        })
}

fn compact_scalar_object_label(key: &str) -> Option<&'static str> {
    match key {
        "max" | "maximum" => Some("Max"),
        "current" | "value" => Some("Current"),
        "default" => Some("Default"),
        "min" | "minimum" => Some("Min"),
        _ => None,
    }
}

fn format_compact_group_line(indent: usize, group: &CompactGroup<'_>) -> String {
    let values = ordered_compact_values(&group.values)
        .into_iter()
        .map(|(label, key, value)| {
            format!(
                "{label} {}",
                format_contextual_scalar(&group.label_key, key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(&group.label_key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

fn format_scalar_object_line(
    indent: usize,
    key: &str,
    object: &serde_json::Map<String, Value>,
    context: &str,
) -> String {
    let values = ordered_scalar_object_values(object)
        .into_iter()
        .map(|(label, field_key, value)| {
            format!(
                "{label} {}",
                format_contextual_scalar(context, field_key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

fn format_measurement_map_line(
    indent: usize,
    key: &str,
    object: &serde_json::Map<String, Value>,
) -> String {
    let values = object
        .iter()
        .map(|(field_key, value)| {
            format!(
                "{} {}",
                format_label(field_key),
                format_contextual_scalar(key, field_key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

fn format_pff_points(indent: usize, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let object = item.as_object()?;
            let raw_temp = object.get("x")?.as_f64()?;
            let raw_frequency = object.get("y")?.as_f64()?;
            Some(nvoc_cli_common::color::stylize(
                &format!(
                    "{}#{}: Temperature {} -> Frequency {}",
                    indent_spaces(indent),
                    index,
                    format_measurement(raw_temp / 256.0, "C"),
                    format_measurement(raw_frequency / 1000.0, "MHz")
                ),
                false,
            ))
        })
        .collect()
}

fn format_vfp_delta_summary(indent: usize, value: &Value) -> Vec<String> {
    let Some(object) = value.as_object() else {
        return format_value_block_with_context(value, indent, "vfp");
    };

    let mut lines = vec![format!(
        "{}{}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title("VFP Deltas")
    )];
    for domain in ["graphics", "memory"] {
        let Some(points) = object.get(domain).and_then(Value::as_object) else {
            continue;
        };
        lines.push(format_vfp_delta_domain_summary(indent + 1, domain, points));
    }
    lines
}

fn format_vfp_delta_domain_summary(
    indent: usize,
    domain: &str,
    points: &serde_json::Map<String, Value>,
) -> String {
    let entries = sorted_object_entries(points);
    let changed = entries
        .iter()
        .filter_map(|(point, value)| {
            let delta = value.as_f64()?;
            (delta != 0.0).then_some((point.as_str(), delta))
        })
        .collect::<Vec<_>>();

    let summary = if entries.is_empty() {
        "no points".to_string()
    } else if changed.is_empty() {
        format!("{} points, all 0 MHz", entries.len())
    } else {
        let preview = changed
            .iter()
            .take(12)
            .map(|(point, delta)| format!("#{point} {}", format_measurement(delta / 1000.0, "MHz")))
            .collect::<Vec<_>>()
            .join(", ");
        if changed.len() > 12 {
            format!(
                "{} points, {} changed: {preview}, ...",
                entries.len(),
                changed.len()
            )
        } else {
            format!(
                "{} points, {} changed: {preview}",
                entries.len(),
                changed.len()
            )
        }
    };

    nvoc_cli_common::color::stylize(
        &format!(
            "{}{}: {summary}",
            indent_spaces(indent),
            format_label(domain)
        ),
        false,
    )
}

fn ordered_compact_values<'a>(
    values: &[(&'static str, &'a str, &'a Value)],
) -> Vec<(&'static str, &'a str, &'a Value)> {
    ["Max", "Current", "Default", "Min"]
        .iter()
        .flat_map(|wanted| {
            values
                .iter()
                .filter(move |(label, _, _)| label == wanted)
                .copied()
        })
        .collect()
}

fn ordered_scalar_object_values<'a>(
    object: &'a serde_json::Map<String, Value>,
) -> Vec<(&'static str, &'a str, &'a Value)> {
    [
        "max", "maximum", "current", "value", "default", "min", "minimum",
    ]
    .iter()
    .filter_map(|key| {
        object.get_key_value(*key).and_then(|(field_key, value)| {
            compact_scalar_object_label(field_key).map(|label| (label, field_key.as_str(), value))
        })
    })
    .collect()
}

fn is_scalar_value(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

fn format_field_line(indent: usize, key: &str, value: &Value) -> String {
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
    )
}

fn field_text(object: &Value, key: &str) -> String {
    object
        .get(key)
        .map(|value| format_scalar(key, value))
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_scalar(key: &str, value: &Value) -> String {
    match value {
        Value::Null => "N/A".to_string(),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        Value::Number(number) => {
            let rendered = number.to_string();
            format_with_unit(key, &rendered)
        }
        Value::String(text) => {
            if text.is_empty() {
                "N/A".to_string()
            } else {
                format_with_unit(key, text)
            }
        }
        Value::Array(items) => items
            .iter()
            .map(|item| format_scalar(key, item))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => "see details".to_string(),
    }
}

fn format_contextual_scalar(context_key: &str, value_key: &str, value: &Value) -> String {
    let Some(number) = value.as_f64() else {
        return format_scalar(value_key, value);
    };
    let context = context_key.to_ascii_lowercase();
    if context.contains("frequency")
        || context.contains("clock")
        || (context.contains("vfp") && context.contains("range"))
    {
        return format_measurement(number / 1000.0, "MHz");
    }
    if context.contains("voltage") && !context.contains("domain") {
        return format_measurement(number / 1000.0, "mV");
    }
    format_scalar(value_key, value)
}

fn format_measurement(value: f64, unit: &str) -> String {
    let rendered = if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };
    format!("{rendered} {unit}")
}

fn format_with_unit(key: &str, rendered: &str) -> String {
    if key.ends_with("_mhz") {
        format!("{rendered} MHz")
    } else if key.ends_with("_khz") {
        format!("{rendered} kHz")
    } else if key.ends_with("_mv") {
        format!("{rendered} mV")
    } else if key.ends_with("_uv") {
        format!("{rendered} uV")
    } else if key.ends_with("_watt") {
        format!("{rendered} W")
    } else if key.ends_with("_percent") || key == "percent" {
        format!("{rendered}%")
    } else if key.ends_with("_c") || key == "celsius" {
        format!("{rendered} C")
    } else {
        rendered.to_string()
    }
}

fn strip_trailing_unit_key(key: &str) -> &str {
    for suffix in ["_mhz", "_khz", "_mv", "_uv", "_watt", "_percent", "_c"] {
        if let Some(stripped) = key.strip_suffix(suffix) {
            return stripped;
        }
    }
    key
}

fn format_label(key: &str) -> String {
    key.split('_')
        .map(|word| match word {
            "gpu" => "GPU".to_string(),
            "id" => "ID".to_string(),
            "pci" => "PCI".to_string(),
            "nvapi" => "NVAPI".to_string(),
            "nvml" => "NVML".to_string(),
            "tdp" => "TDP".to_string(),
            "vfp" => "VFP".to_string(),
            "uv" => "uV".to_string(),
            "mv" => "mV".to_string(),
            "mhz" => "MHz".to_string(),
            "khz" => "kHz".to_string(),
            "c" => "C".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn indent_spaces(indent: usize) -> String {
    "  ".repeat(indent)
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

    #[test]
    fn human_output_formats_objects_without_json_dump() {
        nvoc_cli_common::color::init(true);
        let execution = Execution {
            function: "get-power-watt",
            backend: "nvml".to_string(),
            warnings: Vec::new(),
            results: vec![TargetResult {
                gpu_id: Some(7),
                backend: "nvml",
                ok: true,
                output: Some(json!({
                    "min_watt": 100,
                    "current_watt": 250,
                    "max_watt": 350,
                })),
                error: None,
            }],
        };

        let rendered = format_human(&execution);

        assert!(rendered.contains("Watt: Max 350 W, Current 250 W, Min 100 W"));
        assert!(!rendered.contains('{'));
        assert!(!rendered.contains("\"current_watt\""));
    }

    #[test]
    fn human_output_formats_vfp_points_as_rows() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "domain": "graphics",
            "indexed": true,
            "infer_missing_default": true,
            "points": [
                {
                    "index": 12,
                    "voltage_mv": 900.0,
                    "frequency_mhz": 1800.0,
                    "delta_mhz": 15.0,
                    "default_frequency_mhz": 1785.0,
                }
            ],
        });

        let rendered = format_human_output("get-vfp", &output).join("\n");

        assert!(rendered.contains("V-F Points"));
        assert!(rendered.contains("#12: 900.0 mV, 1800.0 MHz, delta 15.0 MHz"));
        assert!(!rendered.contains("\"points\""));
    }

    #[test]
    fn human_output_compacts_range_fields() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "max_voltage_uv": 0,
            "min_voltage_uv": 0,
            "voltage": {
                "max": 0,
                "min": 0,
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(!rendered.contains("Max Voltage"));
        assert!(!rendered.contains("Min Voltage"));
    }

    #[test]
    fn human_output_adds_contextual_units_to_nested_ranges() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "graphics": {
                "frequency": {
                    "max": 2145000,
                    "min": 300000,
                },
                "frequency_delta": {
                    "max": 1000000,
                    "min": -1000000,
                },
                "voltage": {
                    "max": 0,
                    "min": 0,
                },
                "voltage_domain": "Undefined",
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Frequency: Max 2145 MHz, Min 300 MHz"));
        assert!(rendered.contains("Frequency Delta: Max 1000 MHz, Min -1000 MHz"));
        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(rendered.contains("Voltage Domain: Undefined"));
    }

    #[test]
    fn human_output_compacts_clock_maps_with_units() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "base_clocks": {
                "graphics": 1530000,
                "memory": 4001000,
            },
            "boost_clocks": {
                "graphics": 1830000,
                "memory": 4001000,
            },
            "bios_version": "90.16.34.00.60",
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Base Clocks: Graphics 1530 MHz, Memory 4001 MHz"));
        assert!(rendered.contains("Boost Clocks: Graphics 1830 MHz, Memory 4001 MHz"));
        assert!(rendered.contains("Bios Version: 90.16.34.00.60"));
    }

    #[test]
    fn human_output_labels_pff_throttle_curve_points() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "throttle_curve": {
                "points": [
                    {"x": 21248, "y": 1830000},
                    {"x": 22528, "y": 1830000},
                    {"x": 23040, "y": 1530000},
                ],
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("#0: Temperature 83 C -> Frequency 1830 MHz"));
        assert!(rendered.contains("#1: Temperature 88 C -> Frequency 1830 MHz"));
        assert!(rendered.contains("#2: Temperature 90 C -> Frequency 1530 MHz"));
        assert!(!rendered.contains("X:"));
        assert!(!rendered.contains("Y:"));
    }

    #[test]
    fn human_output_labels_vfp_limit_ranges_as_mhz_delta() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "vfp_limits": {
                "graphics": {
                    "range": {
                        "max": 500000,
                        "min": -500000,
                    },
                },
                "memory": {
                    "range": {
                        "max": 1500000,
                        "min": -500000,
                    },
                },
            },
            "virtual_frame_buffer": 6291456,
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Range: Max 500 MHz, Min -500 MHz"));
        assert!(rendered.contains("Range: Max 1500 MHz, Min -500 MHz"));
        assert!(rendered.contains("Virtual Frame Buffer: 6291456"));
    }

    #[test]
    fn human_output_summarizes_get_settings_vfp_deltas() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "vfp": {
                "graphics": {
                    "0": 0,
                    "1": 0,
                    "2": 15000,
                    "10": -30000,
                },
                "memory": {
                    "0": 0,
                    "1": 0,
                    "2": 0,
                },
            },
        });

        let rendered = format_human_output("get-settings", &output).join("\n");

        assert!(rendered.contains("VFP Deltas"));
        assert!(rendered.contains("Graphics: 4 points, 2 changed: #2 15 MHz, #10 -30 MHz"));
        assert!(rendered.contains("Memory: 3 points, all 0 MHz"));
        assert!(!rendered.contains("  10:"));
    }

    #[test]
    fn human_output_sorts_integer_keyed_maps_numerically() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "points": {
                "10": "ten",
                "2": "two",
                "1": "one",
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");
        let one = rendered.find("1: one").unwrap();
        let two = rendered.find("2: two").unwrap();
        let ten = rendered.find("10: ten").unwrap();

        assert!(one < two);
        assert!(two < ten);
    }

    #[test]
    fn human_output_renders_every_function_without_json_dump() {
        nvoc_cli_common::color::init(true);

        for command in COMMANDS {
            let rendered = format_human_output(command.name(), &sample_output(*command)).join("\n");
            assert!(
                !rendered.contains('{') && !rendered.contains('}') && !rendered.contains('"'),
                "{} still renders JSON-like output:\n{}",
                command.name(),
                rendered
            );
        }
    }

    fn sample_output(command: Command) -> Value {
        match command {
            Command::ListGpus => json!({
                "index": 0,
                "gpu_id": 1,
                "gpu_id_hex": "0x0001",
                "pci_bus": 1,
                "backend_nvapi": true,
                "backend_nvml": true,
                "name": "GPU",
            }),
            Command::GetInfo => json!({
                "name": "GPU",
                "architecture": "Ada",
                "driver_version": "555.0",
            }),
            Command::GetStatus => json!({
                "temperature_c": 65,
                "core_clock_mhz": 1800,
                "memory_clock_mhz": 10500,
            }),
            Command::GetSettings => json!({
                "power_percent": 100,
                "thermal_limit_c": 83,
                "voltage_boost_percent": 0,
            }),
            Command::GetVfp => json!({
                "domain": "graphics",
                "indexed": true,
                "infer_missing_default": true,
                "points": [{
                    "index": 0,
                    "voltage_mv": 800.0,
                    "frequency_mhz": 1500.0,
                    "delta_mhz": 0.0,
                    "default_frequency_mhz": 1500.0,
                }],
            }),
            Command::GetVfpPointVoltageMv => {
                json!({"point": 0, "voltage_uv": 800000, "voltage_mv": 800.0})
            }
            Command::GetPowerWatt => {
                json!({"min_watt": 100, "current_watt": 250, "max_watt": 350})
            }
            Command::GetClockOffsetMhz => {
                json!({"domain": "graphics", "pstate": "P0", "offset_mhz": 120})
            }
            Command::GetPstates => json!([{
                "pstate": "P0",
                "min_core_mhz": 300,
                "max_core_mhz": 2700,
                "min_memory_mhz": 405,
                "max_memory_mhz": 10500,
            }]),
            Command::GetSupportedAppClocks => {
                json!([{"memory_mhz": 10500, "graphics_mhz": 1800}])
            }
            Command::GetFanInfo => json!({"count": 2, "min_percent": 30, "max_percent": 100}),
            Command::GetTemperatureThresholds => {
                json!([{"name": "shutdown", "celsius": 95}])
            }
            Command::GetThrottleReasons => json!([{"name": "power", "active": false}]),
            Command::GetTdpTempLimits => json!({
                "min_tdp_percent": 50,
                "default_tdp_percent": 100,
                "max_tdp_percent": 120,
                "min_temp_c": 65,
                "default_temp_c": 83,
                "max_temp_c": 91,
                "curve": "Default",
            }),
            Command::ProbeVoltageLimits => json!({"lower_point": 0, "upper_point": 80}),
            Command::CheckVoltageFrequency => {
                json!({"point": 42, "precise": true, "matched_point": 42})
            }
            Command::GetLegacyOvervoltRanges => {
                json!([{"pstate": "P0", "min_uv": 0, "current_uv": 0, "max_uv": 100000}])
            }
            Command::GetLegacyP0CoreMaxVoltageDelta => json!({"max_delta_uv": 100000}),
            Command::SetCoreOffsetMhz | Command::SetMemoryOffsetMhz => json!({
                "applied": true,
                "backend": "nvapi",
                "domain": "graphics",
                "pstate": "P0",
                "offset_mhz": 120,
            }),
            Command::SetPowerWatt => json!({"applied": true, "power_watt": 250}),
            Command::SetPowerPercent => json!({"applied": true, "power_percent": 90}),
            Command::SetThermalLimitC => json!({"applied": true, "thermal_limit_c": 83}),
            Command::SetFanPercent => {
                json!({"applied": true, "fan": "all", "policy": "manual", "level_percent": 65})
            }
            Command::SetLockedClocksMhz => {
                json!({"applied": true, "domain": "graphics", "min_mhz": 1500, "max_mhz": 1800})
            }
            Command::SetVfpVoltageLock => json!({"applied": true, "target": "900mv"}),
            Command::SetVfpPointDeltaMhz => {
                json!({"applied": true, "point": 12, "delta_mhz": 15})
            }
            Command::SetVfpRangeDeltaMhz => {
                json!({"applied": true, "start": 12, "end": 16, "delta_mhz": 15})
            }
            Command::SetPstateLock => {
                json!({"applied": true, "pstate_range": "P0..P2", "min_lock_mhz": 300, "max_lock_mhz": 1800})
            }
            Command::SetApplicationsClocksMhz => {
                json!({"applied": true, "memory_mhz": 10500, "graphics_mhz": 1800})
            }
            Command::SetVoltageBoostPercent => {
                json!({"applied": true, "voltage_boost_percent": 25})
            }
            Command::ResetCoreOffsetMhz | Command::ResetMemoryOffsetMhz => json!({
                "applied": true,
                "domain": "graphics",
                "pstate": "P0",
                "offset_mhz": 0,
            }),
            Command::ResetApplicationsClocks
            | Command::ResetVfpLock
            | Command::ResetPowerPercent
            | Command::ResetThermalLimitC
            | Command::ResetPstateBaseVoltages => json!({"applied": true}),
            Command::ResetLockedClocks | Command::ResetVfpDeltas => {
                json!({"applied": true, "domain": "graphics"})
            }
            Command::ResetFan => json!({"applied": true, "fan_indices": [0, 1]}),
        }
    }
}
