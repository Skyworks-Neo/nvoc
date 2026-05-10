use crate::conv::ConvertEnum;
use crate::platform::{
    default_test_exe_path, default_vfp_csv_path, default_vfp_init_csv_path, default_vfp_log_path,
    default_vfp_temp_csv_path,
};
use crate::types::{
    OutputFormat, POSSIBLE_BOOL, POSSIBLE_BOOL_OFF, POSSIBLE_BOOL_ON, ResetSettings, VfpResetDomain,
};
use clap::{Arg, ArgAction, Command};
use nvapi_hi::PState;

pub fn get_arguments() -> Command {
    Command::new("nvoc-auto-optimizer")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Skyworks")
        .about("NVIDIA GPU VFP Curve Optimizer")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("gpu")
                .short('g')
                .long("gpu")
                .value_name("GPU_ID")
                .num_args(1..)
                .action(ArgAction::Append)
                .help(
                    "GPU ID selector. \
             Accepts decimal or hex.\n\
             Examples:\n\
               --gpu=2048\n\
               --gpu=0x0800\n\
               --gpu=256   (legacy, auto-mapped)\n\
             Can be specified multiple times.",
                ),
        )
        .arg(
            Arg::new("oformat")
                .short('O')
                .long("output-format")
                .value_name("OFORMAT")
                .num_args(1)
                .value_parser(OutputFormat::possible_values().to_vec())
                .default_value(OutputFormat::Human.to_str())
                .help("Data output format"),
        )
        .subcommand(
            Command::new("info")
                .about("Information about the model and capabilities of the GPU")
                .arg(
                    Arg::new("output")
                        .value_name("OUTPUT")
                        .short('o')
                        .long("output")
                        .num_args(1)
                        .help("Output file path for JSON format (optional)"),
                ),
        )
        .subcommand(Command::new("list"))
        .subcommand(
            Command::new("status")
                .about("Show current GPU usage, sensor, and clock information")
                .arg(
                    Arg::new("all")
                        .short('a')
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Show all available info"),
                )
                .arg(
                    Arg::new("status")
                        .short('s')
                        .long("status")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_ON)
                        .help("Show status info"),
                )
                .arg(
                    Arg::new("clocks")
                        .short('c')
                        .long("clocks")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_ON)
                        .help("Show clock frequency info"),
                )
                .arg(
                    Arg::new("coolers")
                        .short('C')
                        .long("coolers")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_OFF)
                        .default_value_if("all", clap::builder::ArgPredicate::IsPresent, POSSIBLE_BOOL_ON)
                        .help("Show cooler info"),
                )
                .arg(
                    Arg::new("sensors")
                        .short('S')
                        .long("sensors")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_OFF)
                        .default_value_if("all", clap::builder::ArgPredicate::IsPresent, POSSIBLE_BOOL_ON)
                        .help("Show thermal sensors"),
                )
                .arg(
                    Arg::new("vfp")
                        .short('v')
                        .long("vfp")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_OFF)
                        .default_value_if("all", clap::builder::ArgPredicate::IsPresent, POSSIBLE_BOOL_ON)
                        .help("Show voltage-frequency chart"),
                )
                .arg(
                    Arg::new("pstates")
                        .short('P')
                        .long("pstates")
                        .value_name("SHOW")
                        .num_args(1)
                        .value_parser(POSSIBLE_BOOL.to_vec())
                        .default_value(POSSIBLE_BOOL_OFF)
                        .default_value_if("all", clap::builder::ArgPredicate::IsPresent, POSSIBLE_BOOL_ON)
                        .help("Show power state configurations"),
                )
                .arg(
                    Arg::new("monitor")
                        .short('m')
                        .long("monitor")
                        .value_name("PERIOD")
                        .num_args(1)
                        .help("Monitor GPU status over time, optionally accepts period in seconds"),
                ),
        )
        .subcommand(
            Command::new("get")
                .about("Show GPU overclock settings"),
        )
        .subcommand(
            Command::new("reset")
                .about("Restore all overclocking settings")
                .subcommand(
                    Command::new("nvml-cooler")
                        .about("Restore NVML default fan control")
                        .arg(
                            Arg::new("id")
                                .short('i')
                                .long("id")
                                .value_name("COOLER_ID")
                                .num_args(1)
                                .value_parser(["1", "2", "all"])
                                .default_value("all")
                                .help("Target cooler ID: 1, 2, or all (default: all)"),
                        ),
                )
                .arg(
                    Arg::new("setting")
                        .value_name("SETTING")
                        .num_args(1..)
                        .value_parser(ResetSettings::possible_values().to_vec())
                        .help("Reset only the specified setting(s)"),
                )
                .arg(
                    Arg::new("domain")
                        .long("domain")
                        .value_name("DOMAIN")
                        .num_args(1..)
                        .action(ArgAction::Append)
                        .value_parser(ResetSettings::possible_values().to_vec())
                        .help("Optional alias of setting selector (e.g. --domain voltage-boost --domain power)"),
                )
                .arg(
                    Arg::new("vfp_domain")
                        .long("vfp-domain")
                        .value_name("VFP_DOMAIN")
                        .num_args(1)
                        .default_value(VfpResetDomain::All.to_str())
                        .value_parser(VfpResetDomain::possible_values().to_vec())
                        .help("When resetting vfp, choose all/core/memory"),
                ),
        )
        .subcommand(
            Command::new("set")
                .about("GPU overclocking")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("legacy-clock")
                        .about("Set absolute core/memory clocks for legacy architectures (Fermi/Tesla)")
                        .arg(
                            Arg::new("core")
                                .short('c')
                                .long("core")
                                .value_name("CORE_MHZ")
                                .help("Target absolute core frequency in MHz (1–5000)")
                                .num_args(1)
                                .required(true)
                                .value_parser(clap::value_parser!(u32).range(1..=5_000))
                        )
                        .arg(
                            Arg::new("memory")
                                .short('m')
                                .long("memory")
                                .value_name("MEM_MHZ")
                                .help("Target absolute memory frequency in MHz (1–5000)")
                                .num_args(1)
                                .required(true)
                                .value_parser(clap::value_parser!(u32).range(1..=5_000))
                        )
                )
                .subcommand(
                    Command::new("nvapi")
                        .about("NVAPI settings")
                        .arg_required_else_help(true)
                        .arg(
                            Arg::new("vboost")
                                .short('V')
                                .long("voltage-boost")
                                .value_name("VBOOST")
                                .num_args(1)
                                .value_parser(clap::value_parser!(u32).range(0..=200))
                                .help("Voltage Boost % (0–200)"),
                        )
                        .arg(
                            Arg::new("tlimit")
                                .short('T')
                                .long("thermal-limit")
                                .value_name("TEMPLIMIT")
                                .num_args(1..)
                                .action(ArgAction::Append)
                                .value_parser(clap::value_parser!(i32).range(0..=127))
                                .help("Thermal limit °C (0–127)"),
                        )
                        .arg(
                            Arg::new("plimit")
                                .short('P')
                                .long("power-limit")
                                .value_name("POWERLIMIT")
                                .num_args(1..)
                                .action(ArgAction::Append)
                                .value_parser(clap::value_parser!(u32).range(1..=10_000))
                                .help("Power limit % (1–10000)"),
                        )
                        .arg(
                            Arg::new("voltage_delta")
                                .short('U')
                                .long("voltage-delta")
                                .value_name("UV")
                                .num_args(1)
                                .allow_hyphen_values(true)
                                .value_parser(clap::value_parser!(i32).range(-500_000..=500_000))
                                .help("Core voltage delta in μV via SetPstates20 (±500 000 μV / ±500 mV). Target pstate selectable via -z."),
                        )
                        .arg(
                            Arg::new("pstate")
                                .short('z')
                                .long("pstate")
                                .value_name("PSTATE_ID")
                                .num_args(1)
                                .value_parser(PState::possible_values().to_vec())
                                .default_value(PState::P0.to_str())
                                .help("Target pstate for -U/--voltage-delta and NVAPI offsets (default: P0)"),
                        )
                        .arg(
                            Arg::new("core_offset")
                                .long("core-offset")
                                .value_name("CORE_OFFSET")
                                .num_args(1)
                                .allow_hyphen_values(true)
                                .value_parser(clap::value_parser!(i32).range(-5_000_000..=5_000_000))
                                .help("Core clock offset via NVAPI (kHz, ±5 000 000)."),
                        )
                        .arg(
                            Arg::new("mem_offset")
                                .long("mem-offset")
                                .value_name("MEM_OFFSET")
                                .num_args(1)
                                .allow_hyphen_values(true)
                                .value_parser(clap::value_parser!(i32).range(-5_000_000..=5_000_000))
                                .help("Memory clock offset via NVAPI (kHz, ±5 000 000)."),
                        )
                        .arg(
                            Arg::new("locked_voltage")
                                .long("locked-voltage")
                                .value_name("POINT_OR_VOLTAGE")
                                .num_args(1)
                                .help("Lock by VFP point index (e.g. 68) or explicit voltage unit (e.g. 850mV, 850000uV)."),
                        )
                        .arg(
                            Arg::new("locked_core_clocks")
                                .long("locked-core-clocks")
                                .value_names(["MIN_MHZ", "MAX_MHZ"])
                                .num_args(2)
                                .help("Lock NVAPI graphics clock range (MHz). Example: --nvapi-locked-core-clocks 210 2100")
                                .use_value_delimiter(false),
                        )
                        .arg(
                            Arg::new("locked_mem_clocks")
                                .long("locked-mem-clocks")
                                .value_names(["MIN_MHZ", "MAX_MHZ"])
                                .num_args(2)
                                .help("Lock NVAPI memory clock range (MHz). Example: --nvapi-locked-mem-clocks 5000 9501")
                                .use_value_delimiter(false),
                        )
                        .arg(
                            Arg::new("pstate_lock")
                                .long("pstate-lock")
                                .value_names(["PSTATE_MIN", "PSTATE_MAX"])
                                .num_args(1..=2)
                                .conflicts_with("locked_mem_clocks")
                                .help("Lock a GPU into one NVML P-State or contiguous NVML P-State range by applying the same derived memory window via NVAPI (for example: --pstate-lock 0, --pstate-lock P2 P2, or --pstate-lock P0 P5)."),
                        )
                        .arg(
                            Arg::new("test_limit")
                                .long("test-limit")
                                .action(ArgAction::SetTrue)
                                .help("Test the voltage limits of the GPU automatically."),
                        )
                        .arg(
                            Arg::new("reset_volt_locks")
                                .long("reset-volt-locks")
                                .action(ArgAction::SetTrue)
                                .help("Reset NVAPI Voltage lock state."),
                        )
                        .arg(
                            Arg::new("reset_core_clocks")
                                .long("reset-core-clocks")
                                .action(ArgAction::SetTrue)
                                .help("Reset GPU core clocks lock."),
                        )
                        .arg(
                            Arg::new("reset_mem_clocks")
                                .long("reset-mem-clocks")
                                .alias("pstate-unlock")
                                .action(ArgAction::SetTrue)
                                .help("Reset GPU memory clocks lock. Alias: --pstate-unlock."),
                        )
                )
                .subcommand(
                    Command::new("nvml")
                        .about("NVML settings")
                        .arg_required_else_help(true)
                        .arg(
                            Arg::new("pstate")
                                .long("pstate")
                                .value_name("PSTATE_ID")
                                .num_args(1)
                                .default_value("0")
                                .value_parser(|s: &str| -> Result<String, String> {
                                    let n = if s.starts_with(['P', 'p']) { &s[1..] } else { s };
                                    n.parse::<u32>()
                                        .ok()
                                        .filter(|&v| v <= 15)
                                        .map(|_| s.to_string())
                                        .ok_or_else(|| format!("P-state must be 0–15 or P0–P15, got '{s}'"))
                                })
                                .help("Target PState for NVML clock offset (0–15 or P0–P15, e.g. 0 or P0)."),
                        )
                        .arg(
                            Arg::new("core_offset")
                                .long("core-offset")
                                .value_name("CORE_OFFSET")
                                .num_args(1)
                                .allow_hyphen_values(true)
                                .value_parser(clap::value_parser!(i32).range(-5_000..=5_000))
                                .help("Core clock offset via NVML API (MHz, ±5000)."),
                        )
                        .arg(
                            Arg::new("mem_offset")
                                .long("mem-offset")
                                .value_name("MEM_OFFSET")
                                .num_args(1)
                                .allow_hyphen_values(true)
                                .value_parser(clap::value_parser!(i32).range(-5_000..=5_000))
                                .help("Memory clock offset via NVML API (MHz, ±5000). Note: target effective offset, handled behind the scene as *2."),
                        )
                        // NVML thermal threshold write args are intentionally commented out for now.
                        .arg(
                            Arg::new("power_limit")
                                .short('P')
                                .long("power-limit")
                                .value_name("POWER_LIMIT")
                                .num_args(1)
                                .value_parser(clap::value_parser!(u32).range(1..=3_000))
                                .help("Power limit via NVML API (W, 1–3000)."),
                        )
                        .arg(
                            Arg::new("locked_app_clocks")
                                .long("locked-app-clocks")
                                .alias("app-clock")
                                .value_names(["MEM_MHZ", "CORE_MHZ"])
                                .num_args(2)
                                .value_parser(clap::value_parser!(u32).range(1..=10_000))
                                .help("Set NVML applications clocks (memory and core freq in MHz, 1–10000). Example: --locked-app-clocks 5001 1500")
                                .use_value_delimiter(false),
                        )
                        .arg(
                            Arg::new("reset_app_clocks")
                                .long("reset-app-clocks")
                                .action(ArgAction::SetTrue)
                                .help("Reset NVML applications clocks to defaults."),
                        )
                        .arg(
                            Arg::new("locked_core_clocks")
                                .long("locked-core-clocks")
                                .value_names(["MIN_MHZ", "MAX_MHZ"])
                                .num_args(2)
                                .value_parser(clap::value_parser!(u32).range(1..=10_000))
                                .help("Lock GPU core clocks to a specific range (MHz, 1–10000). Example: --nvml-locked-gpu-clocks 210 2100")
                                .use_value_delimiter(false),
                        )
                        .arg(
                            Arg::new("reset_core_clocks")
                                .long("reset-core-clocks")
                                .action(ArgAction::SetTrue)
                                .help("Reset GPU core clocks lock."),
                        )
                        .arg(
                            Arg::new("locked_mem_clocks")
                                .long("locked-mem-clocks")
                                .value_names(["MIN_MHZ", "MAX_MHZ"])
                                .num_args(2)
                                .value_parser(clap::value_parser!(u32).range(1..=20_000))
                                .help("Lock GPU memory clocks to a specific range (MHz, 1–20000). Example: --nvml-locked-mem-clocks 5000 5000")
                                .use_value_delimiter(false),
                        )
                        .arg(
                            Arg::new("pstate_lock")
                                .long("pstate-lock")
                                .value_names(["PSTATE_MIN", "PSTATE_MAX"])
                                .num_args(1..=2)
                                .conflicts_with_all(["locked_mem_clocks", "reset_mem_clocks"])
                                .help("Lock a GPU into one NVML P-State or a contiguous NVML P-State range by applying a memory lock window derived from the selected P-State memory clocks (for example: --nvml-pstate-lock 0, --nvml-pstate-lock P2 P2, or --nvml-pstate-lock P0 P5)."),
                        )
                        .arg(
                            Arg::new("reset_mem_clocks")
                                .long("reset-mem-clocks")
                                .alias("nvml-pstate-unlock")
                                .action(ArgAction::SetTrue)
                                .help("Reset GPU memory clocks lock. Alias: --nvml-pstate-unlock."),
                        )
                )
                .subcommand(
                    Command::new("nvml-cooler")
                        .about("NVML fan and cooler controls")
                        .arg_required_else_help(true)
                        .arg(
                            Arg::new("id")
                                .short('i')
                                .long("id")
                                .value_name("COOLER_ID")
                                .num_args(1)
                                .value_parser(["1", "2", "all"])
                                .default_value("all")
                                .help("Target cooler ID: 1, 2, or all (default: all)"),
                        )
                        .arg(
                            Arg::new("policy")
                                .long("policy")
                                .value_name("MODE")
                                .num_args(1)
                                .required(true)
                                .help("Cooler policy (e.g. continuous/manual/auto)"),
                        )
                        .arg(
                            Arg::new("level")
                                .long("level")
                                .value_name("LEVEL")
                                .num_args(1)
                                .required(true)
                                .value_parser(clap::value_parser!(u32).range(0..=100))
                                .help("Cooler level % (0–100)"),
                        ),
                )
                .subcommand(
                    Command::new("nvapi-cooler")
                        .about("Fan and cooler controls")
                        .arg_required_else_help(true)
                        .arg(
                            Arg::new("id")
                                .short('i')
                                .long("id")
                                .value_name("COOLER_ID")
                                .num_args(1)
                                .value_parser(["1", "2", "all"])
                                .default_value("all")
                                .help("Target cooler ID: 1, 2, or all (default: all)"),
                        )
                        .arg(
                            Arg::new("policy")
                                .long("policy")
                                .value_name("MODE")
                                .num_args(1)
                                .required(true)
                                .help("Cooler policy (e.g. continuous/manual/auto)"),
                        )
                        .arg(
                            Arg::new("level")
                                .long("level")
                                .value_name("LEVEL")
                                .num_args(1)
                                .required(true)
                                .value_parser(clap::value_parser!(u32).range(0..=100))
                                .help("Cooler level % (0–100)"),
                        ),
                )
                .subcommand(
                    Command::new("vfp")
                        .about("GPU Boost 3.0 voltage-frequency curve")
                        .subcommand_required(true)
                        .arg_required_else_help(true)
                        .subcommand(
                            Command::new("export")
                                .about("Export current curve as CSV")
                                .arg(
                                    Arg::new("tabs")
                                        .short('t')
                                        .long("tabs")
                                        .action(ArgAction::SetTrue)
                                        .help("Separate columns using tabs"),
                                )
                                .arg(
                                    Arg::new("memory")
                                        .long("memory")
                                        .action(ArgAction::SetTrue)
                                        .help("Export memory VF table (default exports core/graphics VF table)"),
                                )
                                .arg(
                                    Arg::new("output")
                                        .value_name("OUTPUT")
                                        .num_args(1)
                                        .default_value("-")
                                        .help("Output file path"),
                                )
                                .arg(
                                    Arg::new("quick")
                                        .short('q')
                                        .long("quick")
                                        .action(ArgAction::SetTrue)
                                        .help("Do not export load curve, fast export"),
                                )
                                .arg(
                                    Arg::new("nocheck")
                                        .short('n')
                                        .long("nocheck")
                                        .action(ArgAction::SetTrue)
                                        .help("whether not to check dynamic result validity"),
                                )
                        )
                        .subcommand(
                            Command::new("export_log")
                                .about("Export current curve as CSV")
                                .arg(
                                    Arg::new("tabs")
                                        .short('t')
                                        .long("tabs")
                                        .action(ArgAction::SetTrue)
                                        .help("Separate columns using tabs"),
                                )
                                .arg(
                                    Arg::new("log")
                                        .value_name("LOG")
                                        .short('l')
                                        .num_args(1)
                                        .default_value(default_vfp_log_path())
                                        .help("input from a log file path"),
                                )
                                .arg(
                                    Arg::new("output")
                                        .value_name("OUTPUT")
                                        .num_args(1)
                                        .default_value("-")
                                        .help("Output file path"),
                                )
                        )
                        .subcommand(
                            Command::new("import")
                                .about("Import a modified curve from CSV")
                                .arg(
                                    Arg::new("tabs")
                                        .short('t')
                                        .long("tabs")
                                        .action(ArgAction::SetTrue)
                                        .help("Separate columns using tabs"),
                                )
                                .arg(
                                    Arg::new("input")
                                        .value_name("INPUT")
                                        .num_args(1)
                                        .default_value("-")
                                        .help("Input file path"),
                                )
                        )
                        .subcommand(
                            Command::new("single_point_adj")
                                .about("modify a single point")
                                .arg(
                                    Arg::new("point_start")
                                        .value_name("point_start")
                                        .short('s')
                                        .num_args(1)
                                        .default_value("40")
                                        .value_parser(clap::value_parser!(u32).range(0..=255))
                                        .help("VFP point index to adjust (0–255)"),
                                )
                                .arg(
                                    Arg::new("delta")
                                        .short('d')
                                        .value_name("DELTA")
                                        .num_args(1)
                                        .allow_hyphen_values(true)
                                        .required(true)
                                        .default_value("150000")
                                        .value_parser(clap::value_parser!(i32).range(-5_000_000..=5_000_000))
                                        .help("Clock delta / OC offset in kHz (±5 000 000)"),
                                ),
                        )
                        .subcommand(
                            Command::new("pointwiseoc")
                                .about("Apply a frequency delta to a range of VFP points (inclusive). \
                                    Example: pointwiseoc 39-76 +150000")
                                .arg(
                                    Arg::new("range")
                                        .value_name("RANGE")
                                        .num_args(1)
                                        .required(true)
                                        .help("Inclusive point range, e.g. 39-76"),
                                )
                                .arg(
                                    Arg::new("delta")
                                        .value_name("DELTA")
                                        .num_args(1)
                                        .required(true)
                                        .allow_hyphen_values(true)
                                        .value_parser(clap::value_parser!(i32).range(-5_000_000..=5_000_000))
                                        .help("Clock frequency delta in kHz (±5 000 000), e.g. +150000 or -50000"),
                                ),
                        )
                        .subcommand(
                            Command::new("fix_result")
                                .about("result fix")
                                .arg(
                                    Arg::new("delta_ref")
                                        .value_name("delta_ref")
                                        .short('d')
                                        .num_args(1)
                                        .default_value("3")
                                        .help("fix ref f"),
                                )
                                .arg(
                                    Arg::new("tempcsv")
                                        .value_name("TMPCSV")
                                        .short('v')
                                        .num_args(1)
                                        .default_value(default_vfp_temp_csv_path())
                                        .help("temporary vfcurve file path"),
                                )
                                .arg(
                                    Arg::new("outputcsv")
                                        .value_name("OUTPUTCSV")
                                        .short('o')
                                        .num_args(1)
                                        .default_value(default_vfp_csv_path())
                                        .help("output vfcurve file path"),
                                )
                                .arg(
                                    Arg::new("initcsv")
                                        .value_name("INITCSV")
                                        .short('i')
                                        .num_args(1)
                                        .default_value(default_vfp_init_csv_path())
                                        .help("reference init vfcurve file path"),
                                )
                                .arg(
                                    Arg::new("ultrafast")
                                        .short('u')
                                        .long("ultrafast")
                                        .action(ArgAction::SetTrue)
                                        .help("Enable ultrafast mode for maximum speed"),
                                )
                                .arg(
                                    Arg::new("vfplog")
                                        .value_name("VFPLOG")
                                        .short('l')
                                        .num_args(1)
                                        .default_value(default_vfp_log_path())
                                        .help("vfplog file path"),
                                )
                                .arg(
                                    Arg::new("minus_bin")
                                        .value_name("MINUS_BIN")
                                        .short('m')
                                        .num_args(1)
                                        .allow_hyphen_values(true)
                                        .value_parser(clap::value_parser!(i32).range(-50..=50))
                                        .help("Margin bin adjustment integer (±50)"),
                                ),
                        )
                        .subcommand(
                            Command::new("gen_seq")
                                .about("generate scan point sequence")
                                .arg(
                                    Arg::new("point_start")
                                        .value_name("point_start")
                                        .short('s')
                                        .num_args(1)
                                        .default_value("40")
                                        .value_parser(clap::value_parser!(u32).range(0..=120))
                                        .help("VFP point index to start (0–120)"),
                                )
                                .arg(
                                    Arg::new("interval_s")
                                        .value_name("interval_s")
                                        .short('a')
                                        .num_args(1)
                                        .default_value("2")
                                        .value_parser(clap::value_parser!(u32).range(1..=50))
                                        .help("small scan interval (1–50)"),
                                )
                                .arg(
                                    Arg::new("interval_m")
                                        .value_name("interval_m")
                                        .short('b')
                                        .num_args(1)
                                        .default_value("3")
                                        .value_parser(clap::value_parser!(u32).range(1..=50))
                                        .help("medium scan interval (1–50)"),
                                )
                                .arg(
                                    Arg::new("interval_l")
                                        .value_name("interval_l")
                                        .short('c')
                                        .num_args(1)
                                        .default_value("5")
                                        .value_parser(clap::value_parser!(u32).range(1..=50))
                                        .help("large scan interval (1–50)"),
                                )
                                .arg(
                                    Arg::new("wait_time_sec")
                                        .value_name("wait_time_sec")
                                        .short('w')
                                        .num_args(1)
                                        .default_value("5")
                                        .value_parser(clap::value_parser!(u32).range(1..=300))
                                        .help("voltage settle wait time in seconds (1–300)"),
                                )
                                .arg(
                                    Arg::new("try_point_start")
                                        .value_name("try_point_start")
                                        .short('t')
                                        .num_args(1)
                                        .default_value("60")
                                        .value_parser(clap::value_parser!(u32).range(0..=120))
                                        .help("VFP point index to start trying (0–120)"),
                                )
                                .arg(
                                    Arg::new("vfcsv")
                                        .value_name("VFCSV")
                                        .short('v')
                                        .num_args(1)
                                        .default_value(default_vfp_init_csv_path())
                                        .help("init vfcurve file path"),
                                ),
                        )
                        .subcommand(
                            Command::new("autoscan")
                                .about("auto-scanner for a new vfp")
                                .arg(
                                    Arg::new("ultrafast")
                                        .short('u')
                                        .long("ultrafast")
                                        .action(ArgAction::SetTrue)
                                        .help("Enable ultrafast mode for maximum speed"),
                                )
                                .arg(
                                    Arg::new("test_exe")
                                        .value_name("TEST_EXE")
                                        .short('w')
                                        .num_args(1)
                                        .default_value(default_test_exe_path())
                                        .help("test exe file path"),
                                )
                                .arg(
                                    Arg::new("log")
                                        .value_name("LOG")
                                        .short('l')
                                        .num_args(1)
                                        .default_value(default_vfp_log_path())
                                        .help("log file path"),
                                )
                                .arg(
                                    Arg::new("point_seq")
                                        .value_name("point_seq")
                                        .short('q')
                                        .num_args(1)
                                        .default_value("-")
                                        .help("Point seq to scan at"),
                                )
                                .arg(
                                    Arg::new("timeout_loops")
                                        .short('t')
                                        .value_name("timeout_loops")
                                        .num_args(1)
                                        .default_value("30")
                                        .value_parser(clap::value_parser!(u32).range(1..=1_000))
                                        .help("stress-test timeout loops (1–1000)"),
                                )
                                .arg(
                                    Arg::new("output")
                                        .value_name("OUTPUTCSV")
                                        .short('o')
                                        .num_args(1)
                                        .default_value(default_vfp_temp_csv_path())
                                        .help("output vfcurve file path with every-point save"),
                                )
                                .arg(
                                    Arg::new("Vmem_scan_switch")
                                        .short('m')
                                        .long("Vmem_scan_switch")
                                        .action(ArgAction::SetTrue)
                                        .help("Enables Vmem scan switch"),
                                )
                                .arg(
                                    Arg::new("initcsv")
                                        .value_name("INITCSV")
                                        .short('i')
                                        .num_args(1)
                                        .default_value(default_vfp_init_csv_path())
                                        .help("reference init vfcurve file path"),
                                )
                                .arg(
                                    Arg::new("bsod_recovery")
                                        .short('b')
                                        .long("recovery_method_switch")
                                        .help("Override recovery method switch: true or false")
                                        .num_args(1)
                                        .value_parser(["aggressive", "traditional"])
                                        .required(false),
                                )
                        )
                        .subcommand(
                            Command::new("autoscan_legacy")
                                .about("auto-scanner for legacy GPUs (Maxwell / pre-Pascal) using global pstate OC offset")
                                .arg(
                                    Arg::new("test_exe")
                                        .value_name("TEST_EXE")
                                        .short('w')
                                        .num_args(1)
                                        .default_value(default_test_exe_path())
                                        .help("test exe file path"),
                                )
                                .arg(
                                    Arg::new("log")
                                        .value_name("LOG")
                                        .short('l')
                                        .num_args(1)
                                        .default_value(default_vfp_log_path())
                                        .help("log file path"),
                                )
                                .arg(
                                    Arg::new("timeout_loops")
                                        .short('t')
                                        .value_name("timeout_loops")
                                        .num_args(1)
                                        .default_value("30")
                                        .value_parser(clap::value_parser!(u32).range(1..=1_000))
                                        .help("stress-test timeout loops (1–1000)"),
                                )
                                .arg(
                                    Arg::new("bsod_recovery")
                                        .short('b')
                                        .long("recovery_method_switch")
                                        .help("Override recovery method switch: aggressive or traditional")
                                        .num_args(1)
                                        .value_parser(["aggressive", "traditional"])
                                        .required(false),
                                ),
                        ),
                )
        )
}
