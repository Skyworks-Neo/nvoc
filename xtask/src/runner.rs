//! `cargo xtask run`: launch a user-facing component from source.

use crate::args::RunTarget;
use crate::util::{self, Res};
use std::process::Command;

pub fn run(target: RunTarget, passthrough: &[String]) -> Res<()> {
    let root = util::repo_root();
    match target {
        RunTarget::Gui => {
            util::step("launching gui (uv run python main.py)");
            let mut command = util::uv_run(&root, "nvoc-gui", &["python", "main.py"]);
            command.args(passthrough);
            util::run(&mut command)
        }
        RunTarget::Tui => {
            util::step("launching tui (uv run nvoc-tui)");
            let mut command = util::uv_run(&root, "nvoc-tui", &["nvoc-tui"]);
            command.args(passthrough);
            util::run(&mut command)
        }
        RunTarget::Cli => {
            util::step("launching nvoc-cli (cargo run)");
            let mut command = Command::new("cargo");
            command.args(["run", "-p", "nvoc-cli"]).current_dir(&root);
            push_cargo_passthrough(&mut command, passthrough);
            util::run(&mut command)
        }
        RunTarget::Stressor => {
            util::step("launching CUDA stressor (default cuda12 build + vulkan)");
            let mut command = Command::new("cargo");
            command
                .args(["run", "-p", "cli-stressor-cuda-rs", "--features", "vulkan"])
                .current_dir(&root);
            push_cargo_passthrough(&mut command, passthrough);
            util::run(&mut command)
        }
    }
}

/// Cargo needs `--` before arguments destined for the built binary.
fn push_cargo_passthrough(command: &mut Command, passthrough: &[String]) {
    if !passthrough.is_empty() {
        command.arg("--");
        command.args(passthrough);
    }
}
