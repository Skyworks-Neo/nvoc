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
