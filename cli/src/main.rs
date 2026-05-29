use std::process::exit;

fn main() {
    let args = std::env::args().skip(1);
    let invocation = match nvoc_cli::parse_args(args) {
        Ok(invocation) => invocation,
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

    if invocation.version {
        println!("nvoc-cli {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if invocation.help {
        println!("{}", nvoc_cli::help_text());
        return;
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
