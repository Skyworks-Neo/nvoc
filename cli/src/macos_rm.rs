use super::*;

pub(super) fn selected(backend: BackendChoice) -> bool {
    backend == BackendChoice::MacosRm
        || (backend == BackendChoice::Auto && cfg!(target_os = "macos"))
}

pub(super) fn validate(command: Command) -> CliResult<()> {
    if matches!(
        command,
        Command::GetGpuList | Command::GetInfo | Command::GetStatus
    ) {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "{} is unavailable on macOS RM; supported commands: get-gpu-list, get-info, get-status. Performance writes are not implemented.",
            command.name()
        )))
    }
}

pub(super) fn execute(invocation: &Invocation, command: Command) -> CliResult<Execution> {
    // Refuse unsupported commands before opening IOKit or allocating RM objects.
    validate(command)?;
    let inventory = discover_targets(BackendSet::MacosRm)?;
    let targets = inventory.targets();
    let selected = select_targets(&targets, &gpu_selector(invocation))?;
    if selected.is_empty() {
        return Err(CliError::new("no GPUs matched the selector"));
    }
    let results: Vec<TargetResult> = selected.into_iter().map(|target| {
        let output = target.macos_rm().map_err(CliError::from).and_then(|device| {
            let mut value = match command {
                Command::GetGpuList => json!({
                    "index": target.index, "gpu_id": target.id.0,
                    "gpu_id_hex": format!("0x{:04X}", target.id.0),
                    "pci_bus": target.id.pci_bus(), "pci_address": device.info().pci_address,
                    "rm_gpu_id": device.info().rm_gpu_id, "name": device.info().name,
                    "backend_nvapi": false, "backend_nvml": false, "backend_macos_rm": true,
                }),
                Command::GetInfo => serde_json::to_value(run(&target, nvoc_core::QueryMacosRmInfo)?.output)?,
                Command::GetStatus => serde_json::to_value(run(&target, nvoc_core::QueryMacosRmStatus)?.output)?,
                _ => unreachable!("validated above"),
            };
            if let Some(object) = value.as_object_mut() {
                object.insert("capabilities".into(), json!({
                    "device_info": true, "pstate_query": true,
                    "telemetry": true, "performance_writes": false,
                }));
            }
            Ok(value)
        });
        match output {
            Ok(output) => TargetResult { gpu_id: Some(target.id.0), backend: "macos-rm",
                ok: true, output: Some(output), error: None },
            Err(error) => TargetResult { gpu_id: Some(target.id.0), backend: "macos-rm",
                ok: false, output: None, error: Some(error.to_string()) },
        }
    }).collect();
    let partial = command == Command::GetStatus
        && results.iter().any(|result: &TargetResult| {
            result
                .output
                .as_ref()
                .and_then(|v| v["unavailable"].as_object())
                .is_some_and(|v| !v.is_empty())
        });
    Ok(Execution {
        function: command.name(),
        command,
        backend: "macos-rm".into(),
        warnings: if partial {
            vec!["Partial macOS RM status: unavailable telemetry is null; see unavailable for reasons.".into()]
        } else {
            Vec::new()
        },
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_flag_is_global_and_conflicts_with_other_backends() {
        for args in [["--macos-rm", "get-info"], ["get-info", "--macos-rm"]] {
            assert_eq!(parse_args(args).unwrap().backend, BackendChoice::MacosRm);
        }
        for other in ["--nvapi", "--nvml"] {
            assert!(parse_args(["get-info", "--macos-rm", other]).is_err());
        }
    }

    #[test]
    fn unsupported_commands_are_rejected_before_driver_access() {
        let invocation = parse_args(["set-power-limit", "100", "--macos-rm"]);
        assert!(
            invocation
                .unwrap_err()
                .to_string()
                .contains("unavailable on macOS RM")
        );
        // Also check callers constructing Invocations without the parser.
        let mut invocation = parse_args(["get-info", "--macos-rm"]).unwrap();
        invocation.command = Some(Command::SetPowerLimit);
        let error = run_invocation(&invocation).unwrap_err().to_string();
        assert!(error.contains("unavailable on macOS RM"));
        assert!(!error.contains("driver loaded"));
    }

    #[test]
    fn auto_uses_platform_backend_without_changing_explicit_choices() {
        assert_eq!(selected(BackendChoice::Auto), cfg!(target_os = "macos"));
        assert!(selected(BackendChoice::MacosRm));
        assert!(!selected(BackendChoice::Nvapi));
        assert!(!selected(BackendChoice::Nvml));
    }

    #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
    #[test]
    fn explicit_backend_on_other_platforms_reports_requirement() {
        let invocation = parse_args(["get-info", "--macos-rm"]).unwrap();
        assert!(
            run_invocation(&invocation)
                .unwrap_err()
                .to_string()
                .contains("requires x86_64 macOS")
        );
    }
}
