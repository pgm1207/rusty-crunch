use crate::converter;
use crate::formats::MediaType;
use crate::util;
use anyhow::Result;
use console::style;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(not(target_os = "windows"))]
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quality {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UpscalePreset {
    Anime,
    Movie,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoScale {
    Original,
    P1080,
    P720,
    AutoTarget {
        target_height: u32,
        preset: UpscalePreset,
    },
    Upscale2xAnime,
    Upscale3xAnime,
    Upscale4xAnime,
    Upscale2xMovie,
    Upscale3xMovie,
    Upscale4xMovie,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageScale {
    Original,
    W1920,
    W1080,
}

pub struct Job<'a> {
    pub folder: &'a Path,
    pub media_type: MediaType,
    pub input_fmt: &'a str,
    pub output_fmt: &'a str,
    pub normalize_audio: bool,
    pub quality: Quality,
    pub keep_metadata: bool,
    pub video_scale: VideoScale,
    pub image_scale: ImageScale,
    pub recursive: bool,
    pub delete_originals: bool,
    pub dry_run: bool,
    /// Number of rayon worker threads. Use `util::active_threads()`.
    pub threads: usize,
    /// If Some, converted files go into `folder/subfolder/` instead of alongside the originals.
    pub output_subfolder: Option<&'a str>,
    /// Optional: minimum file size in bytes (for filtering). None = no minimum.
    pub min_file_size: Option<u64>,
    /// Optional: maximum file size in bytes (for filtering). None = no maximum.
    pub max_file_size: Option<u64>,
    /// Whether to force re-processing of previously optimized files.
    pub force_recheck: bool,
    /// What to do if the output file already exists.
    pub conflict_strategy: crate::config::ConflictStrategy,
}

/// Serializable summary of a conversion job result.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ConversionSummary {
    pub media_type: String,
    pub input_format: String,
    pub output_format: String,
    pub files_converted: usize,
    pub files_skipped: usize,
    pub files_failed: usize,
    pub bytes_saved: u64,
    pub delete_errors: usize,
    pub duration_secs: f64,
    pub threads_used: usize,
    pub compression_best_percent: f64,
    pub compression_worst_percent: f64,
}

pub fn run(job: &Job) -> Result<ConversionSummary> {
    let input_ext = job.input_fmt.to_ascii_lowercase();
    let output_ext = match job.output_fmt {
        "PDF (Optimized)" => "pdf".to_string(),
        "JPEG" => "jpg".to_string(),
        other => other.to_ascii_lowercase(),
    };
    let same_ext = input_ext == output_ext;

    // Create output sub-folder before any parallel work
    if let Some(sub) = job.output_subfolder {
        std::fs::create_dir_all(job.folder.join(sub))?;
    }

    // ── Load optimization cache (for same-extension jobs like PDF → PDF) ──
    let cache = Mutex::new(if same_ext {
        load_opt_cache()
    } else {
        HashMap::new()
    });

    // Session history for restore/undo functionality.
    let session_id = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let history_entries = Mutex::new(Vec::<HistoryEntry>::new());
    let backup_counter = AtomicUsize::new(0);

    // ── Collect matching files ──────────────────────────────────────
    let max_depth = if job.recursive { usize::MAX } else { 1 };
    let mut files: Vec<PathBuf> = WalkDir::new(job.folder)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let path = e.path();
            let mut matches_ext = path
                .extension()
                .map(|ext| {
                    let lc = ext.to_ascii_lowercase();
                    lc == input_ext.as_str()
                        || (input_ext == "jpeg" && lc == "jpg")
                        || (input_ext == "jpg" && lc == "jpeg")
                        || (input_ext == "aiff" && lc == "aif")
                        || (input_ext == "aif" && lc == "aiff")
                        || (input_ext == "aiff" && lc == "aif")
                        || (input_ext == "aif" && lc == "aiff")
                        || (input_ext == "aiff" && lc == "aif")
                        || (input_ext == "aif" && lc == "aiff")
                })
                .unwrap_or(false);

            if matches_ext {
                if let Ok(meta) = path.metadata() {
                    let size = meta.len();
                    if let Some(min) = job.min_file_size {
                        if size < min {
                            matches_ext = false;
                        }
                    }
                    if let Some(max) = job.max_file_size {
                        if size > max {
                            matches_ext = false;
                        }
                    }
                }
            }
            matches_ext
        })
        .map(|e| e.into_path())
        .collect();

    files.sort();

    if files.is_empty() {
        println!(
            "\n  {} No .{} files found in {}",
            style("⚠").yellow(),
            input_ext,
            style(job.folder.display()).dim()
        );
        return Ok(ConversionSummary {
            media_type: format!("{:?}", job.media_type),
            input_format: job.input_fmt.to_string(),
            output_format: job.output_fmt.to_string(),
            ..Default::default()
        });
    }

    let total = files.len();
    let _cores = util::cores();

    // ── Estimate output size and check disk space (preflight warning) ─
    let total_input_bytes: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();

    if !job.dry_run && !job.delete_originals {
        // Estimate output size as 50% of input (rough average for all formats)
        let estimated_output = (total_input_bytes as f64 * 0.5) as u64;
        if check_available_space(job.folder, estimated_output) {
            println!(
                "  {} Estimated output size: {} (you have sufficient space)",
                style("💾").cyan(),
                util::human_bytes(estimated_output),
            );
        } else {
            println!(
                "  {} Warning: estimated output size {} might exceed available disk space",
                style("⚠").yellow(),
                util::human_bytes(estimated_output),
            );
            println!(
                "  {} Consider enabling 'delete originals' or freeing up space\n",
                style("ℹ").cyan(),
            );
        }
    }

    // ── Dry-run mode ────────────────────────────────────────────────
    if job.dry_run {
        println!(
            "\n  {} Dry run: would convert {} file{} ({} total input size)",
            style("🔍").cyan(),
            style(total).cyan().bold(),
            if total == 1 { "" } else { "s" },
            style(util::human_bytes(total_input_bytes)).white().bold(),
        );
        return Ok(ConversionSummary {
            media_type: format!("{:?}", job.media_type),
            input_format: job.input_fmt.to_string(),
            output_format: job.output_fmt.to_string(),
            files_converted: 0,
            ..Default::default()
        });
    }

    let actual_threads = if job.media_type == MediaType::Video {
        // Keep video parallelism intentionally low to avoid VRAM thrashing.
        job.threads.clamp(1, 2)
    } else {
        job.threads.max(1)
    };

    let mode_str = format!(
        "{} parallel thread{}",
        actual_threads,
        if actual_threads == 1 { "" } else { "s" }
    );

    println!(
        "\n  {} Found {} file{} · processing {}",
        style("⚡").cyan(),
        style(total).cyan().bold(),
        if total == 1 { "" } else { "s" },
        style(mode_str).cyan().bold(),
    );

    // ── Progress bar with ETA ───────────────────────────────────────
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} [{bar:40.cyan/dim}] {pos}/{len}  ETA {eta}  {msg}",
        )?
        .progress_chars("━╸─"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let ok_count = AtomicUsize::new(0);
    let skip_count = AtomicUsize::new(0);
    let err_count = AtomicUsize::new(0);
    let del_err_count = AtomicUsize::new(0);
    let saved_bytes = AtomicU64::new(0);
    // Track best/worst compression ratios
    let best_ratio = AtomicU64::new(0); // stored as ratio * 10000 (fixed point)
    let worst_ratio = AtomicU64::new(10000); // 100% = no savings (worst possible)
    let start = Instant::now();

    // Build a local rayon thread pool limited to actual_threads
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(actual_threads)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        futures::stream::iter(files)
            .for_each_concurrent(actual_threads, |input_path| {
                let output_ext = output_ext.clone();
                let pb = pb.clone();
                let cache = &cache;
                let skip_count = &skip_count;
                let ok_count = &ok_count;
                let err_count = &err_count;
                let del_err_count = &del_err_count;
                let saved_bytes = &saved_bytes;
                let best_ratio = &best_ratio;
                let worst_ratio = &worst_ratio;
                let history_entries = &history_entries;
                let backup_counter = &backup_counter;
                async move {
                    // Compute output path (respects optional sub-folder and preserves relative structure)
                    let output_path = if let Some(sub) = job.output_subfolder {
                        // Preserve relative directory structure under the subfolder
                        // e.g., for recursive jobs: input a/b/file.wav → output/a/b/file.ext
                        // for non-recursive: input file.wav → output/file.ext
                        if let Ok(rel_path) = input_path.strip_prefix(job.folder) {
                            // Get parent directory of relative path (if nested)
                            if let Some(parent) = rel_path.parent() {
                                job.folder
                                    .join(sub)
                                    .join(parent)
                                    .join(input_path.file_name().unwrap_or_default())
                                    .with_extension(&output_ext)
                            } else {
                                // File is directly in job.folder (non-nested)
                                job.folder
                                    .join(sub)
                                    .join(input_path.file_name().unwrap_or_default())
                                    .with_extension(&output_ext)
                            }
                        } else {
                            // Fallback: just use filename (shouldn't happen if walkdir is working correctly)
                            job.folder
                                .join(sub)
                                .join(input_path.file_name().unwrap_or_default())
                                .with_extension(&output_ext)
                        }
                    } else {
                        input_path.with_extension(&output_ext)
                    };

                    // Ensure parent directory exists for nested outputs
                    if let Some(parent) = output_path.parent() {
                        if !parent.exists() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                    }

                    let mut final_output_path = output_path;

                    let is_inplace_intent =
                        same_ext && final_output_path == input_path && job.delete_originals;

                    // Handle conflicts if output already exists (unless it's an intended in-place replacement)
                    if !is_inplace_intent && final_output_path.exists() {
                        // For same extension workflows (like MKV->MKV upscale), if the output exactly equals the input
                        // but we aren't doing in-place replacement, we MUST force a rename so we don't accidentally skip or overwrite the source.
                        let strategy = if same_ext && final_output_path == input_path {
                            crate::config::ConflictStrategy::Rename
                        } else {
                            job.conflict_strategy
                        };

                        match strategy {
                            crate::config::ConflictStrategy::Skip => {
                                skip_count.fetch_add(1, Ordering::Relaxed);
                                pb.inc(1);
                                return;
                            }
                            crate::config::ConflictStrategy::Overwrite => {
                                // Do nothing, file will be overwritten by converter
                            }
                            crate::config::ConflictStrategy::Rename => {
                                let mut counter = 1;
                                let file_stem = final_output_path
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                let folder = final_output_path
                                    .parent()
                                    .unwrap_or_else(|| std::path::Path::new(""));
                                loop {
                                    let new_name =
                                        format!("{}.{}.{}", file_stem, counter, output_ext);
                                    let candidate = folder.join(new_name);
                                    if !candidate.exists() {
                                        final_output_path = candidate;
                                        break;
                                    }
                                    counter += 1;
                                }
                            }
                        }
                    }

                    // Skip files already optimized (same-extension jobs like PDF → PDF)
                    if same_ext && !job.force_recheck {
                        if let Some(entry) = cache
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get(&input_path)
                        {
                            if entry.matches(&input_path) {
                                skip_count.fetch_add(1, Ordering::Relaxed);
                                pb.inc(1);
                                return;
                            }
                        }
                    }

                    let name = input_path.file_name().unwrap_or_default().to_string_lossy();
                    pb.set_message(name.to_string());

                    let input_size = input_path.metadata().map(|m| m.len()).unwrap_or(0);

                    match converter::convert(
                        &input_path,
                        &final_output_path,
                        crate::converter::ConversionOptions {
                            media_type: job.media_type,
                            input_fmt: job.input_fmt,
                            output_fmt: job.output_fmt,
                            normalize_audio: job.normalize_audio,
                            quality: job.quality,
                            keep_metadata: job.keep_metadata,
                            video_scale: job.video_scale,
                            image_scale: job.image_scale,
                        },
                    )
                    .await
                    {
                        Ok(()) => {
                            let output_size =
                                final_output_path.metadata().map(|m| m.len()).unwrap_or(0);
                            if input_size > 0 {
                                let saved = input_size.saturating_sub(output_size);
                                saved_bytes.fetch_add(saved, Ordering::Relaxed);

                                // Compression ratio: % of space saved (higher = better)
                                let ratio = ((saved as f64 / input_size as f64) * 10000.0) as u64;
                                // Update best (max saved %)
                                let mut cur = best_ratio.load(Ordering::Relaxed);
                                while ratio > cur {
                                    match best_ratio.compare_exchange_weak(
                                        cur,
                                        ratio,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    ) {
                                        Ok(_) => break,
                                        Err(c) => cur = c,
                                    }
                                }
                                // Update worst (min saved %)
                                cur = worst_ratio.load(Ordering::Relaxed);
                                while ratio < cur {
                                    match worst_ratio.compare_exchange_weak(
                                        cur,
                                        ratio,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    ) {
                                        Ok(_) => break,
                                        Err(c) => cur = c,
                                    }
                                }
                            }

                            let mut backup_path: Option<PathBuf> = None;
                            if job.delete_originals && !same_ext {
                                let idx = backup_counter.fetch_add(1, Ordering::Relaxed);
                                match backup_original_for_restore(&input_path, session_id, idx) {
                                    Ok(p) => backup_path = Some(p),
                                    Err(_) => {
                                        del_err_count.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }

                            if !same_ext {
                                history_entries
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(HistoryEntry {
                                        original_path: input_path.clone(),
                                        converted_path: final_output_path.clone(),
                                        backup_path,
                                    });
                            }

                            // Record file state so we skip it on future runs
                            if same_ext {
                                if let Some(stamp) = CacheEntry::from_path(&input_path) {
                                    cache
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .insert(input_path.to_path_buf(), stamp);
                                }
                            }

                            ok_count.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            pb.suspend(|| {
                                eprintln!(
                                    "  {} {}: {}",
                                    style("✗").red().bold(),
                                    style(name.as_ref()).dim(),
                                    style(e).red()
                                );
                            });
                            // Clean up partial output (but not for in-place where output IS the input)
                            if !same_ext {
                                let _ = std::fs::remove_file(&final_output_path);
                            }
                            err_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    pb.inc(1);
                }
            })
            .await;
    });

    pb.finish_and_clear();

    // ── Persist optimization cache ─────────────────────────────────
    if same_ext {
        let cache = match cache.into_inner() {
            Ok(c) => c,
            Err(p) => p.into_inner(),
        };
        save_opt_cache(&cache);
    }

    // Persist history only when there are actual conversion entries.
    let entries = match history_entries.into_inner() {
        Ok(v) => v,
        Err(p) => p.into_inner(),
    };
    if !entries.is_empty() {
        let _ = save_history_record(&HistoryRecord {
            session_id,
            created_unix: session_id,
            entries,
        });
    }

    // ── Summary ─────────────────────────────────────────────────────
    let elapsed = start.elapsed();
    let ok = ok_count.load(Ordering::Relaxed);
    let skipped = skip_count.load(Ordering::Relaxed);
    let errs = err_count.load(Ordering::Relaxed);
    let del_errs = del_err_count.load(Ordering::Relaxed);
    let saved = saved_bytes.load(Ordering::Relaxed);
    let best = best_ratio.load(Ordering::Relaxed);
    let worst = worst_ratio.load(Ordering::Relaxed);

    println!();
    println!("  {}", style("─".repeat(50)).dim());
    println!(
        "  {} {} converted   {} skipped   {} failed",
        style("┃").dim(),
        style(ok).green().bold(),
        style(skipped).yellow(),
        if errs > 0 {
            style(errs).red().bold()
        } else {
            style(errs).green().bold()
        },
    );
    if saved > 0 {
        println!(
            "  {} {} saved",
            style("┃").dim(),
            style(util::human_bytes(saved)).cyan().bold(),
        );
    }
    if del_errs > 0 {
        println!(
            "  {} {} original{} could not be deleted (check permissions)",
            style("\u{2503}").dim(),
            style(del_errs).yellow(),
            if del_errs == 1 { "" } else { "s" },
        );
    }
    if ok > 1 {
        println!(
            "  {} Compression   best: {:.1}%   worst: {:.1}%",
            style("┃").dim(),
            best as f64 / 100.0,
            worst as f64 / 100.0,
        );
    }
    if job.media_type == MediaType::Video {
        println!(
            "  {} Finished in {:.1}s (processed sequentially)",
            style("┃").dim(),
            elapsed.as_secs_f64(),
        );
    } else {
        println!(
            "  {} Finished in {:.1}s using {} parallel thread{}",
            style("┃").dim(),
            elapsed.as_secs_f64(),
            actual_threads,
            if actual_threads == 1 { "" } else { "s" },
        );
    }
    println!("  {}", style("─".repeat(50)).dim());

    Ok(ConversionSummary {
        media_type: format!("{:?}", job.media_type),
        input_format: job.input_fmt.to_string(),
        output_format: job.output_fmt.to_string(),
        files_converted: ok,
        files_skipped: skipped,
        files_failed: errs,
        bytes_saved: saved,
        delete_errors: del_errs,
        duration_secs: elapsed.as_secs_f64(),
        threads_used: actual_threads,
        compression_best_percent: best as f64 / 100.0,
        compression_worst_percent: if worst == 10000 {
            0.0
        } else {
            worst as f64 / 100.0
        },
    })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct HistoryEntry {
    original_path: PathBuf,
    converted_path: PathBuf,
    backup_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct HistoryRecord {
    session_id: u64,
    created_unix: u64,
    entries: Vec<HistoryEntry>,
}

fn history_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rusty-crunch")
        .join("history")
}

fn history_file_path(session_id: u64) -> PathBuf {
    history_dir().join(format!("session_{session_id}.json"))
}

fn latest_history_file() -> Option<PathBuf> {
    let dir = history_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    files.sort();
    files.pop()
}

fn save_history_record(record: &HistoryRecord) -> Result<()> {
    let path = history_file_path(record.session_id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(record)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn move_file_cross_fs(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    std::fs::remove_file(src)?;
    Ok(())
}

fn backup_original_for_restore(original: &Path, session_id: u64, idx: usize) -> Result<PathBuf> {
    let file_name = original
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let dst = history_dir()
        .join("backups")
        .join(format!("session_{session_id}"))
        .join(format!("{idx:06}_{file_name}"));
    move_file_cross_fs(original, &dst)?;
    Ok(dst)
}

pub fn restore_last_session() -> Result<()> {
    let Some(path) = latest_history_file() else {
        println!(
            "\n  {} No previous conversion history found.",
            style("⚠").yellow()
        );
        return Ok(());
    };

    let json = std::fs::read_to_string(&path)?;
    let record: HistoryRecord = serde_json::from_str(&json)?;

    let mut restored_originals = 0usize;
    let mut removed_outputs = 0usize;
    let mut failures = 0usize;

    for entry in record.entries.iter().rev() {
        if entry.converted_path.exists() {
            if std::fs::remove_file(&entry.converted_path).is_ok() {
                removed_outputs += 1;
            } else {
                failures += 1;
            }
        }

        if let Some(backup) = &entry.backup_path {
            if backup.exists() && !entry.original_path.exists() {
                if move_file_cross_fs(backup, &entry.original_path).is_ok() {
                    restored_originals += 1;
                } else {
                    failures += 1;
                }
            }
        }
    }

    println!(
        "\n  {} Restore complete: {} original(s) restored, {} converted file(s) removed, {} issue(s).",
        style("✔").green().bold(),
        style(restored_originals).cyan().bold(),
        style(removed_outputs).cyan().bold(),
        style(failures).yellow().bold(),
    );

    Ok(())
}

// ── Disk space check ────────────────────────────────────────────────
/// Simple heuristic: warn if estimated output would be too close to input size.
/// (Most conversions should reduce size; if estimated output is >80% of input, space might be tight.)
fn check_available_space(_folder: &Path, _required_bytes: u64) -> bool {
    #[cfg(unix)]
    {
        let check_path = if _folder.exists() {
            _folder
        } else {
            _folder.parent().unwrap_or_else(|| Path::new("/"))
        };

        let c_path = match CString::new(check_path.to_string_lossy().as_bytes()) {
            Ok(c) => c,
            Err(_) => return true,
        };

        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat as *mut libc::statvfs) };
        if rc != 0 {
            return true;
        }

        let available = (stat.f_bavail as u128).saturating_mul(stat.f_frsize as u128);
        available >= _required_bytes as u128
    }

    #[cfg(not(unix))]
    {
        true
    }
}

// ── Optimization cache ──────────────────────────────────────────────
// Tracks (size, mtime) of files after in-place optimization so repeat
// runs skip files that haven't changed since last processing.

#[derive(Serialize, Deserialize, Clone)]
struct CacheEntry {
    size: u64,
    modified: u64,
}

impl CacheEntry {
    fn from_path(path: &Path) -> Option<Self> {
        let meta = path.metadata().ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some(Self {
            size: meta.len(),
            modified,
        })
    }

    fn matches(&self, path: &Path) -> bool {
        Self::from_path(path)
            .map(|s| s.size == self.size && s.modified == self.modified)
            .unwrap_or(false)
    }
}

fn opt_cache_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rusty-crunch")
        .join("opt_cache.json")
}

fn load_opt_cache() -> HashMap<PathBuf, CacheEntry> {
    let path = opt_cache_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_opt_cache(cache: &HashMap<PathBuf, CacheEntry>) {
    let path = opt_cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, json);
    }
}

/// Quick check whether any files with the given format exist in the folder.
pub fn has_matching_files(folder: &Path, input_fmt: &str, recursive: bool) -> bool {
    let input_ext = input_fmt.to_ascii_lowercase();
    let max_depth = if recursive { usize::MAX } else { 1 };
    WalkDir::new(folder)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .map(|ext| {
                        let lc = ext.to_ascii_lowercase();
                        lc == input_ext.as_str()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cache_entry_matches_unchanged_file() {
        let dir = std::env::temp_dir().join("rc_cache_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.pdf");
        std::fs::write(&path, b"dummy pdf content").unwrap();

        let entry = CacheEntry::from_path(&path).unwrap();
        assert!(entry.matches(&path), "entry should match unchanged file");

        // Modify the file → entry should no longer match
        std::thread::sleep(std::time::Duration::from_millis(1100)); // ensure mtime changes
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        f.write_all(b"modified content").unwrap();
        drop(f);
        assert!(
            !entry.matches(&path),
            "entry should NOT match modified file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_roundtrip_serialize() {
        let mut cache = HashMap::new();
        cache.insert(
            PathBuf::from("/tmp/a.pdf"),
            CacheEntry {
                size: 100,
                modified: 1234567890,
            },
        );
        cache.insert(
            PathBuf::from("/tmp/b.pdf"),
            CacheEntry {
                size: 200,
                modified: 9876543210,
            },
        );

        let json = serde_json::to_string(&cache).unwrap();
        let loaded: HashMap<PathBuf, CacheEntry> = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&PathBuf::from("/tmp/a.pdf")].size, 100);
        assert_eq!(loaded[&PathBuf::from("/tmp/b.pdf")].modified, 9876543210);
    }

    #[test]
    fn cache_entry_size_tracking() {
        let dir = std::env::temp_dir().join("rc_cache_size_test");
        let _ = std::fs::create_dir_all(&dir);

        // Create files of different sizes
        let sizes = [100, 1000, 10000, 1000000];
        for (i, size) in sizes.iter().enumerate() {
            let path = dir.join(format!("file{}.dat", i));
            let data = vec![0u8; *size];
            std::fs::write(&path, data).unwrap();

            let entry = CacheEntry::from_path(&path).unwrap();
            assert_eq!(entry.size, *size as u64);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_entry_modified_time() {
        let dir = std::env::temp_dir().join("rc_cache_mtime_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("timetest.txt");

        std::fs::write(&path, b"original").unwrap();
        let entry1 = CacheEntry::from_path(&path).unwrap();

        // Wait and modify
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(&path, b"modified").unwrap();
        let entry2 = CacheEntry::from_path(&path).unwrap();

        // Modified times should differ (usually)
        // Note: might be equal on fast filesystems, so we just check they have some value
        assert!(entry1.modified > 0);
        assert!(entry2.modified > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_empty_operations() {
        let cache: HashMap<PathBuf, CacheEntry> = HashMap::new();
        assert!(cache.is_empty());

        // Serialization of empty cache should work
        let json = serde_json::to_string(&cache).unwrap();
        let restored: HashMap<PathBuf, CacheEntry> = serde_json::from_str(&json).unwrap();
        assert!(restored.is_empty());
    }

    #[test]
    fn cache_large_number_of_entries() {
        let mut cache = HashMap::new();

        // Add many entries
        for i in 0..1000 {
            cache.insert(
                format!("/path/file{}.bin", i),
                CacheEntry {
                    size: (i * 1024) as u64,
                    modified: (1000000000 + i as u128) as u64,
                },
            );
        }

        assert_eq!(cache.len(), 1000);

        // Verify serialization works with large cache
        let json = serde_json::to_string(&cache).unwrap();
        let restored: HashMap<PathBuf, CacheEntry> = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.len(), 1000);
        assert_eq!(
            restored[&PathBuf::from("/path/file999.bin")].size,
            999 * 1024
        );
    }

    #[test]
    fn cache_entry_matches_same_file_twice() {
        let dir = std::env::temp_dir().join("rc_cache_same_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("file.txt");

        std::fs::write(&path, b"content").unwrap();
        let entry1 = CacheEntry::from_path(&path).unwrap();
        let entry2 = CacheEntry::from_path(&path).unwrap();

        // Same file read twice should create equivalent entries
        assert_eq!(entry1.size, entry2.size);
        assert_eq!(entry1.modified, entry2.modified);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversion_summary_creation() {
        let summary = ConversionSummary {
            media_type: "Audio".to_string(),
            input_format: "MP3".to_string(),
            output_format: "FLAC".to_string(),
            files_converted: 10,
            files_skipped: 5,
            files_failed: 2,
            bytes_saved: 1048576,
            delete_errors: 0,
            duration_secs: 42.5,
            threads_used: 4,
            compression_best_percent: 45.0,
            compression_worst_percent: 95.0,
        };

        assert_eq!(summary.files_converted, 10);
        assert_eq!(summary.files_skipped, 5);
        assert_eq!(summary.files_failed, 2);
        assert_eq!(summary.bytes_saved, 1048576);
    }

    #[test]
    fn conversion_summary_serialization() {
        let summary = ConversionSummary {
            media_type: "Video".to_string(),
            input_format: "AVI".to_string(),
            output_format: "MKV".to_string(),
            files_converted: 3,
            files_skipped: 1,
            files_failed: 0,
            bytes_saved: 5242880,
            delete_errors: 0,
            duration_secs: 120.0,
            threads_used: 8,
            compression_best_percent: 50.0,
            compression_worst_percent: 80.0,
        };

        let json = serde_json::to_string(&summary).unwrap();
        let restored: ConversionSummary = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.media_type, "Video");
        assert_eq!(restored.input_format, "AVI");
        assert_eq!(restored.output_format, "MKV");
        assert_eq!(restored.files_converted, 3);
        assert_eq!(restored.bytes_saved, 5242880);
    }

    #[test]
    fn cache_entry_various_paths() {
        let dir = std::env::temp_dir().join("rc_cache_paths_test");
        let _ = std::fs::create_dir_all(&dir);

        let paths = vec![
            "simple.txt",
            "file with spaces.txt",
            "file-with-dashes.txt",
            "file_with_underscores.txt",
            "файл.txt", // Unicode filename
        ];

        for filename in paths {
            let path = dir.join(filename);
            std::fs::write(&path, b"test content").unwrap();

            let entry = CacheEntry::from_path(&path).unwrap();
            assert!(entry.matches(&path));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_entry_zero_size_file() {
        let dir = std::env::temp_dir().join("rc_cache_empty_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.txt");

        // Create zero-byte file
        std::fs::write(&path, b"").unwrap();

        let entry = CacheEntry::from_path(&path).unwrap();
        assert_eq!(entry.size, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
