//! Hermetic CLI integration tests.
//!
//! These run the compiled binary with an isolated configuration directory
//! (`RUSTY_CRUNCH_CONFIG`) and, where relevant, a shimmed `PATH`, so they never
//! touch the developer's real config or require ffmpeg/ImageMagick.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_rusty-crunch")
}

/// A throwaway directory that is removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rusty-crunch-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `Command` with an isolated config dir and no colour.
fn cmd(scratch: &Scratch) -> Command {
    let mut c = Command::new(bin());
    c.env("RUSTY_CRUNCH_CONFIG", scratch.path().join("config.json"))
        .env("NO_COLOR", "1");
    c
}

#[test]
fn version_and_help() {
    let s = Scratch::new("help");
    let out = cmd(&s).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("rusty-crunch"));

    let out = cmd(&s).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--yes"));
    assert!(text.contains("--mode"));
    assert!(text.contains("--list-formats"));
}

#[test]
fn list_formats_lists_every_media_type() {
    let s = Scratch::new("formats");
    let out = cmd(&s).arg("--list-formats").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for label in ["Audio", "Video", "Images", "Documents"] {
        assert!(text.contains(label), "missing {label} in:\n{text}");
    }
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let s = Scratch::new("usage");
    let out = cmd(&s).arg("--definitely-not-a-flag").output().unwrap();
    // clap uses exit code 2 for usage errors.
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn yes_without_folder_is_an_error() {
    let s = Scratch::new("nofolder");
    let out = cmd(&s).arg("--yes").output().unwrap();
    assert!(!out.status.success());
    assert_ne!(out.status.code(), Some(0));
}

#[test]
fn noninteractive_dry_run_emits_json() {
    let s = Scratch::new("dryrun");
    let scans = s.path().join("scans");
    fs::create_dir_all(&scans).unwrap();
    fs::write(scans.join("a.bmp"), b"not really a bmp").unwrap();
    fs::write(scans.join("b.mp3"), b"not really an mp3").unwrap();

    let out = cmd(&s)
        .args(["--yes", "--dry-run", "--json"])
        .arg(&scans)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"input_format\": \"BMP\""),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"output_format\": \"PNG\""),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"input_format\": \"MP3\""),
        "stdout:\n{stdout}"
    );
}

#[test]
fn health_check_fails_when_tools_are_absent() {
    let s = Scratch::new("health");
    // Point PATH at an empty dir so no external tool can be found.
    let empty = s.path().join("emptybin");
    fs::create_dir_all(&empty).unwrap();

    let out = cmd(&s)
        .arg("--health-check")
        .env("PATH", &empty)
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected non-zero exit");
    // 0 happens to be the pre-0.6 behaviour; ensure we changed it.
    assert_ne!(out.status.code(), Some(0));
}

#[test]
fn history_and_stats_run_cleanly_on_empty_state() {
    let s = Scratch::new("hist");
    for flag in ["--history", "--stats"] {
        let out = cmd(&s).arg(flag).output().unwrap();
        assert!(
            out.status.success(),
            "{flag} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
