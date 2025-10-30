use std::process::Command;

#[test]
fn prints_version() {
    let exe = env!("CARGO_BIN_EXE_hn-tui");
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("run hn-tui --version");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "stdout was: {}",
        stdout.trim()
    );
}

#[test]
fn prints_help() {
    let exe = env!("CARGO_BIN_EXE_hn-tui");
    let output = Command::new(exe)
        .arg("--help")
        .output()
        .expect("run hn-tui --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("HN-TUI"));
    assert!(stdout.contains("--version"));
}
