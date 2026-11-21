//! Standalone `metron-jail` trampoline binary for Lane B confinement.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    metron_sandbox::trampoline::run_from_args(&args)
}
