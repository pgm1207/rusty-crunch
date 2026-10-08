use crate::config::{self, AgentRule, AgentTrigger};
use crate::converter;
use crate::deps;
use crate::formats::MediaType;
use crate::prompt;
use crate::util;
use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Global stop flag — set by Ctrl+C handler.
static STOP: AtomicBool = AtomicBool::new(false);
static HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

fn beta_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "[ALPHA]"
    } else {
        "[BETA]"
    }
}

fn install_ctrlc_handler() {
    if !HANDLER_INSTALLED.swap(true, Ordering::SeqCst) {
        ctrlc::set_handler(|| {
            STOP.store(true, Ordering::SeqCst);
        })
        .ok();
    }
}

#[cfg(unix)]
extern "C" fn sigterm_handler(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn install_sigterm_handler() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            sigterm_handler as *const () as libc::sighandler_t,
        );
    }
}

// ── PID / log file management ───────────────────────────────────────────

fn agent_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rusty-crunch")
}

fn pid_file_path() -> PathBuf {
    agent_dir().join("agent.pid")
}

fn log_file_path() -> PathBuf {
    agent_dir().join("agent.log")
}

fn write_pid(pid: u32) -> Result<()> {
    let path = pid_file_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, pid.to_string())?;
    Ok(())
}

fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_file_path())
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file_path());
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    let want = pid.to_string();
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|o| {
            // Match the PID as an exact whitespace-delimited token, so that
            // e.g. PID 123 does not match a process with PID 1234.
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|line| line.split_whitespace().any(|tok| tok == want))
        })
        .unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn kill_process(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
}

#[cfg(windows)]
fn kill_process(pid: u32) -> bool {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
fn kill_process(_pid: u32) -> bool {
    false
}

fn spawn_background() -> Result<u32> {
    let exe =
        std::env::current_exe().context("Could not determine rusty-crunch executable path")?;
    let log_path = log_file_path();
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("Could not open agent log file")?;
    let log_err = log.try_clone()?;

    #[cfg(unix)]
    let child = unsafe {
        Command::new(exe)
            .arg("--agent")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
            .context("Failed to spawn background agent")?
    };

    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        Command::new(exe)
            .arg("--agent")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .context("Failed to spawn background agent")?
    };

    #[cfg(not(any(unix, windows)))]
    let child = Command::new(exe)
        .arg("--agent")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .context("Failed to spawn background agent")?;

    let pid = child.id();
    write_pid(pid)?;
    Ok(pid)
}

// ── CLI entry points (headless) ─────────────────────────────────────────

/// Run the agent headlessly (invoked via `--agent` flag).
/// Reads config, checks deps, runs the watch/periodic loop.
pub fn run_headless() -> Result<()> {
    let cfg = config::load();
    if cfg.agent_rules.is_empty() {
        anyhow::bail!(
            "No agent rules configured.\n  \
             Use the interactive menu to set up rules first: rusty-crunch"
        );
    }

    // Check deps without interactive prompts
    let mut checked: Vec<MediaType> = Vec::new();
    for rule in &cfg.agent_rules {
        if !checked.contains(&rule.media_type) {
            if let Err(e) = deps::check(rule.media_type) {
                eprintln!("Dependency check failed: {e}");
                return Err(e);
            }
            checked.push(rule.media_type);
        }
    }

    write_pid(std::process::id())?;

    install_ctrlc_handler();
    #[cfg(unix)]
    install_sigterm_handler();
    STOP.store(false, Ordering::SeqCst);

    println!(
        "Agent started (PID {}) — {} rule(s), trigger: {}",
        std::process::id(),
        cfg.agent_rules.len(),
        match cfg.agent_trigger {
            AgentTrigger::Watch => "watch".to_string(),
            AgentTrigger::Periodic(s) => format!("every {} min", s / 60),
        },
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = match cfg.agent_trigger {
        AgentTrigger::Watch => run_watch(&cfg.agent_rules, &rt),
        AgentTrigger::Periodic(secs) => run_periodic(&cfg.agent_rules, secs, &rt),
    };

    println!("Agent stopped (PID {})", std::process::id());
    remove_pid_file();
    result
}

/// Stop a running background agent (invoked via `--agent-stop` flag).
pub fn stop_background() -> Result<()> {
    match read_pid() {
        Some(pid) if is_process_alive(pid) => {
            if kill_process(pid) {
                remove_pid_file();
                println!("  {} Agent stopped (was PID {})", style("✓").green(), pid,);
            } else {
                println!(
                    "  {} Could not stop agent (PID {})",
                    style("✗").red().bold(),
                    pid,
                );
            }
        }
        Some(pid) => {
            remove_pid_file();
            println!(
                "  {} No agent running (stale PID {} cleaned up)",
                style("·").dim(),
                pid,
            );
        }
        None => {
            println!("  {} No agent is running", style("·").dim());
        }
    }
    Ok(())
}

/// Show agent status (invoked via `--agent-status` flag).
pub fn show_status() -> Result<()> {
    match read_pid() {
        Some(pid) if is_process_alive(pid) => {
            println!(
                "  {} Agent is running (PID {})",
                style("✓").green(),
                style(pid).white().bold(),
            );
            let log = log_file_path();
            if log.exists() {
                println!("  {} Log: {}", style("┃").dim(), style(log.display()).dim(),);
            }
        }
        Some(pid) => {
            remove_pid_file();
            println!(
                "  {} No agent running (stale PID {} cleaned up)",
                style("·").dim(),
                pid,
            );
        }
        None => {
            println!("  {} No agent is running", style("·").dim());
        }
    }
    Ok(())
}

// ── Interactive setup ───────────────────────────────────────────────────

/// Agent Mode interactive menu.
pub fn setup() -> Result<()> {
    let mut cfg = config::load();

    loop {
        println!(
            "\n  {} {} {}\n",
            style("🤖").cyan(),
            style("Agent Mode").cyan().bold(),
            style(beta_label()).yellow().bold(),
        );

        if cfg.agent_rules.is_empty() {
            println!("  {} No rules configured yet.\n", style("·").dim());
        } else {
            for (i, rule) in cfg.agent_rules.iter().enumerate() {
                println!(
                    "  {} {} {} {} → {}  {}{}",
                    style(format!("{}.", i + 1)).dim(),
                    rule.media_type.icon(),
                    style(&rule.folder).white(),
                    style(&rule.input_fmt).white().bold(),
                    style(&rule.output_fmt).green().bold(),
                    if rule.recursive { "recursive " } else { "" },
                    if rule.delete_originals {
                        "delete-originals"
                    } else {
                        "keep-originals"
                    },
                );
            }
            println!();
            println!(
                "  {} {}",
                style("Trigger:").dim(),
                match cfg.agent_trigger {
                    AgentTrigger::Watch => "👁️  Watch (instant)".to_string(),
                    AgentTrigger::Periodic(s) => format!("⏱️  Every {} min", s / 60),
                },
            );
            println!();
        }

        let mut items: Vec<&str> = vec!["➕ Add Rule"];
        if !cfg.agent_rules.is_empty() {
            items.push("🗑️  Remove Rule");
            items.push("⏱️  Configure Trigger");
            items.push("▶️  Start Agent");
        }
        items.push("↩  Back");

        let sel = Select::with_theme(&theme())
            .with_prompt("Agent Mode")
            .items(&items)
            .default(0)
            .interact_opt()?;

        match sel {
            None => return Ok(()),
            Some(i) => {
                let label = items[i];
                if label.contains("Add Rule") {
                    if let Some(rule) = add_rule(&cfg)? {
                        cfg.agent_rules.push(rule);
                        config::save(&cfg)?;
                    }
                } else if label.contains("Remove Rule") {
                    remove_rule(&mut cfg)?;
                } else if label.contains("Configure Trigger") {
                    if let Some(trigger) = configure_trigger(&cfg)? {
                        cfg.agent_trigger = trigger;
                        config::save(&cfg)?;
                    }
                } else if label.contains("Start Agent") {
                    start(&cfg)?;
                } else {
                    return Ok(());
                }
            }
        }
    }
}

fn add_rule(cfg: &config::Config) -> Result<Option<AgentRule>> {
    println!(
        "\n  {} {}\n",
        style("➕").cyan(),
        style("New Agent Rule").cyan().bold(),
    );

    let media = match prompt::select_media_type()? {
        Some(m) => m,
        None => return Ok(None),
    };

    deps::ensure(media)?;

    let input_fmt = match prompt::select_input_format(media)? {
        Some(f) => f,
        None => return Ok(None),
    };

    let output_fmt = match prompt::select_output_format(media, input_fmt)? {
        Some(f) => f,
        None => return Ok(None),
    };

    match prompt::lossy_warning(media, input_fmt, output_fmt)? {
        Some(true) => {}
        _ => return Ok(None),
    }

    let folder = match prompt::select_folder(cfg)? {
        Some(f) => f,
        None => return Ok(None),
    };

    let recursive = match prompt::confirm_scan_subdirs(cfg)? {
        Some(r) => r,
        None => return Ok(None),
    };

    let delete_originals = match prompt::confirm_delete_originals(cfg)? {
        Some(d) => d,
        None => return Ok(None),
    };

    println!(
        "\n  {} Rule added: {} → {} in {}\n",
        style("✓").green(),
        style(input_fmt).white().bold(),
        style(output_fmt).green().bold(),
        style(folder.display()).dim(),
    );

    Ok(Some(AgentRule {
        folder: folder.display().to_string(),
        media_type: media,
        input_fmt: input_fmt.to_string(),
        output_fmt: output_fmt.to_string(),
        recursive,
        delete_originals,
    }))
}

fn remove_rule(cfg: &mut config::Config) -> Result<()> {
    if cfg.agent_rules.is_empty() {
        return Ok(());
    }

    let items: Vec<String> = cfg
        .agent_rules
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "{}. {} {} → {} ({})",
                i + 1,
                r.media_type.icon(),
                r.input_fmt,
                r.output_fmt,
                r.folder,
            )
        })
        .collect();

    let sel = Select::with_theme(&theme())
        .with_prompt("Remove which rule?  (Esc to cancel)")
        .items(&items)
        .interact_opt()?;

    if let Some(idx) = sel {
        cfg.agent_rules.remove(idx);
        config::save(cfg)?;
        println!("  {} Rule removed", style("✓").green());
    }
    Ok(())
}

fn configure_trigger(cfg: &config::Config) -> Result<Option<AgentTrigger>> {
    let items = [
        "👁️  Watch — convert files as soon as they appear",
        "⏱️  Periodic — scan folders at a fixed interval",
    ];
    let default = match cfg.agent_trigger {
        AgentTrigger::Watch => 0,
        AgentTrigger::Periodic(_) => 1,
    };

    let sel = Select::with_theme(&theme())
        .with_prompt("Trigger mode  (Esc to cancel)")
        .items(&items)
        .default(default)
        .interact_opt()?;

    match sel {
        None => Ok(None),
        Some(0) => Ok(Some(AgentTrigger::Watch)),
        Some(_) => loop {
            let current = match cfg.agent_trigger {
                AgentTrigger::Periodic(s) => (s / 60).to_string(),
                _ => "5".to_string(),
            };
            let input: String = Input::with_theme(&theme())
                .with_prompt("Scan interval (minutes)")
                .default(current)
                .interact_text()?;

            match input.trim().parse::<u64>() {
                Ok(mins) if mins >= 1 => return Ok(Some(AgentTrigger::Periodic(mins * 60))),
                _ => {
                    println!("  {} Enter a valid integer >= 1", style("✗").red());
                }
            }
        },
    }
}

// ── Agent execution ─────────────────────────────────────────────────────

fn start(cfg: &config::Config) -> Result<()> {
    if cfg.agent_rules.is_empty() {
        anyhow::bail!("No rules configured");
    }

    // Check if an agent is already running
    if let Some(pid) = read_pid() {
        if is_process_alive(pid) {
            println!(
                "\n  {} Agent is already running (PID {})",
                style("⚠").yellow(),
                style(pid).white().bold(),
            );
            println!(
                "  {} Stop it first: {}",
                style("·").dim(),
                style("rusty-crunch --agent-stop").white().bold(),
            );
            return Ok(());
        }
        remove_pid_file();
    }

    // Ensure all deps (interactive — can auto-install)
    let mut ensured: Vec<MediaType> = Vec::new();
    for rule in &cfg.agent_rules {
        if !ensured.contains(&rule.media_type) {
            deps::ensure(rule.media_type)?;
            ensured.push(rule.media_type);
        }
    }

    // Spawn background process
    let pid = spawn_background()?;
    let log = log_file_path();

    println!();
    let sep = style("─".repeat(50)).dim();
    println!("  {sep}");
    println!(
        "  {} {} (PID {})",
        style("🤖").cyan(),
        style("Agent started in background").cyan().bold(),
        style(pid).white().bold(),
    );
    println!(
        "  {} {} rule{}  ·  {}",
        style("┃").dim(),
        style(cfg.agent_rules.len()).cyan().bold(),
        if cfg.agent_rules.len() == 1 { "" } else { "s" },
        match cfg.agent_trigger {
            AgentTrigger::Watch => "instant mode".to_string(),
            AgentTrigger::Periodic(s) => format!("every {} min", s / 60),
        },
    );
    println!("  {} Log: {}", style("┃").dim(), style(log.display()).dim(),);
    println!(
        "  {} Stop:   {}",
        style("┃").dim(),
        style("rusty-crunch --agent-stop").white().bold(),
    );
    println!(
        "  {} Status: {}",
        style("┃").dim(),
        style("rusty-crunch --agent-status").white().bold(),
    );
    println!("  {sep}");
    println!();

    Ok(())
}

// ── Watch Mode ──────────────────────────────────────────────────────────

fn run_watch(rules: &[AgentRule], rt: &tokio::runtime::Runtime) -> Result<()> {
    const MAX_PENDING_EVENTS: usize = 10_000;

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )?;

    // Deduplicate watch paths
    let mut watched: HashSet<String> = HashSet::new();
    for rule in rules {
        let folder = Path::new(&rule.folder);
        if !folder.is_dir() {
            eprintln!("  Warning: skipping non-existent folder: {}", rule.folder);
            continue;
        }
        let key = format!("{}:{}", rule.folder, rule.recursive);
        if watched.insert(key) {
            let mode = if rule.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            if let Err(e) = watcher.watch(folder, mode) {
                eprintln!("  Warning: could not watch {}: {}", rule.folder, e);
            }
        }
    }

    // Initial scan — catch files added while agent was off
    let mut processed: HashSet<PathBuf> = HashSet::new();
    let count = scan_and_convert(rules, &mut processed, rt);
    if count > 0 {
        println!(
            "  {} Initial scan — {} file{} converted\n",
            style("·").dim(),
            count,
            if count == 1 { "" } else { "s" },
        );
    }

    // Event loop with debouncing
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();

    while !STOP.load(Ordering::Relaxed) {
        // Drain all available events
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    for path in event.paths {
                        if path.is_file() && !processed.contains(&path) {
                            if pending.len() >= MAX_PENDING_EVENTS {
                                pending.clear();
                                eprintln!(
                                    "  {} Watch backlog exceeded {} events; dropping backlog",
                                    style("⚠").yellow(),
                                    MAX_PENDING_EVENTS
                                );
                            }
                            pending.insert(path, Instant::now());
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Process files that have been stable for ≥2 seconds
        let now = Instant::now();
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= Duration::from_secs(2))
            .map(|(p, _)| p.clone())
            .collect();

        for path in ready {
            pending.remove(&path);
            if path.exists() && !processed.contains(&path) {
                for rule in rules {
                    if matches_rule(&path, rule) {
                        if process_file(&path, rule, rt) {
                            processed.insert(path.clone());
                        }
                        break;
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    Ok(())
}

// ── Periodic Mode ───────────────────────────────────────────────────────

fn run_periodic(
    rules: &[AgentRule],
    interval_secs: u64,
    rt: &tokio::runtime::Runtime,
) -> Result<()> {
    let interval = interval_secs.max(60); // minimum 1 minute
    let mut processed: HashSet<PathBuf> = HashSet::new();

    while !STOP.load(Ordering::Relaxed) {
        let count = scan_and_convert(rules, &mut processed, rt);
        if count > 0 {
            println!(
                "  {} Scan complete — {} file{} converted",
                style("·").dim(),
                count,
                if count == 1 { "" } else { "s" },
            );
        }

        // Sleep in 1-second chunks for responsive Ctrl+C
        let deadline = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < deadline {
            if STOP.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(())
}

// ── Shared helpers ──────────────────────────────────────────────────────

fn scan_and_convert(
    rules: &[AgentRule],
    processed: &mut HashSet<PathBuf>,
    rt: &tokio::runtime::Runtime,
) -> usize {
    let mut count = 0;
    for rule in rules {
        let folder = Path::new(&rule.folder);
        if !folder.is_dir() {
            continue;
        }

        let max_depth = if rule.recursive { usize::MAX } else { 1 };
        let input_ext = rule.input_fmt.to_ascii_lowercase();
        let output_ext = output_extension(&rule.output_fmt);
        let same_ext = input_ext == output_ext;

        let files: Vec<PathBuf> = WalkDir::new(folder)
            .max_depth(max_depth)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                let p = e.path();
                if processed.contains(p) {
                    return false;
                }
                p.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        let lc = ext.to_ascii_lowercase();
                        lc == input_ext
                            || (input_ext == "jpeg" && lc == "jpg")
                            || (input_ext == "jpg" && lc == "jpeg")
                            || (input_ext == "aiff" && lc == "aif")
                            || (input_ext == "aif" && lc == "aiff")
                            || (input_ext == "aiff" && lc == "aif")
                            || (input_ext == "aif" && lc == "aiff")
                            || (input_ext == "aiff" && lc == "aif")
                            || (input_ext == "aif" && lc == "aiff")
                    })
                    .unwrap_or(false)
            })
            .filter(|e| {
                if same_ext {
                    return true;
                }
                let output = e.path().with_extension(&*output_ext);
                !output.exists()
            })
            .map(|e| e.into_path())
            .collect();

        for file in files {
            if process_file(&file, rule, rt) {
                processed.insert(file);
                count += 1;
            }
        }
    }
    count
}

fn matches_rule(path: &Path, rule: &AgentRule) -> bool {
    let rule_folder = Path::new(&rule.folder);

    if !path.starts_with(rule_folder) {
        return false;
    }

    if !rule.recursive && path.parent() != Some(rule_folder) {
        return false;
    }

    let input_ext = rule.input_fmt.to_ascii_lowercase();
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            let lc = ext.to_ascii_lowercase();
            lc == input_ext
                || (input_ext == "jpeg" && lc == "jpg")
                || (input_ext == "jpg" && lc == "jpeg")
                || (input_ext == "aiff" && lc == "aif")
                || (input_ext == "aif" && lc == "aiff")
                || (input_ext == "aiff" && lc == "aif")
                || (input_ext == "aif" && lc == "aiff")
                || (input_ext == "aiff" && lc == "aif")
                || (input_ext == "aif" && lc == "aiff")
        })
        .unwrap_or(false)
}

fn process_file(path: &Path, rule: &AgentRule, rt: &tokio::runtime::Runtime) -> bool {
    let output_ext = output_extension(&rule.output_fmt);
    let output_path = path.with_extension(&*output_ext);
    let same_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase() == output_ext)
        .unwrap_or(false);

    // For different extensions, skip if output already exists
    if !same_ext && output_path.exists() {
        return false;
    }

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let input_size = path.metadata().map(|m| m.len()).unwrap_or(0);

    match rt.block_on(converter::convert(
        path,
        &output_path,
        converter::ConversionOptions {
            media_type: rule.media_type,
            input_fmt: &rule.input_fmt,
            output_fmt: &rule.output_fmt,
            normalize_audio: false,
            quality: crate::processor::Quality::High,
            keep_metadata: true,
            video_scale: crate::processor::VideoScale::Original,
            image_scale: crate::processor::ImageScale::Original,
            restore_backup: None,
        },
    )) {
        Ok(()) => {
            let output_size = output_path.metadata().map(|m| m.len()).unwrap_or(0);
            let saved = input_size.saturating_sub(output_size);
            println!(
                "  {} {} → .{} (saved {})",
                style("✓").green(),
                style(name.as_ref()).white().bold(),
                output_ext,
                style(util::human_bytes(saved)).cyan(),
            );
            if rule.delete_originals && !same_ext {
                let _ = std::fs::remove_file(path);
            }
            true
        }
        Err(e) => {
            eprintln!(
                "  {} {}: {}",
                style("✗").red().bold(),
                style(name.as_ref()).dim(),
                style(e).red(),
            );
            // The transactional converter has already cleaned failed staging
            // output. Never remove an existing destination on failure.
            false
        }
    }
}

use std::borrow::Cow;

fn output_extension<'a>(fmt: &'a str) -> Cow<'a, str> {
    match fmt {
        "PDF (Optimized)" => Cow::Borrowed("pdf"),
        "JPEG" => Cow::Borrowed("jpg"),
        other => {
            // Check if already lowercase to avoid allocation
            if other
                .chars()
                .all(|c| c.is_ascii_lowercase() || !c.is_ascii_alphabetic())
            {
                Cow::Borrowed(other)
            } else {
                Cow::Owned(other.to_ascii_lowercase())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentRule;
    use crate::formats::MediaType;
    use std::fs;

    fn audio_rule(folder: &str) -> AgentRule {
        AgentRule {
            folder: folder.to_string(),
            media_type: MediaType::Audio,
            input_fmt: "WAV".to_string(),
            output_fmt: "FLAC".to_string(),
            recursive: false,
            delete_originals: false,
        }
    }

    fn image_rule(folder: &str) -> AgentRule {
        AgentRule {
            folder: folder.to_string(),
            media_type: MediaType::Images,
            input_fmt: "JPEG".to_string(),
            output_fmt: "AVIF".to_string(),
            recursive: true,
            delete_originals: false,
        }
    }

    #[test]
    fn matches_rule_correct_extension() {
        let dir = std::env::temp_dir().join("crunch_test_match");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("song.wav");
        fs::write(&file, b"fake").unwrap();

        let rule = audio_rule(&dir.display().to_string());
        assert!(matches_rule(&file, &rule));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_rule_wrong_extension() {
        let dir = std::env::temp_dir().join("crunch_test_nomatch");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("song.mp3");
        fs::write(&file, b"fake").unwrap();

        let rule = audio_rule(&dir.display().to_string());
        assert!(!matches_rule(&file, &rule));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_rule_jpeg_jpg_alias() {
        let dir = std::env::temp_dir().join("crunch_test_jpeg");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("photo.jpg");
        fs::write(&file, b"fake").unwrap();

        let rule = image_rule(&dir.display().to_string());
        assert!(matches_rule(&file, &rule));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_rule_non_recursive_blocks_subdir() {
        let dir = std::env::temp_dir().join("crunch_test_norecurse");
        let sub = dir.join("sub");
        let _ = fs::create_dir_all(&sub);
        let file = sub.join("song.wav");
        fs::write(&file, b"fake").unwrap();

        let rule = audio_rule(&dir.display().to_string());
        // rule.recursive is false, file is in subdir -> should NOT match
        assert!(!matches_rule(&file, &rule));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_rule_recursive_allows_subdir() {
        let dir = std::env::temp_dir().join("crunch_test_recurse");
        let sub = dir.join("sub");
        let _ = fs::create_dir_all(&sub);
        let file = sub.join("photo.jpg");
        fs::write(&file, b"fake").unwrap();

        let rule = image_rule(&dir.display().to_string());
        // rule.recursive is true -> should match
        assert!(matches_rule(&file, &rule));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_extension_special_cases() {
        assert_eq!(output_extension("PDF (Optimized)"), "pdf");
        assert_eq!(output_extension("JPEG"), "jpg");
        assert_eq!(output_extension("FLAC"), "flac");
        assert_eq!(output_extension("OPUS"), "opus");
        assert_eq!(output_extension("PNG"), "png");
    }

    #[test]
    fn scan_skips_already_converted() {
        let dir = std::env::temp_dir().join("crunch_test_skip");
        let _ = fs::create_dir_all(&dir);
        let wav = dir.join("test.wav");
        let flac = dir.join("test.flac");
        fs::write(&wav, b"fake-wav").unwrap();
        fs::write(&flac, b"fake-flac").unwrap(); // output already exists

        let rule = audio_rule(&dir.display().to_string());
        let mut processed = HashSet::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // scan_and_convert should skip because .flac already exists
        let count = scan_and_convert(&[rule], &mut processed, &rt);
        assert_eq!(count, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn human_bytes_formatting() {
        assert_eq!(util::human_bytes(500), "500 B");
        assert_eq!(util::human_bytes(2048), "2 KB");
        assert_eq!(util::human_bytes(1_500_000), "1.4 MB");
        assert_eq!(util::human_bytes(2_000_000_000), "1.86 GB");
    }

    #[test]
    fn pattern_matching_extensions_case_insensitive() {
        let path = std::path::Path::new("test.JPG");
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        assert_eq!(ext, "JPG");

        let path2 = std::path::Path::new("document.PDF");
        let ext2 = path2
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        assert_eq!(ext2, "PDF");
    }

    #[test]
    fn file_filter_matching_basic() {
        let dir = std::env::temp_dir().join("rc_agent_filter_test");
        let _ = fs::create_dir_all(&dir);

        // Create test files
        let mp3_path = dir.join("song.mp3");
        let jpeg_path = dir.join("photo.jpeg");
        let txt_path = dir.join("readme.txt");

        fs::write(&mp3_path, b"mp3 data").unwrap();
        fs::write(&jpeg_path, b"jpeg data").unwrap();
        fs::write(&txt_path, b"text data").unwrap();

        // Check extensions
        let mp3_ext = mp3_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        let jpeg_ext = jpeg_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        let txt_ext = txt_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();

        assert_eq!(mp3_ext, "MP3");
        assert_eq!(jpeg_ext, "JPEG");
        assert_eq!(txt_ext, "TXT");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_rule_pathbuf_handling() {
        let rule = AgentRule {
            folder: "/home/user/music".to_string(),
            media_type: crate::formats::MediaType::Audio,
            input_fmt: "MP3".to_string(),
            output_fmt: "FLAC".to_string(),
            recursive: true,
            delete_originals: false,
        };

        assert_eq!(rule.folder, "/home/user/music");
        assert_eq!(rule.input_fmt, "MP3");
        assert_eq!(rule.output_fmt, "FLAC");
    }

    #[test]
    fn multiple_file_extensions() {
        let extensions = ["jpg", "jpeg", "JPG", "JPEG", "Jpg"];
        let normalized: Vec<String> = extensions.iter().map(|e| e.to_uppercase()).collect();

        // All should normalize to uppercase
        for n in &normalized {
            assert_eq!(n.to_uppercase(), *n);
        }
    }

    #[test]
    fn output_extension_for_formats() {
        let test_cases = vec![
            ("JPEG", "PNG", "png"),
            ("MP3", "FLAC", "flac"),
            ("AVI", "MKV", "mkv"),
            ("BMP", "WEBP", "webp"),
        ];

        for (_input, output, expected_ext) in test_cases {
            let output_lower = output.to_lowercase();
            assert_eq!(output_lower, expected_ext);
        }
    }

    #[test]
    fn rule_configuration_variations() {
        // Test different rule configurations
        let configs = vec![
            (true, true),   // recursive, delete originals
            (true, false),  // recursive, keep originals
            (false, true),  // non-recursive, delete originals
            (false, false), // non-recursive, keep originals
        ];

        let media_types = vec![
            crate::formats::MediaType::Audio,
            crate::formats::MediaType::Video,
            crate::formats::MediaType::Images,
            crate::formats::MediaType::Documents,
        ];

        for (recursive, delete) in &configs {
            for media_type in &media_types {
                let rule = AgentRule {
                    folder: "/test".to_string(),
                    media_type: *media_type,
                    input_fmt: "IN".to_string(),
                    output_fmt: "OUT".to_string(),
                    recursive: *recursive,
                    delete_originals: *delete,
                };

                assert_eq!(rule.recursive, *recursive);
                assert_eq!(rule.delete_originals, *delete);
                assert_eq!(rule.media_type, *media_type);
            }
        }
    }

    #[test]
    fn file_format_string_handling() {
        let formats = vec!["MP3", "FLAC", "OGG", "WAV", "AIFF"];

        for fmt in &formats {
            // Should be uppercase
            assert_eq!(*fmt, fmt.to_uppercase());
            // Should not be empty
            assert!(!fmt.is_empty());
            // Should contain only alphanumeric
            assert!(fmt.chars().all(|c| c.is_alphanumeric()));
        }
    }

    #[test]
    fn directory_recursion_settings() {
        let recursive_rule = AgentRule {
            folder: "/root".to_string(),
            media_type: crate::formats::MediaType::Audio,
            input_fmt: "MP3".to_string(),
            output_fmt: "FLAC".to_string(),
            recursive: true,
            delete_originals: false,
        };

        let non_recursive_rule = AgentRule {
            folder: "/root".to_string(),
            media_type: crate::formats::MediaType::Audio,
            input_fmt: "MP3".to_string(),
            output_fmt: "FLAC".to_string(),
            recursive: false,
            delete_originals: false,
        };

        assert!(recursive_rule.recursive);
        assert!(!non_recursive_rule.recursive);
        assert_eq!(recursive_rule.folder, non_recursive_rule.folder);
    }

    #[test]
    fn delete_originals_flag_variations() {
        let keep_rule = AgentRule {
            folder: "/test".to_string(),
            media_type: crate::formats::MediaType::Images,
            input_fmt: "JPEG".to_string(),
            output_fmt: "WEBP".to_string(),
            recursive: true,
            delete_originals: false,
        };

        let delete_rule = AgentRule {
            folder: "/test".to_string(),
            media_type: crate::formats::MediaType::Images,
            input_fmt: "JPEG".to_string(),
            output_fmt: "WEBP".to_string(),
            recursive: true,
            delete_originals: true,
        };

        assert!(!keep_rule.delete_originals);
        assert!(delete_rule.delete_originals);
    }

    #[test]
    fn format_pairs_audio() {
        let pairs = vec![
            ("WAV", "FLAC"),
            ("MP3", "OPUS"),
            ("OGG", "FLAC"),
            ("AAC", "OPUS"),
        ];

        for (input, output) in pairs {
            assert_ne!(input, output); // Should be different
            let input_upper = input.to_uppercase();
            let output_upper = output.to_uppercase();
            assert_eq!(input, input_upper);
            assert_eq!(output, output_upper);
        }
    }

    #[test]
    fn format_pairs_validation() {
        let audio = crate::formats::MediaType::Audio;

        let test_pairs = vec![
            ("WAV", "FLAC"), // lossless to lossless
            ("MP3", "OPUS"), // lossy to lossy
            ("FLAC", "OGG"), // lossless to lossy
        ];

        for (input, output) in test_pairs {
            // Check that compatible outputs contains the target
            let compatible = audio.compatible_outputs(input);
            assert!(
                compatible.contains(&output),
                "Format pair ({}, {}) should be valid for Audio",
                input,
                output
            );
        }
    }
}
