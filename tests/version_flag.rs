use std::process::Command;

#[test]
fn prints_version() {
    let exe = env!("CARGO_BIN_EXE_reddix");
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("run reddix --version");
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
    let exe = env!("CARGO_BIN_EXE_reddix");
    let output = Command::new(exe)
        .arg("--help")
        .output()
        .expect("run reddix --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("Reddix"));
    assert!(stdout.contains("--version"));
}
