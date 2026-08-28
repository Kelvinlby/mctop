//! End-to-end checks of the command line, run against the built binary.
//!
//! Nothing here needs a Minecraft server: the subcommands exercised either
//! never open a socket, or are expected to fail to.

use std::process::Command;

fn mctop() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mctop"));
    // Keep the developer's own config and password out of these runs.
    command.env_remove("MCTOP_RCON_PASSWORD");
    command
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn prints_help() {
    let output = mctop().arg("--help").output().unwrap();
    assert!(output.status.success());

    let text = stdout(&output);
    assert!(
        text.contains("Folia"),
        "help should name the target: {text}"
    );
    for subcommand in ["watch", "status", "probe", "config"] {
        assert!(text.contains(subcommand), "help should list {subcommand}");
    }
}

#[test]
fn prints_a_version() {
    let output = mctop().arg("--version").output().unwrap();
    assert!(output.status.success());
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn rejects_a_bad_address() {
    let output = mctop()
        .args(["status", "example.net:port"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("invalid port"));
}

#[test]
fn rejects_a_bad_address_override() {
    let output = mctop().args(["--address", "host:nope"]).output().unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("invalid port"));
}

#[test]
fn rejects_a_nonsense_interval() {
    let output = mctop().args(["--interval", "0"]).output().unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("must be a positive number"));
}

#[test]
fn reports_a_missing_config_file() {
    let output = mctop()
        .args([
            "--config",
            "/mctop/definitely/not/here.toml",
            "config",
            "show",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("reading config"));
}

#[test]
fn writes_and_reads_back_a_config() {
    let directory = std::env::temp_dir().join("mctop-cli-config-test");
    std::fs::remove_dir_all(&directory).ok();
    let path = directory.join("config.toml");
    let path_argument = path.to_string_lossy().into_owned();

    let created = mctop()
        .args(["--config", &path_argument, "config", "init"])
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", stderr(&created));
    assert!(path.exists());

    // A second init must not silently overwrite the operator's edits.
    let refused = mctop()
        .args(["--config", &path_argument, "config", "init"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--force"));

    let forced = mctop()
        .args(["--config", &path_argument, "config", "init", "--force"])
        .output()
        .unwrap();
    assert!(forced.status.success(), "{}", stderr(&forced));

    let shown = mctop()
        .args(["--config", &path_argument, "config", "show"])
        .output()
        .unwrap();
    assert!(shown.status.success(), "{}", stderr(&shown));
    let text = stdout(&shown);
    assert!(
        text.contains(&path_argument),
        "should say where it came from"
    );
    assert!(text.contains("[rcon]"));

    // Overrides are visible in the effective configuration.
    let overridden = mctop()
        .args([
            "--config",
            &path_argument,
            "--address",
            "10.0.0.5:25599",
            "config",
            "show",
        ])
        .output()
        .unwrap();
    assert!(stdout(&overridden).contains("10.0.0.5:25599"));

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn config_path_reports_where_it_would_look() {
    let output = mctop().args(["config", "path"]).output().unwrap();
    assert!(output.status.success());
    assert!(stdout(&output).contains("config.toml"));
}

#[test]
fn a_missing_password_is_explained_rather_than_hung_on() {
    // Port 1 has nothing listening, but the password is checked first.
    let output = mctop()
        .args(["--address", "127.0.0.1:1", "probe"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("MCTOP_RCON_PASSWORD"));
}

#[test]
fn an_unreachable_server_fails_with_a_useful_message() {
    let output = mctop()
        .env("MCTOP_RCON_PASSWORD", "irrelevant")
        .args(["--address", "127.0.0.1:1", "status"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("127.0.0.1:1"),
        "should name the target: {text}"
    );
}
