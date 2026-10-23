use std::process::exit;

fn main() {
    // The heavy lifting runs on a spawned worker with a 64 MiB stack: debug
    // builds keep large stack temporaries (multi-hundred-KB NVAPI structs,
    // wide output tables) un-inlined on the 1 MiB main-thread stack, which
    // overflows on commands like get-private-vftable / get-status. Release
    // builds fit fine, but this removes the debug-mode cliff entirely.
    let worker = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(cli_main)
        .expect("spawn cli main worker");
    match worker.join() {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn cli_main() {
    let args = std::env::args().skip(1);
    let invocation = match nvoc_cli::parse_args(args) {
        Ok(invocation) => invocation,
        Err(err) if err.print_clap() => {
            exit(err.exit_code());
        }
        Err(err) => {
            eprintln!(
                "{}",
                nvoc_cli_common::color::stylize(&format!("Error: {err}"), true)
            );
            eprintln!("Run `nvoc-cli --help` for usage.");
            exit(2);
        }
    };

    nvoc_cli_common::color::init(invocation.no_color);

    // G-Helper OC conflict guard (Windows only): back up + strip G-Helper's
    // persisted GPU OC keys before any GPU call — the standalone-CLI twin of
    // the TUI/GUI startup guards. Best-effort and silent unless something
    // actionable happened (keys stripped / GHelper.exe alive); diagnostics
    // go to stderr so stdout stays pristine for output parsers.
    let ghelper_running = nvoc_cli_common::ghelper_guard::ghelper_running();
    let ghelper = nvoc_cli_common::ghelper_guard::run_guard(None, ghelper_running);
    if !ghelper.removed.is_empty() || ghelper.ghelper_running == Some(true) {
        for line in &ghelper.lines {
            eprintln!("{line}");
        }
    }

    match nvoc_cli::run_invocation(&invocation) {
        Ok(run) => {
            println!("{}", run.rendered);
            exit(run.exit_code);
        }
        Err(err) => {
            eprintln!(
                "{}",
                nvoc_cli_common::color::stylize(&format!("Error: {err}"), true)
            );
            exit(1);
        }
    }
}
