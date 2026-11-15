//! Opt-in separation of virtual address space from an inherited kernel RAM cap.
//! Only the standard, non-namespaced cgroup-v2 layout is supported; no fallback.

use crate::{EngineError, Result};
use std::path::Path;
use std::process::Command;

pub(crate) fn worker_command(
    executable: &Path,
    physical_budget: u64,
    address_space_limit: u64,
) -> Result<Command> {
    validate_limits(physical_budget, address_space_limit)?;
    #[cfg(target_os = "linux")]
    {
        let membership = read_bounded(Path::new("/proc/self/cgroup"))?;
        let mountinfo = read_bounded(Path::new("/proc/self/mountinfo"))?;
        let relative = standard_membership(&membership, &mountinfo)?;
        let directory = Path::new("/sys/fs/cgroup").join(relative);
        let canonical = directory
            .canonicalize()
            .map_err(|error| invalid(error.to_string()))?;
        if canonical != directory {
            return Err(invalid("cgroup directory must not contain symlinks"));
        }
        let cap_path = directory.join("memory.max");
        memory_cap(&read_bounded(&cap_path)?, physical_budget)?;
        Ok(checked_command(
            executable,
            physical_budget,
            address_space_limit,
            Path::new("/proc/self/cgroup"),
            membership.trim(),
            &cap_path,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = executable;
        Err(invalid("inherited cgroup containment requires Linux"))
    }
}

fn invalid(message: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidConfig(format!("inherited cgroup containment: {message}"))
}

fn validate_limits(physical_budget: u64, address_space_limit: u64) -> Result<()> {
    if physical_budget == 0
        || address_space_limit < 1024
        || address_space_limit > i64::MAX as u64
        || address_space_limit / 1024 * 1024 < physical_budget
    {
        return Err(invalid("requires a positive physical budget and a finite address-space limit at least as large"));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn read_bounded(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(65_537).read_to_string(&mut text))
        .map_err(|error| invalid(format!("reading {}: {error}", path.display())))?;
    if text.len() > 65_536 {
        return Err(invalid("kernel metadata exceeds size limit"));
    }
    Ok(text)
}

#[cfg(any(target_os = "linux", test))]
fn standard_membership<'a>(membership: &'a str, mountinfo: &str) -> Result<&'a str> {
    let entry = membership.trim_end_matches('\n');
    let path = entry
        .strip_prefix("0::/")
        .ok_or_else(|| invalid("requires a unified cgroup-v2 membership"))?;
    if path.is_empty()
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.chars().any(|c| c.is_control() || c == '\\')
        })
    {
        return Err(invalid("unsupported cgroup membership path"));
    }
    let mounts: Vec<_> = mountinfo
        .lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let fields: Vec<_> = before.split_whitespace().collect();
            (fields.get(4) == Some(&"/sys/fs/cgroup")).then_some((fields, after))
        })
        .collect();
    if mounts.len() != 1
        || mounts[0].0.get(3) != Some(&"/")
        || mounts[0].1.split_whitespace().next() != Some("cgroup2")
    {
        return Err(invalid(
            "requires standard /sys/fs/cgroup cgroup-v2 mount with root /",
        ));
    }
    Ok(path)
}

#[cfg(any(target_os = "linux", test))]
fn memory_cap(text: &str, physical_budget: u64) -> Result<u64> {
    let value = text.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("memory.max must be a finite positive byte count"));
    }
    let cap = value
        .parse::<u64>()
        .map_err(|_| invalid("invalid memory.max"))?;
    if cap == 0 || cap > physical_budget {
        return Err(invalid(format!(
            "memory.max {cap} exceeds admitted physical budget {physical_budget} or is zero"
        )));
    }
    Ok(cap)
}

// Recheck in the launched child, before exec, so a changed membership/cap cannot
// silently replace the cap verified while constructing the command.
#[cfg(any(target_os = "linux", test))]
fn checked_command(
    executable: &Path,
    physical_budget: u64,
    address_space_limit: u64,
    membership_file: &Path,
    expected_membership: &str,
    cap_file: &Path,
) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(r#"
fail() { echo "inherited cgroup containment: $*" >&2; exit 78; }
membership_file=$1; expected_membership=$2; cap_file=$3; physical_budget=$4; address_kib=$5
shift 5
membership=$(cat "$membership_file") || fail "cannot read child membership"
[ "$membership" = "$expected_membership" ] || fail "child cgroup membership changed"
cap=$(cat "$cap_file") || fail "cannot read child memory.max"
case "$cap" in ''|*[!0-9]*) fail "child memory.max is not finite";; esac
[ "${#cap}" -le 19 ] && [ "$cap" -gt 0 ] && [ "$cap" -le "$physical_budget" ] || fail "child memory.max exceeds admitted physical budget"
ulimit -v "$address_kib" || fail "cannot apply finite address-space limit"
export MAYHEM_ENGINE_MEMORY_LIMIT_MODE=linux-inherited-cgroup-v2+rlimit-as
export MAYHEM_ENGINE_CGROUP_MEMORY_LIMIT_BYTES="$cap"
exec "$@"
"#).arg("mayhem-inherited-cgroup")
        .arg(membership_file).arg(expected_membership).arg(cap_file)
        .arg(physical_budget.to_string()).arg((address_space_limit / 1024).to_string())
        .arg(executable)
        .env("MAYHEM_ENGINE_MEMORY_LIMIT_BYTES", physical_budget.to_string())
        .env("MAYHEM_ENGINE_ADDRESS_SPACE_LIMIT_BYTES", (address_space_limit / 1024 * 1024).to_string());
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    const MOUNT: &str = "32 25 0:28 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n";

    #[test]
    fn inherited_cgroup_requires_standard_kernel_membership() {
        assert_eq!(
            standard_membership("0::/user.slice/laya.service\n", MOUNT).unwrap(),
            "user.slice/laya.service"
        );
        for membership in [
            "0::/",
            "0::/../escape",
            "0::/a/./b",
            "0::/a//b",
            "0::/a\\b",
            "0::/a\n0::/b",
            "1:memory:/a",
        ] {
            assert!(
                standard_membership(membership, MOUNT).is_err(),
                "{membership}"
            );
        }
        for mount in [
            "",
            "32 25 0:28 /a /sys/fs/cgroup rw - cgroup2 cgroup rw",
            "32 25 0:28 / /sys/fs/cgroup rw - tmpfs tmpfs rw",
        ] {
            assert!(standard_membership("0::/a", mount).is_err());
        }
        assert!(standard_membership("0::/a", &format!("{MOUNT}{MOUNT}")).is_err());
    }

    #[test]
    fn inherited_cgroup_requires_finite_cap_within_admission() {
        assert_eq!(memory_cap("12288\n", 14336).unwrap(), 12288);
        for cap in [
            "",
            "max",
            "0",
            "14337",
            "-1",
            "+12",
            "12 34",
            "18446744073709551616",
        ] {
            assert!(memory_cap(cap, 14336).is_err(), "{cap}");
        }
        assert!(validate_limits(14336, 98304).is_ok());
        for (physical, address) in [(0, 98304), (14336, 1024), (1, u64::MAX), (1025, 1025)] {
            assert!(validate_limits(physical, address).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn inherited_cgroup_child_rejects_changed_or_missing_cap_before_exec() {
        let dir = tempfile::tempdir().unwrap();
        let membership = dir.path().join("membership");
        let cap = dir.path().join("memory.max");
        std::fs::write(&membership, "0::/laya.service\n").unwrap();
        for value in [
            "max",
            "0",
            "14337",
            "18446744073709551616",
            "1; echo unsafe",
        ] {
            std::fs::write(&cap, "12288\n").unwrap();
            assert!(memory_cap(&read_bounded(&cap).unwrap(), 14336).is_ok());
            let mut command = checked_command(
                Path::new("/bin/echo"),
                14336,
                98304,
                &membership,
                "0::/laya.service",
                &cap,
            );
            command.arg("EXECUTED");
            std::fs::write(&cap, value).unwrap();
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(78), "{value}");
            assert!(output.stdout.is_empty());
        }
        std::fs::remove_file(&cap).unwrap();
        let output = checked_command(
            Path::new("/bin/echo"),
            14336,
            98304,
            &membership,
            "0::/laya.service",
            &cap,
        )
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(78));
        std::fs::write(&cap, "12288\n").unwrap();
        std::fs::write(&membership, "0::/different.service\n").unwrap();
        let output = checked_command(
            Path::new("/bin/echo"),
            14336,
            98304,
            &membership,
            "0::/laya.service",
            &cap,
        )
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(78));
        assert!(output.stdout.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inherited_cgroup_child_applies_finite_address_limit() {
        let dir = tempfile::tempdir().unwrap();
        let membership = dir.path().join("membership");
        let cap = dir.path().join("memory.max");
        std::fs::write(&membership, "0::/laya.service\n").unwrap();
        std::fs::write(&cap, "1073741824\n").unwrap();
        let output = checked_command(Path::new("/bin/sh"), 2 << 30, 4 << 30, &membership, "0::/laya.service", &cap)
            .args(["-c", "printf '%s %s %s ' \"$MAYHEM_ENGINE_MEMORY_LIMIT_MODE\" \"$MAYHEM_ENGINE_MEMORY_LIMIT_BYTES\" \"$MAYHEM_ENGINE_CGROUP_MEMORY_LIMIT_BYTES\"; ulimit -v"])
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "linux-inherited-cgroup-v2+rlimit-as 2147483648 1073741824 4194304"
        );
    }
}
