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
fn invalid_conflict_strategy_is_a_usage_error() {
    let s = Scratch::new("conflict");
    let out = cmd(&s)
        .args(["--yes", "--conflict", "bogus"])
        .arg(s.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn history_and_stats_support_json() {
    let s = Scratch::new("json");
    for flag in ["--history", "--stats"] {
        let out = cmd(&s).args([flag, "--json"]).output().unwrap();
        assert!(out.status.success(), "{flag} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let trimmed = stdout.trim();
        assert!(
            trimmed.starts_with('[') || trimmed.starts_with('{'),
            "{flag} --json did not emit JSON: {stdout}"
        );
    }
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

#[test]
fn dry_run_json_is_valid_and_counts_matches() {
    let s = Scratch::new("plan-json");
    let folder = s.path().join("media");
    let output = folder.join("converted");
    fs::create_dir_all(&output).unwrap();
    fs::write(folder.join("image.bmp"), b"input").unwrap();
    // Generated files should be ignored when scanning for new work.
    fs::write(output.join("another.bmp"), b"old").unwrap();
    let run = cmd(&s)
        .args([
            "--yes",
            "--dry-run",
            "--json",
            "--threads",
            "2",
            "--output-subfolder",
            "converted",
        ])
        .arg(&folder)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&run.stdout).expect("stdout must contain only JSON");
    let images = json
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["input_format"] == "BMP")
        .unwrap();
    assert_eq!(images["files_matched"], 1);
    assert_eq!(images["input_bytes"], 5);
    assert_eq!(images["files_converted"], 0);
}

#[test]
fn unsafe_output_subfolder_and_thread_count_are_rejected() {
    let s = Scratch::new("invalid-controls");
    for args in [
        ["--yes", "--output-subfolder", "../escape"],
        ["--yes", "--threads", "0"],
        ["--yes", "--threads", "999"],
    ] {
        let output = cmd(&s).args(args).arg(s.path()).output().unwrap();
        assert!(!output.status.success(), "{args:?} was accepted");
    }
}

#[cfg(unix)]
#[test]
fn failing_converter_does_not_damage_existing_output() {
    use std::os::unix::fs::PermissionsExt;

    let s = Scratch::new("atomic-fail");
    let folder = s.path().join("media");
    let tools = s.path().join("fake-bin");
    fs::create_dir_all(&folder).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(folder.join("picture.bmp"), b"source image").unwrap();
    fs::write(folder.join("picture.png"), b"valuable previous output").unwrap();
    let magick = tools.join("magick");
    fs::write(&magick, b"#!/bin/sh\nfor arg in \"$@\"; do output=\"$arg\"; done\nprintf 'partial' > \"$output\"\nexit 13\n").unwrap();
    let mut permissions = fs::metadata(&magick).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&magick, permissions).unwrap();

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![tools.clone()];
    paths.extend(std::env::split_paths(&inherited_path));
    let full_path = std::env::join_paths(paths).unwrap();
    let run = cmd(&s)
        .args(["--yes", "--conflict", "overwrite", "--threads", "4"])
        .arg(&folder)
        .env("PATH", full_path)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "a failing converter unexpectedly succeeded"
    );
    assert_eq!(
        fs::read(folder.join("picture.png")).unwrap(),
        b"valuable previous output"
    );
    assert_eq!(
        fs::read(folder.join("picture.bmp")).unwrap(),
        b"source image"
    );
    assert!(
        fs::read_dir(&folder)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".rusty-crunch-stage-")),
        "abandoned staging files after a failed conversion"
    );
}

#[cfg(unix)]
#[test]
fn converter_publishes_completed_output_without_deleting_input() {
    use std::os::unix::fs::PermissionsExt;

    let s = Scratch::new("atomic-success");
    let folder = s.path().join("media");
    let tools = s.path().join("fake-bin");
    fs::create_dir_all(&folder).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(folder.join("picture.bmp"), b"source image").unwrap();
    let magick = tools.join("magick");
    fs::write(&magick, b"#!/bin/sh\nfor arg in \"$@\"; do output=\"$arg\"; done\nprintf 'complete conversion' > \"$output\"\n").unwrap();
    let mut permissions = fs::metadata(&magick).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&magick, permissions).unwrap();

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![tools.clone()];
    paths.extend(std::env::split_paths(&inherited_path));
    let run = cmd(&s)
        .args([
            "--yes",
            "--output-subfolder",
            "compressed",
            "--threads",
            "2",
        ])
        .arg(&folder)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        fs::read(folder.join("compressed/picture.png")).unwrap(),
        b"complete conversion"
    );
    assert_eq!(
        fs::read(folder.join("picture.bmp")).unwrap(),
        b"source image"
    );
}
