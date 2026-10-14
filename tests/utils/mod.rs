use std::process::Command;
use std::process::Stdio;
use strip_ansi_escapes::strip as strip_ansi_escapes;

#[allow(dead_code, reason = "not every test target uses this helper")]
pub fn run_cmd(args: &[&str]) -> String {
    let out = String::from_utf8(strip_ansi_escapes(run_cmd_raw(args, &[]))).unwrap();
    let out = out.trim().to_string();

    // Expected trees are written with `/` separators.
    #[cfg(windows)]
    let out = out.replace('\\', "/");

    out
}

/// Like [`run_cmd`] but keeps ANSI escapes, sets `envs`, and ignores `NO_COLOR` and `LS_COLORS`
/// from the host environment unless given in `envs`.
#[allow(dead_code, reason = "not every test target uses this helper")]
pub fn run_cmd_raw(args: &[&str], envs: &[(&str, &str)]) -> Vec<u8> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "run",
        "--",
        "--threads",
        "1",
        "--disk-usage",
        "logical",
        "--sort",
        "name",
        "--no-config",
    ]);

    for arg in args {
        cmd.arg(arg);
    }

    cmd.env_remove("NO_COLOR").env_remove("LS_COLORS");
    cmd.envs(envs.iter().copied());

    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    if !output.status.success() {
        let err_msg = String::from_utf8(output.stderr).unwrap();
        panic!("{err_msg}");
    }

    output.stdout
}
