//! `cargo xtask`: the one-stop development entry point for the nvoc workspace.
//!
//! Dependency-free by design; see args::HELP for the command surface.

mod args;
mod builder;
mod checker;
mod doctor;
mod runner;
mod tester;
mod util;

use args::Command;
use std::process::ExitCode;

fn main() -> ExitCode {
    let tokens: Vec<String> = std::env::args().skip(1).collect();
    let command = match args::parse(&tokens) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let result = match command {
        Command::Help => {
            print!("{}", args::HELP);
            return ExitCode::SUCCESS;
        }
        Command::Setup(setup) => doctor::setup(&setup),
        Command::Build(build) => builder::build(&build),
        Command::Check => checker::check(),
        Command::Test(tier) => tester::test(tier),
        Command::Run {
            target,
            passthrough,
        } => runner::run(target, &passthrough),
        Command::Ci => checker::check().and_then(|()| tester::test(args::Tier::Safe)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\nxtask: {message}");
            ExitCode::FAILURE
        }
    }
}
