#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!(
            "bugatti-install-script-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_fake_uname(dir: &Path, os: &str, arch: &str) {
    let uname_path = dir.join("uname");
    fs::write(
        &uname_path,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  -s) printf '%s\\n' '{os}' ;;\n  -m) printf '%s\\n' '{arch}' ;;\n  *) exit 1 ;;\nesac\n"
        ),
    )
    .unwrap();

    let mut permissions = fs::metadata(&uname_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&uname_path, permissions).unwrap();
}

fn run_print_target(os: &str, arch: &str) -> std::process::Output {
    let temp_dir = TempDir::new(&format!("{os}-{arch}"));
    write_fake_uname(temp_dir.path(), os, arch);

    let mut path = OsString::from(temp_dir.path());
    path.push(":");
    path.push(env::var_os("PATH").unwrap_or_default());

    let install_script = Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh");
    Command::new("sh")
        .arg(install_script)
        .arg("--print-target")
        .env("PATH", path)
        .output()
        .unwrap()
}

fn print_target_with_uname(os: &str, arch: &str) -> String {
    let output = run_print_target(os, arch);

    assert!(
        output.status.success(),
        "install.sh --print-target failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn install_script_detects_supported_release_targets() {
    let cases = [
        ("Darwin", "arm64", "aarch64-apple-darwin"),
        ("Darwin", "x86_64", "x86_64-apple-darwin"),
        ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
        ("Linux", "aarch64", "aarch64-unknown-linux-gnu"),
        ("Linux", "amd64", "x86_64-unknown-linux-gnu"),
    ];

    for (os, arch, expected) in cases {
        assert_eq!(print_target_with_uname(os, arch), expected);
    }
}

#[test]
fn install_script_rejects_unsupported_platforms() {
    let cases = [
        ("FreeBSD", "amd64", "Unsupported OS"),
        ("Linux", "riscv64", "Unsupported"),
        ("Darwin", "i386", "Unsupported"),
    ];

    for (os, arch, expected_message) in cases {
        let output = run_print_target(os, arch);
        assert!(
            !output.status.success(),
            "expected failure for {os}/{arch}, got success with stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected_message),
            "expected '{expected_message}' in stderr for {os}/{arch}, got: {stderr}"
        );
    }
}
