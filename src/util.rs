#[cfg(target_os = "windows")]
use anyhow::Context;
use anyhow::{bail, Result};
use console::style;
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Number of available CPU cores (cached).
pub fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// Module-level cache for `has()` lookups.
type HasCacheMap = Vec<(String, bool)>;
static HAS_CACHE: OnceLock<Mutex<HasCacheMap>> = OnceLock::new();

/// Cache for resolved command paths (or not found).
type ResolveCacheMap = Vec<(String, Option<String>)>;
static RESOLVE_CACHE: OnceLock<Mutex<ResolveCacheMap>> = OnceLock::new();

/// Resolve a command to an executable path or runnable command token.
/// On success, returns either an absolute executable path or the command name.
pub fn resolve_command(name: &str) -> Option<String> {
    let cache = RESOLVE_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());

    if let Some((_, found)) = guard.iter().find(|(n, _)| n == name) {
        return found.clone();
    }

    let found = resolve_command_uncached(name);
    guard.push((name.to_string(), found.clone()));
    found
}

/// Returns true if `name` is found in PATH (cached per unique name).
pub fn has(name: &str) -> bool {
    let cache = HAS_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(entry) = guard.iter().find(|(n, _)| n == name) {
        return entry.1;
    }

    let found = resolve_command(name).is_some();
    guard.push((name.to_string(), found));
    found
}

/// Clear the has() cache so freshly installed tools are detected.
pub fn clear_has_cache() {
    if let Some(cache) = HAS_CACHE.get() {
        cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    if let Some(cache) = RESOLVE_CACHE.get() {
        cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// On Windows, refresh this process PATH from Machine+User environment scopes.
/// This helps detect tools immediately after winget/choco/scoop installs.
#[cfg(target_os = "windows")]
pub fn refresh_windows_process_path() {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let merged = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !merged.is_empty() {
                std::env::set_var("PATH", merged);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)] // no-op stub; only meaningful on Windows
pub fn refresh_windows_process_path() {}

/// Total threads to use for parallel processing, based on the configured thread mode.
pub fn active_threads() -> usize {
    let total = cores();
    crate::config::load().thread_mode.to_threads(total)
}

// ── Size parsing ───────────────────────────────────────────────────────────────────────

/// Parse a size string like "10MB", "1.5GB", "512KB" into bytes.
/// Returns None if parsing fails.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    let pos = s.find(|c: char| c.is_alphabetic())?;
    let (num_str, unit) = s.split_at(pos);

    let num: f64 = num_str.trim().parse().ok()?;
    if num < 0.0 {
        return None;
    }

    let multiplier: u64 = match unit.trim() {
        "B" => 1,
        "KB" => 1_024,
        "MB" => 1_024 * 1_024,
        "GB" => 1_024 * 1_024 * 1_024,
        "TB" => 1_024u64 * 1_024 * 1_024 * 1_024,
        _ => return None,
    };

    Some((num * multiplier as f64) as u64)
}

/// VAAPI render node: honours `RUSTY_CRUNCH_VAAPI_DEVICE`, otherwise the first
/// `/dev/dri/renderD*`, otherwise `/dev/dri/renderD128` (the common default).
pub fn vaapi_device() -> String {
    if let Ok(p) = std::env::var("RUSTY_CRUNCH_VAAPI_DEVICE") {
        let p = p.trim();
        if !p.is_empty() {
            return p.to_string();
        }
    }
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        let mut nodes: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("renderD"))
            .collect();
        nodes.sort();
        if let Some(n) = nodes.first() {
            return format!("/dev/dri/{n}");
        }
    }
    "/dev/dri/renderD128".to_string()
}

// ── Logging & notifications ───────────────────────────────────────────────────────────

static VERBOSE: AtomicBool = AtomicBool::new(false);
static QUIET: AtomicBool = AtomicBool::new(false);

/// Enable verbose diagnostics (also echoed to stderr).
pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

/// Suppress non-essential stdout.
pub fn set_quiet(v: bool) {
    QUIET.store(v, Ordering::Relaxed);
}

pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

pub fn config_dir() -> std::path::PathBuf {
    // Allow full isolation (used by tests): RUSTY_CRUNCH_CONFIG=<dir>/config.json
    if let Some(custom) = std::env::var_os("RUSTY_CRUNCH_CONFIG") {
        let p = std::path::PathBuf::from(custom);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                return parent.to_path_buf();
            }
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-crunch")
}

/// Where the diagnostics log is written.
pub fn log_path() -> std::path::PathBuf {
    config_dir().join("rusty-crunch.log")
}

/// Append a timestamped line to the log file, rotating at ~1 MiB.
/// Verbose mode (and any `ERROR`) also echoes to stderr.
pub fn log_event(level: &str, msg: &str) {
    use std::io::Write;

    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 1_000_000 {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{ts}] {level} {msg}");
    }
    if is_verbose() || level == "ERROR" {
        eprintln!("  {level}: {msg}");
    }
}

/// Best-effort desktop notification (no-op if the platform tool is missing).
pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send").arg(title).arg(body).status();
    }
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            esc(body),
            esc(title)
        );
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
    #[cfg(target_os = "windows")]
    {
        let esc = |s: &str| s.replace('\'', "''");
        let script = format!(
            "[reflection.assembly]::loadwithpartialname('System.Windows.Forms') | Out-Null; [System.Windows.Forms.MessageBox]::Show('{}','{}')",
            esc(body),
            esc(title)
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body);
    }
}

// ── Auto-update ───────────────────────────────────────────────────────────────────────

/// Check GitHub for a newer release of rusty-crunch.
/// Returns `Ok(Some(version))` if a newer version is available,
/// `Ok(None)` if already up-to-date, or `Err` if the check failed.
pub fn check_for_update() -> Result<Option<String>> {
    if !has("curl") {
        bail!("curl is not installed — required for update checks");
    }
    let output = Command::new("curl")
        .args([
            "-sf",
            "--connect-timeout",
            "5",
            "--max-time",
            "10",
            "https://api.github.com/repos/pgm1207/rusty-crunch/releases/latest",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("curl failed: {}", e))?;

    if !output.status.success() {
        bail!("Could not reach GitHub — check your internet connection");
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&body)
        .map_err(|_| anyhow::anyhow!("Unexpected response from GitHub API"))?;
    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("tag_name missing from GitHub response"))?;
    let ver = tag.trim_start_matches('v').to_string();
    if is_newer_than_current(&ver) {
        Ok(Some(ver))
    } else {
        Ok(None)
    }
}

fn is_newer_than_current(ver: &str) -> bool {
    fn parse(v: &str) -> [u32; 3] {
        let mut p = v.splitn(3, '.');
        [
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
            p.next().and_then(|x| x.parse().ok()).unwrap_or(0),
        ]
    }
    parse(ver) > parse(env!("CARGO_PKG_VERSION"))
}

fn update_artifact() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("rusty-crunch-linux-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("rusty-crunch-macos-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("rusty-crunch-macos-x86_64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("rusty-crunch-windows-x86_64.exe")
    } else {
        None
    }
}

/// Expected SHA-256 for `artifact` from the release's `SHA256SUMS`, if published.
fn fetch_expected_sha(version: &str, artifact: &str) -> Option<String> {
    let url =
        format!("https://github.com/pgm1207/rusty-crunch/releases/download/v{version}/SHA256SUMS");
    let out = Command::new("curl")
        .args(["-sfL", "--connect-timeout", "10", "--max-time", "30"])
        .arg(&url)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout);
    for line in body.lines() {
        let mut it = line.split_whitespace();
        if let (Some(hash), Some(name)) = (it.next(), it.next()) {
            if name.trim_start_matches('*') == artifact {
                return Some(hash.to_lowercase());
            }
        }
    }
    None
}

/// Compute the SHA-256 of a file with the platform's standard tool.
fn sha256_file(path: &std::path::Path) -> Option<String> {
    let (tool, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("shasum", &["-a", "256"])
    } else {
        ("sha256sum", &[])
    };
    if !has(tool) {
        return None;
    }
    let out = Command::new(tool).args(args).arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(|h| h.to_lowercase())
}

/// Download and install the given version. Replaces the running binary and restarts.
/// On Windows: swaps via a helper batch script.
/// On Unix: atomic rename, then exec the new binary in-place.
pub fn download_and_install_update(version: &str) -> Result<()> {
    let artifact = update_artifact()
        .ok_or_else(|| anyhow::anyhow!("Auto-update not supported on this platform"))?;

    let url =
        format!("https://github.com/pgm1207/rusty-crunch/releases/download/v{version}/{artifact}");

    let current_exe = std::env::current_exe()?;
    let tmp = current_exe.with_extension("update_tmp");

    println!(
        "  {} Downloading v{} \u{2026}",
        style("\u{2b07}").cyan(),
        style(version).white().bold()
    );

    let status = Command::new("curl")
        .args([
            "-L",
            "--connect-timeout",
            "30",
            "--max-time",
            "300",
            "-#",
            "-o",
        ])
        .arg(&tmp)
        .arg(&url)
        .status()
        .map_err(|_| anyhow::anyhow!("curl not found — install it to use auto-update"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("Download failed — check your internet connection");
    }

    // Supply-chain check: verify the artifact against the release SHA256SUMS.
    match fetch_expected_sha(version, artifact) {
        Some(expected) => match sha256_file(&tmp) {
            Some(actual) if actual == expected => {
                println!("  {} Checksum verified", style("\u{2714}").green());
            }
            Some(_) => {
                let _ = std::fs::remove_file(&tmp);
                bail!("Checksum mismatch for {artifact} — aborting update");
            }
            None => {
                eprintln!("  \u{26a0} Could not verify checksum (sha256sum/shasum not found)");
            }
        },
        None => {
            eprintln!(
                "  \u{26a0} Release publishes no SHA256SUMS — skipping checksum verification"
            );
        }
    }

    #[cfg(target_os = "windows")]
    {
        let new_exe = current_exe.with_extension("new.exe");
        std::fs::rename(&tmp, &new_exe)?;

        let cur = current_exe.to_string_lossy().replace('"', "\"\"");
        let new = new_exe.to_string_lossy().replace('"', "\"\"");
        let bat = current_exe.with_extension("upd.bat");
        let bat_str = bat
            .to_str()
            .context("Update batch path is not valid UTF-8")?;
        std::fs::write(
            &bat,
            format!("@echo off\r\nping -n 3 127.0.0.1>nul\r\nmove /y \"{new}\" \"{cur}\"\r\nstart \"\" \"{cur}\"\r\ndel \"%~f0\"\r\n").as_bytes(),
        )?;
        Command::new("cmd")
            .args(["/C", "start", "/min", "", bat_str])
            .spawn()?;
        println!(
            "\n  {} Updated to v{}. Restarting\u{2026}\n",
            style("\u{2714}").green().bold(),
            version
        );
        std::process::exit(0);
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::process::CommandExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &current_exe)?;
        println!(
            "\n  {} Updated to v{}. Restarting\u{2026}\n",
            style("\u{2714}").green().bold(),
            version
        );
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let err = Command::new(&current_exe).args(&args).exec();
        bail!("Restart failed: {}", err);
    }
}

fn resolve_command_uncached(name: &str) -> Option<String> {
    if let Some(p) = resolve_from_path(name) {
        return Some(p);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(p) = resolve_windows_known_location(name) {
            return Some(p);
        }
    }

    None
}

fn resolve_from_path(name: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    let checker = "where";
    #[cfg(not(target_os = "windows"))]
    let checker = "which";

    let output = Command::new(checker)
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(first.to_string())
}

#[cfg(target_os = "windows")]
fn program_files() -> Option<PathBuf> {
    std::env::var_os("ProgramFiles").map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn user_profile() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn resolve_windows_known_location(name: &str) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(appdata) = local_app_data() {
        candidates.push(
            appdata
                .join("Microsoft")
                .join("WindowsApps")
                .join(format!("{name}.exe")),
        );
    }

    if let Some(home) = user_profile() {
        let shims = home.join("scoop").join("shims");
        candidates.push(shims.join(format!("{name}.exe")));
        candidates.push(shims.join(format!("{name}.cmd")));
    }

    if let Some(pf) = program_files() {
        match name {
            "magick" => {
                if let Ok(entries) = std::fs::read_dir(&pf) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
                                if n.starts_with("ImageMagick-") {
                                    candidates.push(p.join("magick.exe"));
                                }
                            }
                        }
                    }
                }
            }
            "soffice" | "libreoffice" => {
                candidates.push(pf.join("LibreOffice").join("program").join("soffice.exe"));
            }
            "ffmpeg" => {
                candidates.push(pf.join("ffmpeg").join("bin").join("ffmpeg.exe"));
            }
            "ffprobe" => {
                candidates.push(pf.join("ffmpeg").join("bin").join("ffprobe.exe"));
            }
            "gswin64c" | "gswin32c" | "gs" => {
                let gsroot = pf.join("gs");
                if let Ok(entries) = std::fs::read_dir(gsroot) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            candidates.push(p.join("bin").join("gswin64c.exe"));
                            candidates.push(p.join("bin").join("gswin32c.exe"));
                            candidates.push(p.join("bin").join("gs.exe"));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let program_data = std::env::var_os("ProgramData").map(PathBuf::from);
    if let Some(pd) = program_data {
        candidates.push(
            pd.join("chocolatey")
                .join("bin")
                .join(format!("{name}.exe")),
        );
        candidates.push(
            pd.join("chocolatey")
                .join("bin")
                .join(format!("{name}.bat")),
        );
    }

    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

pub fn ffmpeg_command() -> String {
    resolve_command("ffmpeg").unwrap_or_else(|| "ffmpeg".to_string())
}

pub fn ffprobe_command() -> String {
    resolve_command("ffprobe").unwrap_or_else(|| "ffprobe".to_string())
}

/// Detect the best H.264 encoder available in ffmpeg (cached).
/// Returns (encoder, quality_args) — e.g. ("libx264", &["-preset","medium","-crf","23"])
/// or ("h264_nvenc", &["-preset","p4","-crf","23"]) for NVIDIA GPUs.
pub fn best_h264_encoder() -> &'static H264Encoder {
    static ENCODER: OnceLock<H264Encoder> = OnceLock::new();
    ENCODER.get_or_init(detect_h264_encoder)
}

pub fn software_h264_encoder() -> &'static H264Encoder {
    static SW_ENCODER: OnceLock<H264Encoder> = OnceLock::new();
    SW_ENCODER.get_or_init(|| {
        if probe_encoder("libx264", &[]) {
            H264Encoder {
                name: "libx264",
                quality_args: &["-preset", "medium", "-crf", "23"],
            }
        } else {
            H264Encoder {
                name: "libopenh264",
                quality_args: &[],
            }
        }
    })
}

pub struct H264Encoder {
    pub name: &'static str,
    pub quality_args: &'static [&'static str],
}

fn detect_h264_encoder() -> H264Encoder {
    // Try encoders in preference order by actually probing ffmpeg.
    // Just checking `-encoders` output is insufficient — an encoder may be
    // listed but fail at runtime (e.g. h264_nvenc without CUDA drivers).

    #[cfg(target_os = "macos")]
    if probe_encoder("h264_videotoolbox", &[]) {
        return H264Encoder {
            name: "h264_videotoolbox",
            quality_args: &["-q:v", "65"],
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        if probe_encoder("h264_nvenc", &[]) {
            return H264Encoder {
                name: "h264_nvenc",
                quality_args: &["-preset", "p4", "-crf", "23"],
            };
        }

        if probe_encoder("h264_qsv", &[]) {
            return H264Encoder {
                name: "h264_qsv",
                quality_args: &["-global_quality", "23"],
            };
        }

        // Mesa compatible GPUs (AMD/Intel on Linux) via VAAPI.
        let vaapi = vaapi_device();
        let path = std::path::Path::new(&vaapi);
        if path.exists() {
            let va_args = [
                "-vaapi_device",
                vaapi.as_str(),
                "-vf",
                "format=nv12,hwupload",
            ];
            if probe_encoder("h264_vaapi", &va_args) {
                return H264Encoder {
                    name: "h264_vaapi",
                    quality_args: &["-global_quality:v", "23"],
                };
            }
            // Fallback to other VAAPI codecs only if H.264 VAAPI is unavailable.
            if probe_encoder("hevc_vaapi", &va_args) {
                return H264Encoder {
                    name: "hevc_vaapi",
                    quality_args: &["-global_quality:v", "23"],
                };
            }
            if probe_encoder("av1_vaapi", &va_args) {
                return H264Encoder {
                    name: "av1_vaapi",
                    quality_args: &["-global_quality:v", "23"],
                };
            }
        }
    }

    // Try standard software encoder
    if probe_encoder("libx264", &[]) {
        return H264Encoder {
            name: "libx264",
            quality_args: &["-preset", "medium", "-crf", "23"],
        };
    }

    // Last resort — libopenh264 (Fedora/RHEL ship this instead of libx264)
    H264Encoder {
        name: "libopenh264",
        quality_args: &[],
    }
}

/// Resolve the Ghostscript binary name for the current platform.
/// On Windows, Ghostscript installs as `gswin64c` / `gswin32c`, not `gs`.
pub fn gs_command() -> String {
    if let Some(cmd) = resolve_command("gs") {
        return cmd;
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(cmd) = resolve_command("gswin64c") {
            return cmd;
        }
        if let Some(cmd) = resolve_command("gswin32c") {
            return cmd;
        }
    }
    "gs".to_string()
}

/// Resolve the LibreOffice binary name.
/// On Windows / some macOS installs the command is `soffice`, not `libreoffice`.
pub fn lo_command() -> String {
    if let Some(cmd) = resolve_command("libreoffice") {
        return cmd;
    }
    if let Some(cmd) = resolve_command("soffice") {
        return cmd;
    }
    "libreoffice".to_string()
}

/// Resolve the ImageMagick binary name.
/// On Windows, NEVER fall back to `convert` — that is a built-in disk utility.
pub fn magick_command() -> String {
    if let Some(cmd) = resolve_command("magick") {
        return cmd;
    }
    #[cfg(not(target_os = "windows"))]
    if let Some(cmd) = resolve_command("convert") {
        return cmd;
    }
    "magick".to_string()
}

/// Returns true if Ghostscript is available (any platform-specific binary name).
pub fn has_gs() -> bool {
    has("gs") || has("gswin64c") || has("gswin32c")
}

/// Returns true if LibreOffice is available (any known binary name).
pub fn has_lo() -> bool {
    has("libreoffice") || has("soffice")
}

/// Returns true if ImageMagick is available.
/// On Windows, only checks for `magick` (never `convert`).
pub fn has_magick() -> bool {
    if has("magick") {
        return true;
    }
    #[cfg(not(target_os = "windows"))]
    if has("convert") {
        return true;
    }
    false
}

/// Format byte count as a human-friendly string (e.g. "1.4 MB").
pub fn human_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if b >= GB {
        format!("{:.2} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.0} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

/// Try to actually initialize an encoder — returns true only if it can start.
fn probe_encoder(name: &str, extra_args: &[&str]) -> bool {
    let mut args = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=256x256:d=0.04:r=25",
    ];
    args.extend(extra_args);
    args.extend(["-c:v", name, "-frames:v", "1", "-f", "null", "-"]);
    let ffmpeg = ffmpeg_command();
    Command::new(ffmpeg)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size("100"), None); // no unit
        assert_eq!(parse_size("100B"), Some(100));
        assert_eq!(parse_size("0B"), Some(0));
    }

    #[test]
    fn test_parse_size_kilobytes() {
        assert_eq!(parse_size("1KB"), Some(1024));
        assert_eq!(parse_size("10KB"), Some(10240));
        assert_eq!(parse_size("1.5KB"), Some(1536));
    }

    #[test]
    fn test_parse_size_megabytes() {
        assert_eq!(parse_size("1MB"), Some(1048576));
        assert_eq!(parse_size("10MB"), Some(10485760));
        assert_eq!(parse_size("0.5MB"), Some(524288));
    }

    #[test]
    fn test_parse_size_gigabytes() {
        assert_eq!(parse_size("1GB"), Some(1073741824));
        assert_eq!(parse_size("2GB"), Some(2147483648));
    }

    #[test]
    fn test_parse_size_terabytes() {
        assert_eq!(parse_size("1TB"), Some(1099511627776));
    }

    #[test]
    fn test_parse_size_case_insensitive() {
        assert_eq!(parse_size("10mb"), Some(10485760));
        assert_eq!(parse_size("10MB"), Some(10485760));
        assert_eq!(parse_size("10Mb"), Some(10485760));
    }

    #[test]
    fn test_parse_size_with_spaces() {
        assert_eq!(parse_size("  10 MB  "), Some(10485760));
        assert_eq!(parse_size("10   MB"), Some(10485760));
    }

    #[test]
    fn test_parse_size_invalid() {
        assert_eq!(parse_size("10XB"), None); // invalid unit
        assert_eq!(parse_size("-10MB"), None); // negative
        assert_eq!(parse_size("abc"), None); // not a number
        assert_eq!(parse_size(""), None); // empty
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(100), "100 B");
        assert_eq!(human_bytes(1024), "1 KB");
        assert_eq!(human_bytes(1048576), "1.0 MB");
        assert_eq!(human_bytes(1073741824), "1.00 GB");
    }

    #[test]
    fn test_human_bytes_edge_cases() {
        // Test boundary conditions
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1 KB");
        assert_eq!(human_bytes(1025), "1 KB");
        assert_eq!(human_bytes(2048), "2 KB");
        assert_eq!(human_bytes(2560), "2 KB"); // 2.5 KB rounds to 2 with {:.0}
        assert_eq!(human_bytes(1047552), "1023 KB"); // 1047552 / 1024 = 1022.99... ≈ 1023
        assert_eq!(human_bytes(1048576), "1.0 MB"); // Exactly 1 MB
        assert_eq!(human_bytes(1572864), "1.5 MB");
        assert_eq!(human_bytes(1073741823), "1024.0 MB"); // Just under 1 GB
        assert_eq!(human_bytes(1073741824), "1.00 GB"); // Exactly 1 GB
        assert_eq!(human_bytes(2147483648), "2.00 GB"); // 2 GB
    }

    #[test]
    fn test_parse_size_decimal_edge_cases() {
        // Decimal parsing with various levels of precision
        assert_eq!(parse_size("0.5B"), Some(0)); // Rounds down
        assert_eq!(parse_size("0.5KB"), Some(512)); // Float calc: 0.5 * 1024 = 512
        assert_eq!(parse_size("2.5KB"), Some(2560)); // 2.5 * 1024 = 2560
        assert_eq!(parse_size("1.25MB"), Some(1310720)); // 1.25 * 1024 * 1024 = 1310720
        assert_eq!(parse_size("0.1MB"), Some(104857)); // 0.1 * 1024 * 1024 ≈ 104857.6, truncated to 104857
                                                       // Note: "99.99MB" produces approximately 104847114 due to floating point precision
    }

    #[test]
    fn test_parse_size_large_values() {
        // Test large file sizes
        assert_eq!(parse_size("1TB"), Some(1099511627776));
        assert_eq!(parse_size("100TB"), Some(109951162777600));
        assert_eq!(parse_size("1.5TB"), Some(1649267441664));
    }

    #[test]
    fn test_parse_size_whitespace_handling() {
        // Various whitespace combinations
        assert_eq!(parse_size("10MB"), Some(10485760));
        assert_eq!(parse_size("   10MB   "), Some(10485760));
        assert_eq!(parse_size("10   MB"), Some(10485760));
        assert_eq!(parse_size("  10  MB  "), Some(10485760));
        assert_eq!(parse_size("	10MB"), Some(10485760)); // Tab
    }

    #[test]
    fn test_parse_size_invalid_edge_cases() {
        // More invalid cases
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("   "), None);
        assert_eq!(parse_size("MB"), None); // No number
        assert_eq!(parse_size("10"), None); // No unit
        assert_eq!(parse_size("10 10 MB"), None); // Double number
        assert_eq!(parse_size("10.5.5MB"), None); // Multiple decimals
        assert_eq!(parse_size("10XB"), None); // Invalid unit
        assert_eq!(parse_size("abc MB"), None); // Non-numeric
        assert_eq!(parse_size("--10MB"), None); // Double negative
    }

    #[test]
    fn test_cores() {
        // cores() should return at least 1
        assert!(cores() >= 1);
        // Should match available_parallelism (or 1 if unavailable)
        assert_eq!(
            cores(),
            std::thread::available_parallelism().map_or(1, |n| n.get())
        );
    }

    #[test]
    fn test_has_caching() {
        // Clear cache first
        clear_has_cache();
        // Common tools should be detectable or not consistently
        let result = has("true"); // 'true' is a standard POSIX utility
        assert!(result); // Should exist on all POSIX systems

        // Call again - should use cache
        let result2 = has("true");
        assert_eq!(result, result2);
    }

    #[test]
    fn test_active_threads() {
        // Should return a reasonable number of threads
        let threads = active_threads();
        assert!(threads >= 1);
        assert!(threads <= cores() * 2); // Sanity check
    }
}
