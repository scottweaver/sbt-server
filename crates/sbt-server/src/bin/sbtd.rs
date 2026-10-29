//! `sbtd`: keeps one warm sbt server per build root and serves it to
//! `sbtctl` over loopback HTTP. Everything lives in `sbt_server_daemon`;
//! this binary exists so one package ships both executables.

fn main() -> anyhow::Result<()> {
    sbt_server_daemon::run()
}
