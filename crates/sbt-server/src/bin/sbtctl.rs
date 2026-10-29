//! `sbtctl`: what agents run instead of `sbt`. Everything lives in
//! `sbt_server_cli`; this binary exists so one package ships both
//! executables.

use std::process::ExitCode;

fn main() -> ExitCode {
    sbt_server_cli::run()
}
