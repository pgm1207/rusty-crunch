mod agent;
mod config;
mod converter;
mod deps;
mod formats;
mod processor;
mod prompt;
mod util;

use anyhow::Result;
use clap::Parser;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rusty-crunch",
    version,
    about = "Fast parallel media converter"
)]
struct Cli {
    /// Simulate the run without converting anything
    #[arg(long)]
    dry_run: bool,

    /// Run the agent as a background service (reads rules from config)
    #[arg(long)]
    agent: bool,

    /// Stop a running background agent
    #[arg(long)]
    agent_stop: bool,

    /// Check if the background agent is currently running
    #[arg(long)]
    agent_status: bool,

    /// Check that all required tools are installed
    #[arg(long)]
    health_check: bool,

    /// Minimum file size (e.g., "10MB", "1GB"). Only process files larger than this.
    #[arg(long, value_name = "SIZE")]
    min_size: Option<String>,

    /// Maximum file size (e.g., "500MB", "2GB"). Only process files smaller than this.
    #[arg(long, value_name = "SIZE")]
    max_size: Option<String>,

    /// Folder to process (skips the directory browser)
    #[arg(value_name = "FOLDER")]
    folder: Option<PathBuf>,

    /// Print conversion summary as JSON to stdout
    #[arg(long)]
    json: bool,

    /// Disable colored output (also honours the NO_COLOR environment variable)
    #[arg(long, global = true)]
    no_color: bool,

    /// Non-interactive: skip every prompt and use config defaults (requires FOLDER)
    #[arg(short = 'y', long = "yes")]
    yes: bool,

    /// Non-interactive action: optimize (default), upscale, or restore
    #[arg(long, value_name = "MODE", value_parser = ["optimize", "upscale", "restore"])]
    mode: Option<String>,

    /// Scan sub-folders recursively (non-interactive; overrides the config default)
    #[arg(long)]
    recursive: bool,

    /// Do not scan sub-folders (non-interactive; overrides the config default)
    #[arg(long = "no-recursive", conflicts_with = "recursive")]
    no_recursive: bool,

    /// Delete originals after a successful conversion (non-interactive)
    #[arg(long = "delete-originals")]
    delete_originals: bool,

    /// Never remove original files, even if the saved config enables removal
    #[arg(long = "keep-originals", conflicts_with = "delete_originals")]
    keep_originals: bool,

    /// Maximum number of concurrent conversion jobs (default: config preset)
    #[arg(long, value_name = "N")]
    threads: Option<usize>,

    /// Write converted files into a named subfolder (non-interactive supported)
    #[arg(long = "output-subfolder", value_name = "NAME")]
    output_subfolder: Option<String>,

    /// Re-process files even if they were already optimized (non-interactive)
    #[arg(long = "force-recheck")]
    force_recheck: bool,

    /// Output quality for optimize: low, medium, or high
    #[arg(long, value_name = "QUALITY", value_parser = ["low", "medium", "high"])]
    quality: Option<String>,

    /// Target height for --mode upscale (e.g. 1080, 1440, 2160)
    #[arg(long, value_name = "HEIGHT")]
    target: Option<u32>,

    /// Upscale profile for --mode upscale: anime or movie
    #[arg(long, value_name = "PROFILE", value_parser = ["anime", "movie"])]
    preset: Option<String>,

    /// How to handle an existing output file (non-interactive): skip, overwrite, rename
    #[arg(long, value_name = "STRATEGY", value_parser = ["skip", "overwrite", "rename"])]
    conflict: Option<String>,

    /// Check for a newer release and exit
    #[arg(long = "check-update")]
    check_update: bool,

    /// Download and install the latest release, then exit
    #[arg(long = "self-update")]
    self_update: bool,

    /// List supported input/output formats and exit
    #[arg(long = "list-formats")]
    list_formats: bool,

    /// Print recorded conversion history and exit
    #[arg(long)]
    history: bool,

    /// Print cumulative conversion statistics and exit
    #[arg(long)]
    stats: bool,

    /// Send a desktop notification when a batch finishes
    #[arg(long)]
    notify: bool,

    /// Verbose diagnostics (also written to the log file)
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Suppress non-essential output
    #[arg(short = 'q', long)]
    quiet: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.threads.is_some_and(|n| n == 0 || n > 64) {
        anyhow::bail!("--threads must be between 1 and 64");
    }
    if let Some(name) = cli.output_subfolder.as_deref() {
        let mut parts = std::path::Path::new(name).components();
        if !matches!(parts.next(), Some(std::path::Component::Normal(_)))
            || parts.next().is_some()
            || name.contains(['\\', ':'])
            || name.contains("..")
            || name.trim() != name
        {
            anyhow::bail!("--output-subfolder must be a single folder name (not a path)");
        }
    }

    // Respect --no-color, the NO_COLOR convention, and non-TTY output.
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() || !console::user_attended() {
        console::set_colors_enabled(false);
    }

    util::set_verbose(cli.verbose);
    util::set_quiet(cli.quiet || cli.json);
    util::log_event(
        "INFO",
        &format!("rusty-crunch v{} started", env!("CARGO_PKG_VERSION")),
    );

    if cli.agent {
        return agent::run_headless();
    }
    if cli.agent_stop {
        return agent::stop_background();
    }
    if cli.agent_status {
        return agent::show_status();
    }
    if cli.health_check {
        return run_health_check();
    }
    if cli.list_formats {
        return list_formats();
    }
    if cli.check_update {
        return check_update_cmd();
    }
    if cli.self_update {
        return self_update_cmd();
    }
    if cli.history {
        return processor::print_history(cli.json);
    }
    if cli.stats {
        return processor::print_stats(cli.json);
    }

    // Non-interactive path: `--yes` (or an explicit `--mode`).
    if cli.yes || cli.mode.is_some() {
        return run_noninteractive(&cli);
    }

    loop {
        let display_mode = config::load().display_mode;
        if !util::is_quiet() {
            maybe_clear(display_mode);
            banner();
        }

        let agent_label = if cfg!(target_os = "macos") {
            "🤖 Agent Mode [ALPHA]"
        } else {
            "🤖 Agent Mode [BETA]"
        };

        let menu = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("What would you like to do?")
            .items(&[
                "✨ Optimize",
                "🪄 Upscale",
                "♻ Restore",
                agent_label,
                "⚙️  Settings",
                "📊 History & Statistics",
                "🔄 Check for Updates",
                "🚪 Exit",
            ])
            .default(0)
            .interact_opt()?;

        match menu {
            Some(0) => {
                let opt = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Choose optimize mode")
                    .items(&[
                        "🚀 Recommended (smart defaults)",
                        "🧰 Custom Optimize (manual)",
                        "↩ Back",
                    ])
                    .default(0)
                    .interact_opt()?;
                match opt {
                    Some(0) => run_recommended_crunch(&cli)?,
                    Some(1) => run_crunch(&cli, Some(prompt::CrunchMode::Standard))?,
                    _ => {}
                }
                pause_before_menu();
            }
            Some(1) => {
                let up = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Choose upscale mode")
                    .items(&[
                        "🚀 Recommended Upscale (auto-detect + smart rules)",
                        "🧰 Custom Upscale (manual)",
                        "↩ Back",
                    ])
                    .default(0)
                    .interact_opt()?;
                match up {
                    Some(0) => run_recommended_upscale(&cli)?,
                    Some(1) => run_crunch(&cli, Some(prompt::CrunchMode::UpscaleVideo))?,
                    _ => {}
                }
                pause_before_menu();
            }
            Some(2) => {
                processor::restore_last_session()?;
                pause_before_menu();
            }
            Some(3) => agent::setup()?,
            Some(4) => config::edit_settings()?,
            Some(5) => {
                let view = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("View activity")
                    .items(&["📜 Conversion history", "📊 Cumulative statistics", "↩ Back"])
                    .default(0)
                    .interact_opt()?;
                match view {
                    Some(0) => processor::print_history(false)?,
                    Some(1) => processor::print_stats(false)?,
                    _ => {}
                }
                pause_before_menu();
            }
            Some(6) => {
                check_for_updates()?;
                pause_before_menu();
            }
            _ => {
                println!("  {} Bye!\n", style("👋").cyan());
                break;
            }
        }
    }
    Ok(())
}

fn run_noninteractive(cli: &Cli) -> Result<()> {
    let cfg = config::load();
    let mode = cli.mode.as_deref().unwrap_or("optimize");

    if mode == "restore" {
        return processor::restore_last_session();
    }

    // Validate size filters up front (same rules as the interactive path).
    let min_size = cli.min_size.as_deref().and_then(util::parse_size);
    let max_size = cli.max_size.as_deref().and_then(util::parse_size);
    if cli.min_size.is_some() && min_size.is_none() {
        anyhow::bail!("Invalid --min-size format. Use: 10MB, 1GB, 512KB, etc.");
    }
    if cli.max_size.is_some() && max_size.is_none() {
        anyhow::bail!("Invalid --max-size format. Use: 500MB, 2GB, 100MB, etc.");
    }
    if min_size.zip(max_size).is_some_and(|(min, max)| min > max) {
        anyhow::bail!("--min-size must be less than or equal to --max-size");
    }

    let folder = cli
        .folder
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("FOLDER is required in non-interactive mode (--yes)"))?;
    let folder = if folder.is_relative() {
        std::env::current_dir()?.join(folder)
    } else {
        folder.clone()
    };
    if !folder.is_dir() {
        anyhow::bail!("Not a directory: {}", folder.display());
    }

    let recursive = if cli.recursive {
        true
    } else if cli.no_recursive {
        false
    } else {
        cfg.default_recursive
    };
    let delete = !cli.keep_originals && (cli.delete_originals || cfg.default_delete_originals);
    let conflict = match cli.conflict.as_deref() {
        Some("overwrite") => config::ConflictStrategy::Overwrite,
        Some("rename") => config::ConflictStrategy::Rename,
        Some("skip") => config::ConflictStrategy::Skip,
        _ => cfg.conflict_strategy,
    };
    let quality = match cli.quality.as_deref() {
        Some("low") => processor::Quality::Low,
        Some("medium") => processor::Quality::Medium,
        _ => processor::Quality::High,
    };
    let threads = cli.threads.unwrap_or_else(util::active_threads);
    let discovered = processor::scan_formats(&folder, recursive, cli.output_subfolder.as_deref());

    if !cli.json {
        println!(
            "\n  {} {} (non-interactive)",
            style("⚙\u{fe0f}").cyan(),
            style(mode).cyan().bold(),
        );
        println!(
            "  {} {}",
            style("Folder").dim(),
            style(folder.display()).white().bold(),
        );
        println!(
            "  {} recursive={}  delete_originals={}{}",
            style("Options").dim(),
            recursive,
            delete,
            if cli.dry_run { "  dry_run=true" } else { "" },
        );
        println!();
    }

    let mut summaries: Vec<processor::ConversionSummary> = Vec::new();

    if mode == "upscale" {
        let target_height = cli.target.unwrap_or(1080);
        let preset = match cli.preset.as_deref() {
            Some("anime") => processor::UpscalePreset::Anime,
            _ => processor::UpscalePreset::Movie,
        };
        let applicable: Vec<&str> = formats::MediaType::Video
            .formats()
            .iter()
            .copied()
            .filter(|f| processor::scanned_has_format(&discovered, f))
            .collect();
        if applicable.is_empty() {
            if !cli.json {
                println!("  {} No video files found.", style("\u{26a0}").yellow());
            }
            if cli.json {
                println!("[]");
            }
            return Ok(());
        }
        if !cli.dry_run {
            deps::check(formats::MediaType::Video)?;
        }
        for input_fmt in &applicable {
            let summary = processor::run(&processor::Job {
                folder: &folder,
                media_type: formats::MediaType::Video,
                input_fmt,
                output_fmt: "MKV",
                recursive,
                delete_originals: delete,
                force_recheck: cli.force_recheck,
                dry_run: cli.dry_run,
                threads,
                output_subfolder: cli.output_subfolder.as_deref(),
                min_file_size: min_size,
                max_file_size: max_size,
                conflict_strategy: conflict,
                normalize_audio: false,
                quality,
                keep_metadata: true,
                video_scale: processor::VideoScale::AutoTarget {
                    target_height,
                    preset,
                },
                image_scale: processor::ImageScale::Original,
            })?;
            summaries.push(summary);
        }
    } else {
        let applicable: Vec<(formats::MediaType, &str, &str)> = formats::recommended_conversions()
            .iter()
            .copied()
            .filter(|(_, input_fmt, _)| {
                processor::scanned_has_format(&discovered, input_fmt)
            })
            .collect();
        if applicable.is_empty() {
            if !cli.json {
                println!(
                    "  {} No files found that can be optimized.",
                    style("\u{26a0}").yellow()
                );
            }
            if cli.json {
                println!("[]");
            }
            return Ok(());
        }
        let mut ensured: Vec<formats::MediaType> = Vec::new();
        for &(mt, _, _) in &applicable {
            if !cli.dry_run && !ensured.contains(&mt) {
                deps::check(mt)?;
                ensured.push(mt);
            }
        }
        for &(mt, input_fmt, output_fmt) in &applicable {
            let summary = processor::run(&processor::Job {
                folder: &folder,
                media_type: mt,
                input_fmt,
                output_fmt,
                recursive,
                delete_originals: delete,
                force_recheck: cli.force_recheck,
                dry_run: cli.dry_run,
                threads,
                output_subfolder: cli.output_subfolder.as_deref(),
                min_file_size: min_size,
                max_file_size: max_size,
                conflict_strategy: conflict,
                normalize_audio: false,
                quality,
                keep_metadata: true,
                video_scale: processor::VideoScale::Original,
                image_scale: processor::ImageScale::Original,
            })?;
            summaries.push(summary);
        }
    }

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else {
        let converted: usize = summaries.iter().map(|s| s.files_converted).sum();
        let failed: usize = summaries.iter().map(|s| s.files_failed).sum();
        let saved: u64 = summaries.iter().map(|s| s.bytes_saved).sum();
        println!(
            "\n  {} {} file{} converted, {} failed, {} saved",
            style("\u{2714}").green().bold(),
            converted,
            if converted == 1 { "" } else { "s" },
            failed,
            util::human_bytes(saved),
        );
    }

    if !cli.dry_run && summaries.iter().any(|s| s.files_failed > 0) {
        maybe_notify(cli, &summaries);
        std::process::exit(1);
    }
    maybe_notify(cli, &summaries);
    Ok(())
}

fn run_crunch(cli: &Cli, forced_mode: Option<prompt::CrunchMode>) -> Result<()> {
    let cfg = config::load();

    let crunch_mode = match forced_mode {
        Some(m) => m,
        None => match prompt::select_crunch_mode()? {
            Some(m) => m,
            None => return Ok(()),
        },
    };
    ack("Mode", crunch_mode.label());

    match crunch_mode {
        prompt::CrunchMode::UpscaleVideo => {
            println!(
                "  {} {}",
                style("ℹ").cyan(),
                style("Upscale mode: choose input/output + 2x/3x/4x anime/movie profile.").dim(),
            );
            println!(
                "  {} {}",
                style("ℹ").cyan(),
                style("Tip: MKV → MKV usually preserves subtitles/audio streams best.").dim(),
            );
        }
        prompt::CrunchMode::Standard => {
            println!(
                "  {} {}",
                style("ℹ").cyan(),
                style("Standard mode: combine optimize/convert jobs across media types.").dim(),
            );
        }
    }

    // Parse size filters
    let min_size = cli.min_size.as_deref().and_then(util::parse_size);
    let max_size = cli.max_size.as_deref().and_then(util::parse_size);

    if cli.min_size.is_some() && min_size.is_none() {
        anyhow::bail!("Invalid --min-size format. Use: 10MB, 1GB, 512KB, etc.");
    }
    if cli.max_size.is_some() && max_size.is_none() {
        anyhow::bail!("Invalid --max-size format. Use: 500MB, 2GB, 100MB, etc.");
    }

    // ── Shared settings (asked once for all jobs) ───────────────────
    let folder = if let Some(ref f) = cli.folder {
        let f = if f.is_relative() {
            std::env::current_dir()?.join(f)
        } else {
            f.clone()
        };
        if !f.is_dir() {
            anyhow::bail!("Not a directory: {}", f.display());
        }
        ack("Folder", &f.display().to_string());
        f
    } else {
        match prompt::select_folder(&cfg)? {
            Some(f) => {
                ack("Folder", &f.display().to_string());
                f
            }
            None => return Ok(()),
        }
    };

    let recursive = match prompt::confirm_scan_subdirs(&cfg)? {
        Some(r) => {
            ack("Recursive", if r { "Yes" } else { "No" });
            r
        }
        None => return Ok(()),
    };

    let delete = match prompt::confirm_delete_originals(&cfg)? {
        Some(d) => {
            ack("Delete originals", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    let subfolder = match cli.output_subfolder.as_ref() {
        Some(sub) => Some(sub.clone()),
        None => prompt::select_output_destination()?,
    };
    if let Some(ref s) = subfolder {
        ack("Output sub-folder", s);
    }

    let force_recheck = match prompt::confirm_force_recheck()? {
        Some(d) => {
            ack("Force recheck", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    // ── Collect one or more conversion jobs ─────────────────────────
    struct JobSpec {
        media: formats::MediaType,
        input_fmt: &'static str,
        output_fmt: &'static str,
        normalize_audio: bool,
        quality: crate::processor::Quality,
        keep_metadata: bool,
        video_scale: crate::processor::VideoScale,
        image_scale: crate::processor::ImageScale,
    }

    let mut specs: Vec<JobSpec> = Vec::new();

    'outer: loop {
        let media = match crunch_mode {
            prompt::CrunchMode::UpscaleVideo => formats::MediaType::Video,
            prompt::CrunchMode::Standard => match prompt::select_media_type()? {
                Some(m) => m,
                None => break,
            },
        };

        // Lazy dep install — only if the tool is actually missing (never on a dry run)
        if !cli.dry_run {
            if let Err(e) = deps::ensure(media) {
                println!("\n  {} {}\n", style("✗").red(), style(e).red());
                continue;
            }
        }

        let raw_input = match prompt::select_input_format(media)? {
            Some(f) => f,
            None => {
                if crunch_mode == prompt::CrunchMode::UpscaleVideo {
                    return Ok(());
                }
                continue 'outer;
            }
        };

        let mut normalize_audio = false;
        if media == formats::MediaType::Audio {
            normalize_audio = prompt::confirm_audio_normalization()?;
            if normalize_audio {
                ack("Normalize Volume", "Yes (-af loudnorm)");
            }
        }

        // "All Lossless Audio → FLAC" and "All Lossy Audio → OPUS" batch shortcuts
        if raw_input == formats::LOSSLESS_AUDIO_SENTINEL {
            let quality = prompt::select_quality().unwrap_or(crate::processor::Quality::High);
            let keep_metadata = prompt::confirm_keep_metadata().unwrap_or(true);
            for &fmt in formats::LOSSLESS_AUDIO_INPUTS {
                specs.push(JobSpec {
                    media: formats::MediaType::Audio,
                    input_fmt: fmt,
                    output_fmt: "FLAC",
                    normalize_audio,
                    quality,
                    keep_metadata,
                    video_scale: crate::processor::VideoScale::Original,
                    image_scale: crate::processor::ImageScale::Original,
                });
            }
            ack(
                "Added",
                &format!("{} → FLAC", formats::LOSSLESS_AUDIO_INPUTS.join("/")),
            );
        } else if raw_input == formats::LOSSY_AUDIO_SENTINEL {
            let quality = prompt::select_quality().unwrap_or(crate::processor::Quality::High);
            let keep_metadata = prompt::confirm_keep_metadata().unwrap_or(true);
            for &fmt in formats::LOSSY_AUDIO_INPUTS {
                specs.push(JobSpec {
                    media: formats::MediaType::Audio,
                    input_fmt: fmt,
                    output_fmt: "OPUS",
                    normalize_audio,
                    quality,
                    keep_metadata,
                    video_scale: crate::processor::VideoScale::Original,
                    image_scale: crate::processor::ImageScale::Original,
                });
            }
            ack(
                "Added",
                &format!("{} → OPUS", formats::LOSSY_AUDIO_INPUTS.join("/")),
            );
        } else {
            let output_fmt = 'pick_out: loop {
                match prompt::select_output_format(media, raw_input)? {
                    None => {
                        if crunch_mode == prompt::CrunchMode::UpscaleVideo {
                            return Ok(());
                        }
                        continue 'outer;
                    }
                    Some(f) => match prompt::lossy_warning(media, raw_input, f)? {
                        Some(true) => break 'pick_out f,
                        Some(false) => continue,
                        None => {
                            if crunch_mode == prompt::CrunchMode::UpscaleVideo {
                                return Ok(());
                            }
                            continue 'outer;
                        }
                    },
                }
            };
            ack("Output format", output_fmt);
            specs.push(JobSpec {
                media,
                input_fmt: raw_input,
                output_fmt,
                normalize_audio,
                quality: prompt::select_quality()?,
                keep_metadata: prompt::confirm_keep_metadata()?,
                video_scale: if media == formats::MediaType::Video {
                    if crunch_mode == prompt::CrunchMode::UpscaleVideo {
                        prompt::select_video_upscale_preset()?
                    } else {
                        prompt::select_video_scale()?
                    }
                } else {
                    crate::processor::VideoScale::Original
                },
                image_scale: if media == formats::MediaType::Images {
                    prompt::select_image_scale()?
                } else {
                    crate::processor::ImageScale::Original
                },
            });
        }

        if crunch_mode == prompt::CrunchMode::UpscaleVideo {
            break;
        }

        // Cap at 8 jobs; ask about adding more
        if specs.len() >= 8 || !prompt::confirm_add_another()? {
            break;
        }
    }

    if specs.is_empty() {
        return Ok(());
    }

    // ── Summary ─────────────────────────────────────────────────────
    println!();
    let sep = style("─".repeat(50)).dim();
    println!("  {sep}");
    for s in &specs {
        println!(
            "  {} {} {} → {}",
            style("┃").dim(),
            style(s.media).cyan().bold(),
            style(s.input_fmt).white().bold(),
            style(s.output_fmt).green().bold(),
        );
    }
    println!(
        "  {} {:<18} {}",
        style("┃").dim(),
        style("Directory").dim(),
        style(folder.display()).white(),
    );
    let mut opts = format!("recursive={}  delete_originals={}", recursive, delete);
    if cli.dry_run {
        opts.push_str("  dry_run=true");
    }
    if let Some(ref s) = subfolder {
        opts.push_str(&format!("  sub-folder={s}"));
    }
    println!(
        "  {} {:<18} {}",
        style("┃").dim(),
        style("Options").dim(),
        style(&opts).white()
    );
    println!("  {sep}\n");

    match prompt::final_confirmation()? {
        Some(true) => {}
        _ => {
            println!("  {} Cancelled.", style("✗").red());
            return Ok(());
        }
    }

    let threads = cli.threads.unwrap_or_else(util::active_threads);

    // ── Run all jobs ─────────────────────────────────────────────────
    let mut summaries = Vec::new();
    for s in &specs {
        if specs.len() > 1 {
            println!(
                "\n  {} {} {} → {}",
                style("→").cyan().bold(),
                s.media.icon(),
                style(s.input_fmt).white().bold(),
                style(s.output_fmt).green().bold(),
            );
        }
        let summary = processor::run(&processor::Job {
            folder: &folder,
            media_type: s.media,
            input_fmt: s.input_fmt,
            output_fmt: s.output_fmt,
            recursive,
            delete_originals: delete,
            force_recheck,
            dry_run: cli.dry_run,
            threads,
            output_subfolder: subfolder.as_deref(),
            min_file_size: min_size,
            max_file_size: max_size,
            conflict_strategy: cfg.conflict_strategy,
            normalize_audio: s.normalize_audio,
            quality: s.quality,
            keep_metadata: s.keep_metadata,
            video_scale: s.video_scale,
            image_scale: s.image_scale,
        })?;
        summaries.push(summary);
    }

    if cli.json {
        if let Ok(j) = serde_json::to_string_pretty(&summaries) {
            println!("{}", j);
        }
    }

    maybe_notify(cli, &summaries);
    println!("\n  {} Done!", style("✔").green().bold());
    Ok(())
}

fn run_recommended_upscale(cli: &Cli) -> Result<()> {
    let cfg = config::load();

    println!(
        "\n  {} {}\n",
        style("🪄").cyan(),
        style("Recommended Upscale").cyan().bold(),
    );
    println!(
        "  {}",
        style("Auto-detects video formats and applies smart integer upscale rules.").dim(),
    );
    println!(
        "  {} Target resolution is selected once; each file gets the closest 2x/3x/4x multiplier.",
        style("·").dim(),
    );
    println!(
        "  {} Keeps streams in MKV for best subtitle/audio preservation.",
        style("·").dim()
    );
    println!();

    let folder = if let Some(ref f) = cli.folder {
        let f = if f.is_relative() {
            std::env::current_dir()?.join(f)
        } else {
            f.clone()
        };
        if !f.is_dir() {
            anyhow::bail!("Not a directory: {}", f.display());
        }
        ack("Folder", &f.display().to_string());
        f
    } else {
        match prompt::select_folder(&cfg)? {
            Some(f) => {
                ack("Folder", &f.display().to_string());
                f
            }
            None => return Ok(()),
        }
    };

    let recursive = match prompt::confirm_scan_subdirs(&cfg)? {
        Some(r) => {
            ack("Recursive", if r { "Yes" } else { "No" });
            r
        }
        None => return Ok(()),
    };

    let delete = match prompt::confirm_delete_originals(&cfg)? {
        Some(d) => {
            ack("Delete originals", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    let subfolder = match cli.output_subfolder.as_ref() {
        Some(sub) => Some(sub.clone()),
        None => prompt::select_output_destination()?,
    };
    if let Some(ref s) = subfolder {
        ack("Output sub-folder", s);
    }

    let force_recheck = match prompt::confirm_force_recheck()? {
        Some(d) => {
            ack("Force recheck", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    let Some(target_height) = prompt::select_recommended_upscale_target()? else {
        return Ok(());
    };
    ack("Target", &format!("{}p", target_height));

    let Some(preset) = prompt::select_recommended_upscale_preset()? else {
        return Ok(());
    };
    ack(
        "Profile",
        match preset {
            crate::processor::UpscalePreset::Anime => "Anime",
            crate::processor::UpscalePreset::Movie => "Movie",
        },
    );

    let discovered = processor::scan_formats(&folder, recursive, subfolder.as_deref());
    let video_inputs = formats::MediaType::Video.formats();
    let applicable: Vec<&str> = video_inputs
        .iter()
        .copied()
        .filter(|input_fmt| processor::scanned_has_format(&discovered, input_fmt))
        .collect();

    if applicable.is_empty() {
        println!(
            "\n  {} No video files found in {}",
            style("⚠").yellow(),
            style(folder.display()).dim(),
        );
        return Ok(());
    }

    println!();
    let sep = style("─".repeat(50)).dim();
    println!("  {sep}");
    for inf in &applicable {
        println!(
            "  {} {} {} → {}  {}",
            style("┃").dim(),
            formats::MediaType::Video.icon(),
            style(*inf).white().bold(),
            style("MKV").green().bold(),
            style(format!("(auto {}p)", target_height)).dim(),
        );
    }
    if cli.dry_run {
        println!("  {} {}", style("┃").dim(), style("dry_run=true").white());
    }
    println!("  {sep}\n");

    match prompt::final_confirmation()? {
        Some(true) => {}
        _ => {
            println!("  {} Cancelled.", style("✗").red());
            return Ok(());
        }
    }

    if !cli.dry_run {
        deps::ensure(formats::MediaType::Video)?;
    }

    let threads = cli.threads.unwrap_or_else(util::active_threads);
    let mut summaries = Vec::new();
    for input_fmt in &applicable {
        println!(
            "\n  {} {} → {}",
            style("→").cyan().bold(),
            style(*input_fmt).white().bold(),
            style("MKV").green().bold(),
        );
        let summary = processor::run(&processor::Job {
            folder: &folder,
            media_type: formats::MediaType::Video,
            input_fmt,
            output_fmt: "MKV",
            recursive,
            delete_originals: delete,
            force_recheck,
            dry_run: cli.dry_run,
            threads,
            output_subfolder: subfolder.as_deref(),
            min_file_size: cli.min_size.as_deref().and_then(util::parse_size),
            max_file_size: cli.max_size.as_deref().and_then(util::parse_size),
            conflict_strategy: cfg.conflict_strategy,
            normalize_audio: false,
            quality: crate::processor::Quality::Medium,
            keep_metadata: true,
            video_scale: crate::processor::VideoScale::AutoTarget {
                target_height,
                preset,
            },
            image_scale: crate::processor::ImageScale::Original,
        })?;
        summaries.push(summary);
    }

    if cli.json {
        if let Ok(j) = serde_json::to_string_pretty(&summaries) {
            println!("{}", j);
        }
    }

    maybe_notify(cli, &summaries);
    println!(
        "\n  {} Recommended Upscale complete!",
        style("✔").green().bold()
    );
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn banner() {
    println!();
    println!(
        "  {}  {}",
        style("🔧").cyan(),
        style("rusty-crunch").cyan().bold(),
    );
    println!(
        "     {}",
        style("optimize · upscale · restore · media workflows").dim(),
    );
    println!();
}

fn ack(label: &str, value: &str) {
    println!(
        "  {} {:<16} {}",
        style("✓").green(),
        style(label).dim(),
        style(value).white().bold(),
    );
}

fn maybe_clear(mode: config::DisplayMode) {
    if mode == config::DisplayMode::Clean {
        // Use the console crate's clear which handles Windows and Unix.
        // On Windows cmd.exe we also try the `cls` fallback.
        let term = console::Term::stdout();
        if term.clear_screen().is_err() {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "cls"])
                    .status();
            }
        }
    }
}

/// Send a desktop notification summarising a finished batch, if requested.
fn maybe_notify(cli: &Cli, summaries: &[processor::ConversionSummary]) {
    if !cli.notify {
        return;
    }
    let converted: usize = summaries.iter().map(|s| s.files_converted).sum();
    if converted == 0 {
        return;
    }
    let failed: usize = summaries.iter().map(|s| s.files_failed).sum();
    let saved: u64 = summaries.iter().map(|s| s.bytes_saved).sum();
    util::notify(
        "rusty-crunch",
        &format!(
            "{converted} file(s) converted, {failed} failed, {} reclaimed",
            util::human_bytes(saved)
        ),
    );
}

/// Pause so the user can read results before the screen clears.
fn pause_before_menu() {
    use std::io::{self, Write};
    println!();
    print!("  Press Enter to return to the menu...");
    let _ = io::stdout().flush();
    let _ = io::stdin().read_line(&mut String::new());
}

fn run_recommended_crunch(cli: &Cli) -> Result<()> {
    let cfg = config::load();

    println!(
        "\n  {} {}\n",
        style("🚀").cyan(),
        style("Recommended Crunch").cyan().bold(),
    );
    println!(
        "  {}",
        style("Converts files to the most efficient format per type:").dim(),
    );
    println!(
        "  {} Audio: WAV/AIFF → FLAC · MP3/OGG/AAC/M4A/WMA → OPUS",
        style("·").dim(),
    );
    println!("  {} Video: AVI/MOV/FLV/WMV/TS → MKV", style("·").dim(),);
    println!(
        "  {} Images: BMP/TIFF/ICO/GIF → PNG · JPEG → AVIF",
        style("·").dim(),
    );
    println!("  {} Documents: PDF → PDF (Optimized)", style("·").dim(),);
    println!();

    // ── Folder ──────────────────────────────────────────────────────
    let folder = if let Some(ref f) = cli.folder {
        let f = if f.is_relative() {
            std::env::current_dir()?.join(f)
        } else {
            f.clone()
        };
        if !f.is_dir() {
            anyhow::bail!("Not a directory: {}", f.display());
        }
        ack("Folder", &f.display().to_string());
        f
    } else {
        match prompt::select_folder(&cfg)? {
            Some(f) => {
                ack("Folder", &f.display().to_string());
                f
            }
            None => return Ok(()),
        }
    };

    // ── Recursive ───────────────────────────────────────────────────
    let recursive = match prompt::confirm_scan_subdirs(&cfg)? {
        Some(r) => {
            ack("Recursive", if r { "Yes" } else { "No" });
            r
        }
        None => return Ok(()),
    };

    // ── Delete originals ────────────────────────────────────────────
    let delete = match prompt::confirm_delete_originals(&cfg)? {
        Some(d) => {
            ack("Delete originals", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    let subfolder = match cli.output_subfolder.as_ref() {
        Some(sub) => Some(sub.clone()),
        None => prompt::select_output_destination()?,
    };
    if let Some(ref s) = subfolder {
        ack("Output sub-folder", s);
    }

    let force_recheck = match prompt::confirm_force_recheck()? {
        Some(d) => {
            ack("Force recheck", if d { "Yes" } else { "No" });
            d
        }
        None => return Ok(()),
    };

    // ── Scan for applicable conversions ─────────────────────────────
    let discovered = processor::scan_formats(&folder, recursive, subfolder.as_deref());
    let all_conversions = formats::recommended_conversions();
    let applicable: Vec<(formats::MediaType, &str, &str)> = all_conversions
        .iter()
        .copied()
        .filter(|(_, input_fmt, _)| processor::scanned_has_format(&discovered, input_fmt))
        .collect();

    if applicable.is_empty() {
        println!(
            "\n  {} No files found that can be optimized in {}",
            style("⚠").yellow(),
            style(folder.display()).dim(),
        );
        return Ok(());
    }

    // ── Summary ─────────────────────────────────────────────────────
    println!();
    let sep = style("─".repeat(50)).dim();
    println!("  {sep}");
    for &(mt, inf, outf) in &applicable {
        println!(
            "  {} {} {} → {}",
            style("┃").dim(),
            mt.icon(),
            style(inf).white().bold(),
            style(outf).green().bold(),
        );
    }
    if cli.dry_run {
        println!("  {} {}", style("┃").dim(), style("dry_run=true").white(),);
    }
    println!("  {sep}");
    println!();

    // ── Confirm ─────────────────────────────────────────────────────
    match prompt::final_confirmation()? {
        Some(true) => {}
        _ => {
            println!("  {} Cancelled.", style("✗").red());
            return Ok(());
        }
    }

    // ── Ensure dependencies ─────────────────────────────────────────
    let mut ensured: Vec<formats::MediaType> = Vec::new();
    for &(mt, _, _) in &applicable {
        if !cli.dry_run && !ensured.contains(&mt) {
            deps::ensure(mt)?;
            ensured.push(mt);
        }
    }

    // ── Run conversions ─────────────────────────────────────────────
    let threads = cli.threads.unwrap_or_else(util::active_threads);
    let mut summaries = Vec::new();
    for &(media_type, input_fmt, output_fmt) in &applicable {
        println!(
            "\n  {} {} → {}",
            style("→").cyan().bold(),
            style(input_fmt).white().bold(),
            style(output_fmt).green().bold(),
        );
        let summary = processor::run(&processor::Job {
            folder: &folder,
            media_type,
            input_fmt,
            output_fmt,
            recursive,
            delete_originals: delete,
            force_recheck,
            dry_run: cli.dry_run,
            threads,
            output_subfolder: subfolder.as_deref(),
            min_file_size: None,
            max_file_size: None,
            conflict_strategy: cfg.conflict_strategy,
            normalize_audio: false,
            quality: crate::processor::Quality::High,
            keep_metadata: true,
            video_scale: crate::processor::VideoScale::Original,
            image_scale: crate::processor::ImageScale::Original,
        })?;
        summaries.push(summary);
    }

    if cli.json {
        if let Ok(j) = serde_json::to_string_pretty(&summaries) {
            println!("{}", j);
        }
    }

    maybe_notify(cli, &summaries);
    println!(
        "\n  {} Recommended Crunch complete!",
        style("✔").green().bold(),
    );
    Ok(())
}

fn check_for_updates() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!(
        "\n  {} Checking for updates (current: v{}) \u{2026}",
        style("🔄").cyan(),
        style(current).dim(),
    );

    match util::check_for_update() {
        Err(e) => {
            println!("  {} {}\n", style("⚠").yellow(), style(e).dim());
        }
        Ok(None) => {
            println!(
                "  {} Already up to date (v{})\n",
                style("✓").green(),
                current,
            );
        }
        Ok(Some(latest)) => {
            println!(
                "  {} Update available: v{}\n",
                style("🆕").cyan(),
                style(&latest).cyan().bold(),
            );
            let prompt = format!("Update from v{current} to v{latest}?");
            use dialoguer::theme::ColorfulTheme;
            use dialoguer::Confirm;
            match Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(&prompt)
                .default(true)
                .interact_opt()?
            {
                Some(true) => util::download_and_install_update(&latest)?,
                _ => println!("  {} Skipped\n", style("·").dim()),
            }
        }
    }
    Ok(())
}

/// `--list-formats`: print every supported type and its I/O formats.
fn list_formats() -> Result<()> {
    println!();
    for mt in formats::MediaType::ALL {
        println!("  {}  {}", mt.icon(), style(mt.label()).bold());
        println!("     input : {}", mt.formats().join(", "));
        let mut outs: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for i in mt.formats() {
            for o in mt.compatible_outputs(i) {
                outs.insert(o);
            }
        }
        println!(
            "     output: {}",
            outs.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    println!();
    Ok(())
}

/// `--check-update`: report a newer release without installing it.
fn check_update_cmd() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    match util::check_for_update() {
        Ok(Some(v)) => println!("Update available: v{v} (current v{current})"),
        Ok(None) => println!("Already up to date (v{current})"),
        Err(e) => {
            eprintln!("Update check failed: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// `--self-update`: download and install the latest release.
fn self_update_cmd() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    match util::check_for_update() {
        Ok(Some(v)) => {
            println!("Updating v{current} \u{2192} v{v}\u{2026}");
            util::download_and_install_update(&v)
        }
        Ok(None) => {
            println!("Already up to date (v{current})");
            Ok(())
        }
        Err(e) => {
            eprintln!("Update check failed: {e}");
            std::process::exit(1);
        }
    }
}

fn run_health_check() -> Result<()> {
    println!(
        "\n  {} {}\n",
        style("🔍").cyan(),
        style("System Health Check").cyan().bold()
    );

    let mut issues = Vec::new();

    // Check ffmpeg (used by audio/video)
    if !util::has("ffmpeg") {
        issues.push(("ffmpeg", "Audio/Video conversion"));
    }

    // Check ImageMagick (used by images)
    if !util::has_magick() {
        issues.push(("ImageMagick (magick)", "Image conversion"));
    }

    // Check Ghostscript (used by PDF)
    if !util::has_gs() {
        issues.push(("Ghostscript (gs)", "PDF optimization"));
    }

    // Check LibreOffice (used by documents)
    if !util::has_lo() {
        issues.push(("LibreOffice", "Document conversion"));
    }

    if issues.is_empty() {
        println!(
            "  {} All required tools are installed:\n",
            style("✓").green()
        );
        println!("  {} ffmpeg — audio/video", style("✓").green());
        println!("  {} ImageMagick — images", style("✓").green());
        println!("  {} Ghostscript — PDF", style("✓").green());
        println!("  {} LibreOffice — documents", style("✓").green());
        println!(
            "\n  {} System is ready for conversions\n",
            style("✓").green().bold()
        );
        return Ok(());
    }

    println!(
        "  {} {} tool{} missing:\n",
        style("✗").red(),
        issues.len(),
        if issues.len() == 1 { "" } else { "s" }
    );
    for (tool, purpose) in &issues {
        println!(
            "  {} {} — {}",
            style("✗").red(),
            style(tool).white().bold(),
            purpose
        );
    }

    println!("\n  {} Install missing tools:\n", style("ℹ").cyan());
    println!(
        "  {} rusty-crunch can auto-install on supported systems",
        style("·").dim()
    );

    #[cfg(target_os = "windows")]
    {
        println!("  {} Manual install (PowerShell):", style("·").dim());
        println!(
            "    {}",
            style("winget install Gyan.FFmpeg ImageMagick.ImageMagick TheDocumentFoundation.LibreOffice").dim()
        );
        println!(
            "  {} Ghostscript: https://ghostscript.com/releases/gsdnld.html",
            style("·").dim()
        );
        println!(
            "  {} Restart terminal after installing so PATH updates are detected\n",
            style("·").dim()
        );
    }

    #[cfg(not(target_os = "windows"))]
    {
        println!(
            "  {} Or install manually: https://github.com/pgm1207/rusty-crunch#installation\n",
            style("·").dim()
        );
    }

    // Non-zero exit so scripts/CI can detect a missing dependency.
    std::process::exit(1);
}
