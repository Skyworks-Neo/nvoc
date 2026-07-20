use clap::ArgMatches;
use nvoc_cli_common::color::stylize;
use nvoc_core::{
    CheckVoltageFrequency, ClockDomain, ConvertEnum, Error, GpuOperation, GpuTarget, Kilohertz,
    Microvolts, ProbeVoltageLimits, QueryTdpTempLimits, QueryVfpPointVoltage, SetVfpFrequencyLock,
    SetVfpVoltageLock, TdpTempLimits, VfpLockRequest, run,
};
use std::str::FromStr;
use time::{OffsetDateTime, format_description::parse_borrowed};

pub fn local_time_hms() -> String {
    let format = match parse_borrowed::<3>("[hour]:[minute]:[second]") {
        Ok(format) => format,
        Err(_) => return String::from("??:??:??"),
    };

    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());

    now.format(&format)
        .unwrap_or_else(|_| String::from("??:??:??"))
}

pub fn print_scan_separator() {
    println!(
        "{}",
        stylize(
            "================================================================================",
            false
        )
    );
}

fn run_output<O: GpuOperation>(gpu: &GpuTarget<'_>, op: O) -> Result<O::Output, Error> {
    run(gpu, op).map(|report| report.output)
}

#[derive(Clone, Debug)]
pub struct GpuVoltageLimits {
    pub gpu_id: u32,
    pub lower_point: usize,
    pub upper_point: usize,
}

#[derive(Clone, Debug)]
pub struct GpuVoltageFrequencyCheck {
    pub gpu_id: u32,
    pub precise: bool,
    pub matched_point: Option<usize>,
}

fn apply_vfp_lock(
    gpu: &GpuTarget<'_>,
    request: VfpLockRequest,
    feedback: bool,
) -> Result<(), Error> {
    match request {
        VfpLockRequest::VoltagePoint(point) => run_output(
            gpu,
            SetVfpVoltageLock {
                voltage_target: nvoc_core::NvapiLockedVoltageTarget::Point(point),
                feedback,
            },
        ),
        VfpLockRequest::Voltage(voltage) => run_output(
            gpu,
            SetVfpVoltageLock {
                voltage_target: nvoc_core::NvapiLockedVoltageTarget::Voltage(voltage),
                feedback,
            },
        ),
        VfpLockRequest::Frequency {
            domain,
            upper,
            lower,
        } => run_output(
            gpu,
            SetVfpFrequencyLock {
                domain,
                upper,
                lower,
            },
        ),
    }
}

fn parse_clock_domain(raw: Option<&String>) -> Result<ClockDomain, Error> {
    match raw.map(|s| s.as_str()).unwrap_or("Graphics") {
        "Graphics" => Ok(ClockDomain::Graphics),
        "Memory" => Ok(ClockDomain::Memory),
        other => ClockDomain::from_str(other)
            .map_err(|e| Error::from(format!("Invalid --domain value '{}': {}", other, e))),
    }
}

fn parse_lock_frequency(
    matches: &ArgMatches,
) -> Result<(ClockDomain, Kilohertz, Option<Kilohertz>), Error> {
    let raw_targets = matches
        .get_many::<String>("clock")
        .ok_or_else(|| Error::from("Missing --clock <UPPER_MHZ> [LOWER_MHZ] value"))?
        .map(|s| s.as_str())
        .collect::<Vec<_>>();

    let upper_mhz = raw_targets[0]
        .parse::<u32>()
        .map_err(|_| Error::from("In --clock mode, UPPER_MHZ must be an integer MHz value"))?;

    let lower_mhz =
        if raw_targets.len() >= 2 {
            Some(raw_targets[1].parse::<u32>().map_err(|_| {
                Error::from("In --clock mode, LOWER_MHZ must be an integer MHz value")
            })?)
        } else {
            None
        };

    if let Some(lower) = lower_mhz
        && lower > upper_mhz
    {
        return Err(Error::from(
            "--clock expects upper bound first and lower bound second",
        ));
    }

    Ok((
        parse_clock_domain(matches.get_one::<String>("domain"))?,
        Kilohertz(upper_mhz.saturating_mul(1000)),
        lower_mhz.map(|v| Kilohertz(v.saturating_mul(1000))),
    ))
}

fn parse_lock_voltage(
    gpu: &GpuTarget<'_>,
    matches: &ArgMatches,
    default_point: usize,
) -> Result<VfpLockRequest, Error> {
    let raw_target = matches
        .get_one::<String>("point")
        .map(|s| s.as_str())
        .unwrap_or("");

    if matches
        .try_get_one::<bool>("voltage")
        .is_ok_and(|v| v.copied().unwrap_or(false))
    {
        const MIN_LOCK_UV: u32 = 500_000;
        const MAX_LOCK_UV: u32 = 2_000_000;

        if raw_target.is_empty() {
            return Err(Error::from("--voltage requires --point <VOLTAGE_MV_OR_UV>"));
        }

        let input_voltage = raw_target.parse::<u32>().map_err(|_| {
            Error::from("--voltage --point value must be an integer mV or uV value")
        })?;
        let voltage_uv = if input_voltage >= 10_000 {
            input_voltage
        } else {
            input_voltage.saturating_mul(1000)
        };

        if !(MIN_LOCK_UV..=MAX_LOCK_UV).contains(&voltage_uv) {
            return Err(Error::from(format!(
                "--voltage {} uV is outside the supported range {}-{} uV (0.5-2.0 V)",
                voltage_uv, MIN_LOCK_UV, MAX_LOCK_UV
            )));
        }

        Ok(VfpLockRequest::Voltage(Microvolts(voltage_uv)))
    } else {
        let point = raw_target.parse::<usize>().unwrap_or(default_point);
        run_output(gpu, QueryVfpPointVoltage { point })?;
        Ok(VfpLockRequest::VoltagePoint(point))
    }
}

fn parse_nvapi_locked_clock_range(
    matches: &ArgMatches,
    key: &str,
) -> Result<Option<(u32, u32)>, Error> {
    let Some(raw) = matches.get_many::<String>(key) else {
        return Ok(None);
    };

    let (invalid_msg, count_msg, order_msg) = if key == "locked_core_clocks" {
        (
            "Invalid --locked-core-clocks value: expected integer MHz",
            "Invalid arguments for --locked-core-clocks, expected 2 values (MIN_MHZ MAX_MHZ)",
            "--locked-core-clocks expects MIN_MHZ <= MAX_MHZ",
        )
    } else {
        (
            "Invalid --locked-mem-clocks value: expected integer MHz",
            "Invalid arguments for --locked-mem-clocks, expected 2 values (MIN_MHZ MAX_MHZ)",
            "--locked-mem-clocks expects MIN_MHZ <= MAX_MHZ",
        )
    };

    let clocks = raw
        .map(|s| u32::from_str(s.as_str()).map_err(|_| Error::from(invalid_msg)))
        .collect::<Result<Vec<_>, _>>()?;

    if clocks.len() != 2 {
        return Err(Error::from(count_msg));
    }

    let min_clock = clocks[0];
    let max_clock = clocks[1];
    if min_clock > max_clock {
        return Err(Error::from(order_msg));
    }

    Ok(Some((min_clock, max_clock)))
}

pub fn handle_lock_vfp(
    gpus: &[GpuTarget<'_>],
    matches: &ArgMatches,
    default_point: usize,
    feedback_flag: bool,
) -> Result<(), Error> {
    if let Some(locked_voltage_raw) = matches.get_one::<String>("locked_voltage") {
        let target = nvoc_core::parse_nvapi_locked_voltage_target(locked_voltage_raw.as_str())?;
        for gpu in gpus {
            let request = match target {
                nvoc_core::NvapiLockedVoltageTarget::Point(point) => {
                    VfpLockRequest::VoltagePoint(point)
                }
                nvoc_core::NvapiLockedVoltageTarget::Voltage(v) => VfpLockRequest::Voltage(v),
            };
            apply_vfp_lock(gpu, request, feedback_flag)?;
        }
        return Ok(());
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_core_clocks")?
    {
        for gpu in gpus {
            run_output(
                gpu,
                SetVfpFrequencyLock {
                    domain: ClockDomain::Graphics,
                    upper: Kilohertz(max_clock.saturating_mul(1000)),
                    lower: Some(Kilohertz(min_clock.saturating_mul(1000))),
                },
            )?;
        }
        return Ok(());
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_mem_clocks")?
    {
        for gpu in gpus {
            run_output(
                gpu,
                SetVfpFrequencyLock {
                    domain: ClockDomain::Memory,
                    upper: Kilohertz(max_clock.saturating_mul(1000)),
                    lower: Some(Kilohertz(min_clock.saturating_mul(1000))),
                },
            )?;
        }
        return Ok(());
    }

    if matches.get_one::<String>("clock").is_some() {
        if matches
            .try_get_one::<bool>("voltage")
            .is_ok_and(|v| v.copied().unwrap_or(false))
        {
            return Err(Error::from("Cannot use --clock and --voltage together"));
        }

        let (domain, upper, lower) = parse_lock_frequency(matches)?;
        for gpu in gpus {
            apply_vfp_lock(
                gpu,
                VfpLockRequest::Frequency {
                    domain,
                    upper,
                    lower,
                },
                feedback_flag,
            )?;
        }
        return Ok(());
    }

    let request = parse_lock_voltage(
        gpus.first().ok_or_else(|| Error::from("no GPU selected"))?,
        matches,
        default_point,
    )?;
    for gpu in gpus {
        apply_vfp_lock(gpu, request, feedback_flag)?;
    }
    Ok(())
}

pub fn handle_test_voltage_limits(
    gpus: &[GpuTarget<'_>],
    _matches: &ArgMatches,
    mut print_separator: impl FnMut(),
) -> Result<Vec<GpuVoltageLimits>, Error> {
    if gpus.is_empty() {
        return Err(Error::from("no GPU selected"));
    }

    print_separator();
    gpus.iter()
        .map(|gpu| {
            let limits = run_output(gpu, ProbeVoltageLimits)?;
            Ok(GpuVoltageLimits {
                gpu_id: gpu.id.0,
                lower_point: limits.lower_point,
                upper_point: limits.upper_point,
            })
        })
        .collect()
}

pub fn voltage_frequency_check(
    gpus: &[GpuTarget<'_>],
    point: usize,
) -> Result<Vec<GpuVoltageFrequencyCheck>, Error> {
    if gpus.is_empty() {
        return Err(Error::from("no GPU selected"));
    }

    gpus.iter()
        .map(|gpu| {
            run_output(gpu, CheckVoltageFrequency { point }).map(|check| GpuVoltageFrequencyCheck {
                gpu_id: gpu.id.0,
                precise: check.precise,
                matched_point: check.matched_point,
            })
        })
        .collect()
}

pub fn get_gpu_tdp_temp_limit(matches: &ArgMatches) -> Result<TdpTempLimits, Error> {
    let selector = match matches.get_many::<String>("gpu") {
        Some(values) => nvoc_core::GpuSelector::from_specs(values.cloned()),
        None => nvoc_core::GpuSelector::all(),
    };
    let inventory = nvoc_core::discover_targets(nvoc_core::BackendSet::Nvapi)?;
    let all_targets = inventory.targets();
    let gpus = nvoc_core::select_targets(&all_targets, &selector)?;
    let gpu = gpus.first().ok_or_else(|| Error::from("no GPU selected"))?;
    run_output(gpu, QueryTdpTempLimits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, ArgAction, Command};

    fn lock_matches(args: &[&str]) -> ArgMatches {
        Command::new("test")
            .arg(
                Arg::new("voltage")
                    .long("voltage")
                    .action(ArgAction::SetTrue),
            )
            .arg(Arg::new("point").long("point").num_args(1))
            .arg(
                Arg::new("locked_core_clocks")
                    .long("locked-core-clocks")
                    .num_args(2),
            )
            .arg(
                Arg::new("locked_mem_clocks")
                    .long("locked-mem-clocks")
                    .num_args(2),
            )
            .try_get_matches_from(args)
            .expect("valid test args")
    }

    #[test]
    fn voltage_mode_requires_explicit_point_value() {
        let matches = lock_matches(&["test", "--voltage"]);
        let gpu = GpuTarget::without_backends(nvoc_core::GpuId(0), 0);

        let error = parse_lock_voltage(&gpu, &matches, 42).expect_err("missing point errors");

        assert_eq!(
            error.to_string(),
            "--voltage requires --point <VOLTAGE_MV_OR_UV>"
        );
    }

    #[test]
    fn locked_clock_range_errors_use_actual_flag_names() {
        let matches = lock_matches(&["test", "--locked-core-clocks", "2000", "1000"]);
        let error = parse_nvapi_locked_clock_range(&matches, "locked_core_clocks")
            .expect_err("reversed clocks error");
        assert_eq!(
            error.to_string(),
            "--locked-core-clocks expects MIN_MHZ <= MAX_MHZ"
        );

        let matches = lock_matches(&["test", "--locked-mem-clocks", "2000", "1000"]);
        let error = parse_nvapi_locked_clock_range(&matches, "locked_mem_clocks")
            .expect_err("reversed clocks error");
        assert_eq!(
            error.to_string(),
            "--locked-mem-clocks expects MIN_MHZ <= MAX_MHZ"
        );
    }
}
