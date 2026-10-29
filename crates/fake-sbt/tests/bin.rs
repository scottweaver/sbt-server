//! Exercises the `fake-sbt` binary itself. Its presence also makes cargo
//! build the binary whenever the workspace is tested, so the other crates'
//! integration tests never run against a stale one.

use std::process::Command;

use fake_sbt::{BOOT_FAIL_ENV, SCENARIO_ENV};

fn fake() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fake-sbt"));
    command.env_remove(SCENARIO_ENV).env_remove(BOOT_FAIL_ENV);
    command
}

#[test]
fn boot_failure_exits_one_with_the_error_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let output = fake()
        .current_dir(dir.path())
        .env(BOOT_FAIL_ENV, "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project loading failed"), "{stderr}");
    assert!(!dir.path().join("project/target/active.json").exists());
}

#[test]
fn unparseable_scenario_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let output = fake()
        .current_dir(dir.path())
        .env(SCENARIO_ENV, "exec=nonsense")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bad configuration"));
}

#[test]
fn bin_path_resolves_to_the_built_binary() {
    assert_eq!(
        fake_sbt::bin_path().unwrap(),
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_fake-sbt"))
    );
}
