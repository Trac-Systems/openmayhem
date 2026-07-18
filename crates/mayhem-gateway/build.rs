#![forbid(unsafe_code)]

use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=MAYHEM_BUILD_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    let revision = env::var("MAYHEM_BUILD_GIT_SHA")
        .ok()
        .filter(|value| is_git_revision(value))
        .or_else(git_head_revision)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=MAYHEM_BUILD_GIT_SHA={revision}");
}

fn git_head_revision() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    is_git_revision(revision).then(|| revision.to_ascii_lowercase())
}

fn is_git_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
