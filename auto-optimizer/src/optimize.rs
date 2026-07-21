use crate::arg_help;
use crate::oc_profile_function::{fix_result, handle_vfp_export, handle_vfp_import};
use crate::oc_scanner::{autoscan_gpuboostv3, autoscan_legacy};
use clap::ArgMatches;
use nvoc_core::{
    Error, GpuTarget, QueryGpuInfo, ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpLock,
    VfpResetDomain, run,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};

fn error(message: impl Into<String>) -> Error {
    Error::Custom(message.into())
}

fn parse_subcommand(args: Vec<String>, expected: &str) -> Result<ArgMatches, Error> {
    let matches = arg_help::get_arguments()
        .try_get_matches_from(args)
        .map_err(|e| error(e.to_string()))?;
    let (name, matches) = matches
        .subcommand()
        .ok_or_else(|| error("internal workflow command is missing a subcommand"))?;
    if name != expected {
        return Err(error(format!(
            "internal workflow expected {expected}, got {name}"
        )));
    }
    Ok(matches.clone())
}

fn select_target<'a>(
    targets: &'a [GpuTarget<'a>],
    matches: &ArgMatches,
) -> Result<&'a GpuTarget<'a>, Error> {
    if targets.is_empty() {
        return Err(error("no NvAPI GPU is available for optimization"));
    }

    let explicit_gpu_count = matches
        .get_many::<String>("gpu")
        .map(|values| values.count())
        .unwrap_or(0);
    if explicit_gpu_count > 0 {
        return match targets {
            [target] => Ok(target),
            _ => Err(error("optimize accepts exactly one --gpu selector")),
        };
    }
    if targets.len() == 1 {
        return Ok(&targets[0]);
    }
    if !io::stdin().is_terminal() {
        return Err(error(
            "multiple GPUs are available; pass exactly one --gpu in non-interactive mode",
        ));
    }

    eprintln!("Available GPUs:");
    for (index, target) in targets.iter().enumerate() {
        match run(target, QueryGpuInfo) {
            Ok(report) => eprintln!(
                "  {index}: GPU ID {} — {} ({})",
                target.id.0,
                report.output.name,
                report.output.uuid.as_deref().unwrap_or("UUID unavailable")
            ),
            Err(_) => eprintln!("  {index}: GPU ID {}", target.id.0),
        }
    }
    eprint!("Select GPU index: ");
    io::stderr().flush().map_err(Error::from)?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).map_err(Error::from)?;
    let index = answer
        .trim()
        .parse::<usize>()
        .map_err(|_| error("invalid GPU index"))?;
    targets
        .get(index)
        .ok_or_else(|| error("GPU index is out of range"))
}

fn confirm(matches: &ArgMatches) -> Result<(), Error> {
    eprintln!("WARNING: V/F optimization intentionally probes unstable GPU settings.");
    eprintln!("Driver resets, display loss, application failure, or a system reboot may occur.");
    if matches.get_flag("yes") {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(error("non-interactive optimize requires --yes"));
    }
    eprint!("Type 'yes' to continue: ");
    io::stderr().flush().map_err(Error::from)?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).map_err(Error::from)?;
    if answer.trim().eq_ignore_ascii_case("yes") {
        Ok(())
    } else {
        Err(error("optimization cancelled"))
    }
}

fn validate_workspace(path: &Path) -> Result<(), Error> {
    if path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(error(
            "--workspace must be a relative path without '..' components",
        ));
    }
    Ok(())
}

fn workspace_for(uuid: &str, matches: &ArgMatches) -> Result<PathBuf, Error> {
    if let Some(path) = matches.get_one::<String>("workspace") {
        let path = PathBuf::from(path);
        validate_workspace(&path)?;
        return Ok(path);
    }

    let preferred = PathBuf::from(format!("GPUScan-{uuid}"));
    #[cfg(target_os = "linux")]
    {
        let legacy = PathBuf::from(format!("Scan-{uuid}"));
        if !preferred.exists() && legacy.exists() {
            return Ok(legacy);
        }
    }
    Ok(preferred)
}

fn reset_pstate_offsets(gpu: &GpuTarget<'_>) -> Result<(), Error> {
    let info = run(gpu, QueryGpuInfo)?.output;
    let offsets = info
        .pstate_limits
        .iter()
        .flat_map(|(&pstate, limits)| {
            limits
                .iter()
                .filter(|&(_, limit)| limit.frequency_delta.is_some())
                .map(move |(&domain, _)| (pstate, domain))
        })
        .collect();
    run(gpu, ResetPstateClockOffsets { offsets })?;
    Ok(())
}

fn command_prefix(gpu: &GpuTarget<'_>) -> Vec<String> {
    vec![
        "nvoc-auto-optimizer".to_string(),
        "--gpu".to_string(),
        gpu.id.0.to_string(),
    ]
}

pub fn run_optimize(targets: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let gpu = select_target(targets, matches)?;
    let info = run(gpu, QueryGpuInfo)?.output;
    let uuid = info
        .uuid
        .as_deref()
        .filter(|uuid| !uuid.is_empty())
        .ok_or_else(|| error("target GPU UUID is unavailable"))?
        .trim_start_matches("GPU-")
        .to_string();
    let workspace = workspace_for(&uuid, matches)?;
    let mode = matches
        .get_one::<String>("mode")
        .map(String::as_str)
        .unwrap_or("standard");

    eprintln!(
        "Selected GPU {}: {} ({})\nWorkspace: {}\nMode: {}",
        gpu.id.0,
        info.name,
        uuid,
        workspace.display(),
        mode
    );
    confirm(matches)?;

    fs::create_dir_all(&workspace).map_err(Error::from)?;
    let log = workspace.join("vfp.jsonl");
    let init = workspace.join("vfp-init.csv");
    let temporary = workspace.join("vfp-tem.csv");
    let result = workspace.join("vfp.csv");
    let final_export = workspace.join("vfp-final.csv");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(Error::from)?;
    if matches.get_flag("fresh") {
        File::create(&log).map_err(Error::from)?;
        File::create(&temporary).map_err(Error::from)?;
    }

    reset_pstate_offsets(gpu)?;
    run(
        gpu,
        ResetVfpDeltas {
            domain: VfpResetDomain::All,
        },
    )?;
    run(gpu, ResetVfpLock)?;

    if !init.exists() {
        let mut args = command_prefix(gpu);
        args.extend(["export-vfp".into(), init.to_string_lossy().into_owned()]);
        let export = parse_subcommand(args, "export-vfp")?;
        handle_vfp_export(gpu, &export)?;
    }

    let mut scan_args = command_prefix(gpu);
    let scan_command = if mode == "legacy" {
        "autoscan-vfp-legacy"
    } else {
        "autoscan-vfp"
    };
    scan_args.push(scan_command.into());
    scan_args.extend(["--log".into(), log.to_string_lossy().into_owned()]);
    scan_args.extend(["--cuda-device".into(), gpu.index.to_string()]);
    let external_backend = matches
        .try_get_one::<String>("stressor_backend")
        .ok()
        .flatten()
        .map(|value| value == "external")
        .unwrap_or_else(|| {
            matches
                .try_get_one::<String>("test_exe")
                .ok()
                .flatten()
                .is_some()
                && matches.try_get_one::<String>("stressor_profile").is_err()
        });
    if external_backend {
        let test_exe = matches
            .try_get_one::<String>("test_exe")
            .ok()
            .flatten()
            .ok_or_else(|| error("external stressor backend requires --test-exe"))?;
        let minload_exe = matches
            .try_get_one::<String>("minload_exe")
            .ok()
            .flatten()
            .ok_or_else(|| error("external stressor backend requires --minload-exe"))?;
        scan_args.extend(["--test-exe".into(), test_exe.clone()]);
        scan_args.extend(["--minload-exe".into(), minload_exe.clone()]);
        if matches
            .try_get_one::<String>("stressor_backend")
            .ok()
            .flatten()
            .is_some()
        {
            scan_args.extend(["--stressor-backend".into(), "external".into()]);
        }
    } else if let Some(config) = matches
        .try_get_one::<String>("stressor_config")
        .ok()
        .flatten()
    {
        scan_args.extend(["--stressor-config".into(), config.clone()]);
    } else {
        let profile = matches
            .try_get_one::<String>("stressor_profile")
            .ok()
            .flatten()
            .cloned()
            .unwrap_or_else(|| "auto".into());
        scan_args.extend(["--stressor-profile".into(), profile]);
    }
    if mode != "legacy" {
        scan_args.extend(["-i".into(), init.to_string_lossy().into_owned()]);
        scan_args.extend(["-o".into(), temporary.to_string_lossy().into_owned()]);
        if mode == "ultrafast" {
            scan_args.push("--ultrafast".into());
        }
    }
    let scan = parse_subcommand(scan_args, scan_command)?;
    let selected = vec![*gpu];
    if mode == "legacy" {
        autoscan_legacy(&selected, &scan)?;
        return Ok(());
    }
    autoscan_gpuboostv3(&selected, &scan)?;

    let mut fix_args = command_prefix(gpu);
    fix_args.extend([
        "fix-vfp-result".into(),
        "-m".into(),
        "1".into(),
        "-v".into(),
        temporary.to_string_lossy().into_owned(),
        "-o".into(),
        result.to_string_lossy().into_owned(),
        "-l".into(),
        log.to_string_lossy().into_owned(),
    ]);
    if mode == "ultrafast" {
        fix_args.push("--ultrafast".into());
    }
    let fix = parse_subcommand(fix_args, "fix-vfp-result")?;
    fix_result(gpu, &fix)?;

    let mut import_args = command_prefix(gpu);
    import_args.extend(["import-vfp".into(), result.to_string_lossy().into_owned()]);
    let import = parse_subcommand(import_args, "import-vfp")?;
    handle_vfp_import(gpu, &import)?;

    let mut export_args = command_prefix(gpu);
    export_args.extend([
        "export-vfp".into(),
        final_export.to_string_lossy().into_owned(),
    ]);
    let export = parse_subcommand(export_args, "export-vfp")?;
    handle_vfp_export(gpu, &export)?;

    eprintln!("Optimization complete: {}", final_export.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_workspace;
    use std::path::Path;

    #[test]
    fn workspace_must_stay_relative_and_contained() {
        assert!(validate_workspace(Path::new("GPUScan-123")).is_ok());
        assert!(validate_workspace(Path::new("nested/GPUScan-123")).is_ok());
        assert!(validate_workspace(Path::new("../outside")).is_err());
        assert!(validate_workspace(Path::new("nested/../../outside")).is_err());
        assert!(validate_workspace(Path::new("/absolute")).is_err());
    }
}
