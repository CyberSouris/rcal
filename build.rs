use std::env;
use std::process::Command;

fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if let Some(stripped) = s.strip_prefix('v') {
        s = stripped.to_string();
    }
    Some(s)
}

/// Whether a `git describe` result is usable as a version string. With a
/// reachable tag it yields a semver ("0.1.0-3-gabc1234"); with no reachable
/// tag (shallow clone, tags not fetched) it falls back to a bare commit hash
/// via `--always` ("abc1234"), which is not a version and must be rejected.
fn is_semver(version: &str) -> bool {
    version
        .split(['.', '-'])
        .next()
        .map(|major| major.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or(false)
}

fn main() {
    // Re-run when the release pipeline injects an exact version.
    println!("cargo:rerun-if-env-changed=RCAL_VERSION");

    // Honour an explicit version from the release pipeline (git-derived
    // semver); otherwise fall back to `git describe` (last tagged version,
    // plus the number of commits since and a dirty marker), then to Cargo's
    // version when git is unavailable or describe output is a bare hash.
    let version = env::var("RCAL_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| git_describe().filter(|v| is_semver(v)))
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));

    println!("cargo:rustc-env=RCAL_BUILD_VERSION={version}");
}
