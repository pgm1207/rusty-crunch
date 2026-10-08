use crate::formats::MediaType;
use crate::processor::{ImageScale, Quality, VideoScale};
use crate::util;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
// use std::sync::Mutex; // No longer needed here
use tokio::sync::Mutex;

/// Global lock for LibreOffice — only one headless instance may run at a time per user profile.
static LO_LOCK: Mutex<()> = Mutex::const_new(());

/// Run an external command, suppress stdout/stderr, and return a nice error on failure.
async fn run(cmd: &mut Command, ctx: &str) -> Result<()> {
    let output = cmd
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to launch `{}` — is it installed?", ctx))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = stderr.lines().take(3).collect::<Vec<_>>().join("\n  ");
    let details = if details.trim().is_empty() {
        "unknown error".to_string()
    } else {
        details
    };
    bail!("{ctx}:\n  {details}");
}

pub struct ConversionOptions<'a> {
    pub media_type: MediaType,
    pub input_fmt: &'a str,
    pub output_fmt: &'a str,
    pub normalize_audio: bool,
    pub quality: Quality,
    pub keep_metadata: bool,
    pub video_scale: VideoScale,
    pub image_scale: ImageScale,
}

/// Create a unique staging directory *beside* the output. Publishing is then a
/// same-filesystem rename, including when the source lives on another volume.
static STAGING_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn staging_dir(output: &Path) -> Result<std::path::PathBuf> {
    let parent = output.parent().context("Output has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    for _ in 0..128 {
        let id = STAGING_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = parent.join(format!(
            ".rusty-crunch-stage-{}-{id}",
            std::process::id()
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("Could not create conversion staging directory"),
        }
    }
    bail!("Could not allocate a unique staging directory")
}

/// Publish only a fully written file. Retain the previous destination until
/// the rename succeeds, so failed overwrites never destroy user data.
fn publish_staged(staged: &Path, destination: &Path) -> Result<()> {
    let backup = staged
        .parent()
        .context("Staged output has no parent")?
        .join("previous-output");
    let had_destination = destination.symlink_metadata().is_ok();
    if had_destination {
        std::fs::rename(destination, &backup).with_context(|| {
            format!("Could not preserve existing {}", destination.display())
        })?;
    }

    if let Err(error) = std::fs::rename(staged, destination) {
        if had_destination {
            if let Err(restore_error) = std::fs::rename(&backup, destination) {
                bail!(
                    "Publishing {} failed ({error}); restoring the previous file also failed ({restore_error}).                      The previous file remains at {}",
                    destination.display(),
                    backup.display()
                );
            }
        }
        return Err(error).with_context(|| format!("Could not publish {}", destination.display()));
    }
    if had_destination {
        // A cleanup failure must not turn a successful conversion into an error.
        if let Err(e) = std::fs::remove_file(&backup) {
            crate::util::log_event("WARN", &format!(
                "Could not clean up previous output {}: {e}", backup.display()
            ));
        }
    }
    Ok(())
}

pub async fn convert(input: &Path, output: &Path, opts: ConversionOptions<'_>) -> Result<()> {
    let stage_dir = staging_dir(output)?;
    // LibreOffice derives the output basename from the INPUT, not from the
    // requested destination. Preserve the original stem inside its own outdir.
    let stem = input.file_name().context("Input has no file name")?;
    let extension = output.extension().context("Output has no file extension")?;
    let staged = stage_dir.join(stem).with_extension(extension);

    let conversion = convert_dispatch(input, &staged, &opts).await;
    if let Err(error) = conversion {
        let _ = std::fs::remove_dir_all(&stage_dir);
        return Err(error);
    }

    // Some tools exit with status 0 yet produce nothing (or an empty file).
    let metadata = staged.metadata().with_context(|| {
        format!("Converter did not produce {}", staged.display())
    });
    match metadata {
        Ok(info) if info.is_file() && info.len() > 0 => {}
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&stage_dir);
            bail!("Converter produced an empty or invalid output for {}", input.display());
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(error);
        }
    }

    // If publication fails, keep the staging directory for manual recovery,
    // particularly if the rollback itself also fails.
    publish_staged(&staged, output)?;
    if let Err(e) = std::fs::remove_dir_all(&stage_dir) {
        crate::util::log_event("WARN", &format!(
            "Could not clean up staging directory {}: {e}", stage_dir.display()
        ));
    }
    Ok(())
}

async fn convert_dispatch(input: &Path, output: &Path, opts: &ConversionOptions<'_>) -> Result<()> {
    match opts.media_type {
        MediaType::Audio => {
            convert_audio(
                input,
                output,
                opts.output_fmt,
                opts.normalize_audio,
                opts.quality,
                opts.keep_metadata,
            )
            .await
        }
        MediaType::Video => {
            convert_video(
                input,
                output,
                opts.output_fmt,
                opts.quality,
                opts.video_scale,
                opts.keep_metadata,
            )
            .await
        }
        MediaType::Images => {
            convert_image(
                input,
                output,
                opts.output_fmt,
                opts.quality,
                opts.image_scale,
                opts.keep_metadata,
            )
            .await
        }
        MediaType::Documents => {
            convert_document(input, output, opts.input_fmt, opts.output_fmt).await
        }
    }
}

// ── Audio ──────────────────────────────────────────────────────────────

async fn convert_audio(
    input: &Path,
    output: &Path,
    out_fmt: &str,
    normalize_audio: bool,
    quality: Quality,
    keep_metadata: bool,
) -> Result<()> {
    let ffmpeg = util::ffmpeg_command();
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"]);
    cmd.arg(input);

    if !keep_metadata {
        cmd.args(["-map_metadata", "-1"]);
    }

    match out_fmt {
        "OPUS" => {
            let br = match quality {
                Quality::High => "128k",
                Quality::Medium => "96k",
                Quality::Low => "64k",
            };
            cmd.args(["-c:a", "libopus", "-b:a", br]);
        }
        "AAC" | "M4A" => {
            let br = match quality {
                Quality::High => "256k",
                Quality::Medium => "192k",
                Quality::Low => "128k",
            };
            cmd.args(["-c:a", "aac", "-b:a", br]);
        }
        "FLAC" => {
            cmd.args(["-c:a", "flac"]);
        }
        "OGG" => {
            let q = match quality {
                Quality::High => "6",
                Quality::Medium => "4",
                Quality::Low => "2",
            };
            cmd.args(["-c:a", "libvorbis", "-q:a", q]);
        }
        "MP3" => {
            let q = match quality {
                Quality::High => "0",
                Quality::Medium => "2",
                Quality::Low => "4",
            };
            cmd.args(["-c:a", "libmp3lame", "-q:a", q]);
        }
        "WMA" => {
            let br = match quality {
                Quality::High => "192k",
                Quality::Medium => "128k",
                Quality::Low => "96k",
            };
            cmd.args(["-c:a", "wmav2", "-b:a", br]);
        }
        "WAV" | "AIFF" => {}
        _ => {
            cmd.args(["-q:a", "0"]);
        }
    }

    if normalize_audio {
        cmd.args(["-af", "loudnorm"]);
    }

    let is_inplace = input == output;
    let final_output = if is_inplace {
        output.with_extension(format!(
            "tmp.{}",
            output.extension().unwrap_or_default().to_string_lossy()
        ))
    } else {
        output.to_path_buf()
    };

    cmd.arg("-y").arg(&final_output);
    let res = run(&mut cmd, ffmpeg.as_str()).await;

    if is_inplace {
        if res.is_ok() {
            std::fs::rename(&final_output, output)?;
        } else {
            let _ = std::fs::remove_file(&final_output);
        }
    }

    res
}

// ── Video ──────────────────────────────────────────────────────────────

async fn convert_video(
    input: &Path,
    output: &Path,
    out_fmt: &str,
    quality: Quality,
    scale: VideoScale,
    keep_metadata: bool,
) -> Result<()> {
    let best_enc = util::best_h264_encoder();
    let res = build_and_run_video(
        input,
        output,
        out_fmt,
        quality,
        scale,
        keep_metadata,
        best_enc,
    )
    .await;

    if res.is_err() && best_enc.name != "libx264" && best_enc.name != "libopenh264" {
        eprintln!(
            "  \n\033[33m⚠ Hardware encoder '{}' failed. Falling back to CPU...\033[0m",
            best_enc.name
        );
        let sw_enc = util::software_h264_encoder();
        return build_and_run_video(
            input,
            output,
            out_fmt,
            quality,
            scale,
            keep_metadata,
            sw_enc,
        )
        .await;
    }

    res
}

async fn build_and_run_video(
    input: &Path,
    output: &Path,
    out_fmt: &str,
    quality: Quality,
    scale: VideoScale,
    keep_metadata: bool,
    enc: &util::H264Encoder,
) -> Result<()> {
    let use_vaapi =
        (out_fmt == "MKV" || out_fmt == "MP4" || out_fmt == "TS") && enc.name.ends_with("vaapi");
    let threads_str = "0".to_string();

    let ffmpeg = util::ffmpeg_command();
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    cmd.args(["-threads", &threads_str, "-filter_threads", &threads_str]);
    if use_vaapi {
        let vaapi = util::vaapi_device();
        cmd.args(["-vaapi_device", vaapi.as_str()]);
        cmd.args([
            "-hwaccel",
            "vaapi",
            "-hwaccel_device",
            vaapi.as_str(),
            "-hwaccel_output_format",
            "vaapi",
        ]);
    }
    cmd.args(["-i"]).arg(input);

    // Preserve primary video plus all audio/subtitle streams.
    cmd.args(["-map", "0:v:0", "-map", "0:a?", "-map", "0:s?"]);

    if !keep_metadata {
        cmd.args(["-map_metadata", "-1"]);
    }

    let mut scale_filter: Option<String> = None;

    match scale {
        VideoScale::P1080 => {
            scale_filter = Some("scale=-2:1080".to_string());
        }
        VideoScale::P720 => {
            scale_filter = Some("scale=-2:720".to_string());
        }
        VideoScale::AutoTarget {
            target_height,
            preset,
        } => {
            let flags = match preset {
                crate::processor::UpscalePreset::Anime => "spline",
                crate::processor::UpscalePreset::Movie => "lanczos",
            };

            if let Some((w, h)) = probe_video_dimensions(input).await {
                let h_u32 = h.max(1);
                let mult = if h_u32 >= target_height {
                    1_u32
                } else {
                    target_height.div_ceil(h_u32).clamp(2, 4)
                };

                if mult > 1 {
                    let tw = ((w as u64).saturating_mul(mult as u64) / 2) * 2;
                    let th = ((h as u64).saturating_mul(mult as u64) / 2) * 2;
                    if tw >= 2 && th >= 2 {
                        scale_filter = Some(format!("scale={tw}:{th}:flags={flags}"));
                    } else {
                        scale_filter = Some(format!(
                            "scale=trunc(iw*{mult}/2)*2:trunc(ih*{mult}/2)*2:flags={flags}"
                        ));
                    }
                }
            } else {
                scale_filter = Some(format!("scale=-2:{target_height}:flags={flags}"));
            }
        }
        VideoScale::Upscale2xAnime
        | VideoScale::Upscale3xAnime
        | VideoScale::Upscale4xAnime
        | VideoScale::Upscale2xMovie
        | VideoScale::Upscale3xMovie
        | VideoScale::Upscale4xMovie => {
            let (mult, flags) = match scale {
                VideoScale::Upscale2xAnime => (2_u64, "spline"),
                VideoScale::Upscale3xAnime => (3_u64, "spline"),
                VideoScale::Upscale4xAnime => (4_u64, "spline"),
                VideoScale::Upscale2xMovie => (2_u64, "lanczos"),
                VideoScale::Upscale3xMovie => (3_u64, "lanczos"),
                VideoScale::Upscale4xMovie => (4_u64, "lanczos"),
                _ => unreachable!(),
            };

            if let Some((w, h)) = probe_video_dimensions(input).await {
                let tw = ((w as u64).saturating_mul(mult) / 2) * 2;
                let th = ((h as u64).saturating_mul(mult) / 2) * 2;
                if tw >= 2 && th >= 2 {
                    scale_filter = Some(format!("scale={tw}:{th}:flags={flags}"));
                } else {
                    scale_filter = Some(format!(
                        "scale=trunc(iw*{mult}/2)*2:trunc(ih*{mult}/2)*2:flags={flags}"
                    ));
                }
            } else {
                scale_filter = Some(format!(
                    "scale=trunc(iw*{mult}/2)*2:trunc(ih*{mult}/2)*2:flags={flags}"
                ));
            }
        }
        VideoScale::Original => {}
    }

    if use_vaapi {
        if let Some(filter) = scale_filter.clone() {
            if filter.contains("scale=") {
                let dims = filter.split(':').take(2).collect::<Vec<_>>();
                if dims.len() >= 2 && dims[0].starts_with("scale=") {
                    let w = dims[0].replace("scale=", "");
                    let h = dims[1];
                    scale_filter = Some(format!("scale_vaapi=w={}:h={}", w, h));
                } else {
                    scale_filter = None;
                }
            } else {
                scale_filter = None;
            }
        }
    }

    if let Some(filter) = &scale_filter {
        cmd.args(["-vf", filter]);
    }

    let crf = match quality {
        Quality::High => "18",
        Quality::Medium => "23",
        Quality::Low => "28",
    };

    let threads = threads_str.clone();

    match out_fmt {
        "WEBM" => {
            cmd.args([
                "-c:v",
                "libvpx-vp9",
                "-crf",
                crf,
                "-b:v",
                "0",
                "-threads",
                &threads,
                "-c:a",
                "libopus",
            ]);
        }
        "AVI" => {
            cmd.args([
                "-c:v",
                "mpeg4",
                "-q:v",
                "5",
                "-threads",
                &threads,
                "-c:a",
                "libmp3lame",
            ]);
        }
        "WMV" => {
            cmd.args([
                "-c:v", "wmv2", "-b:v", "2M", "-threads", &threads, "-c:a", "wmav2",
            ]);
        }
        "TS" => {
            cmd.args(["-c:v", enc.name]);
            match enc.name {
                "h264_videotoolbox" => {
                    let q = match quality {
                        Quality::High => "80",
                        Quality::Medium => "65",
                        Quality::Low => "50",
                    };
                    cmd.args(["-q:v", q]);
                }
                "h264_nvenc" => {
                    let q = match quality {
                        Quality::High => "18",
                        Quality::Medium => "23",
                        Quality::Low => "28",
                    };
                    cmd.args(["-preset", "p4", "-cq", q]);
                }
                "h264_qsv" => {
                    let q = match quality {
                        Quality::High => "18",
                        Quality::Medium => "23",
                        Quality::Low => "28",
                    };
                    cmd.args(["-global_quality", q]);
                }
                "libx264" => {
                    cmd.args(["-preset", "medium", "-crf", crf]);
                }
                _ => {
                    cmd.args(enc.quality_args);
                }
            }
            cmd.args(["-threads", &threads, "-c:a", "copy", "-c:s", "copy"]);
        }
        _ => {
            // MP4/MKV/MOV/FLV — try hardware-accelerated H.264, fall back to libx264
            cmd.args(["-c:v", enc.name]);
            match enc.name {
                "h264_videotoolbox" => {
                    let q = match quality {
                        Quality::High => "80",
                        Quality::Medium => "65",
                        Quality::Low => "50",
                    };
                    cmd.args(["-q:v", q]);
                }
                "h264_nvenc" => {
                    let cq = match quality {
                        Quality::High => "18",
                        Quality::Medium => "23",
                        Quality::Low => "28",
                    };
                    cmd.args(["-preset", "p4", "-cq", cq]);
                }
                "h264_qsv" => {
                    let q = match quality {
                        Quality::High => "18",
                        Quality::Medium => "23",
                        Quality::Low => "28",
                    };
                    cmd.args(["-global_quality", q]);
                }
                "libx264" => {
                    cmd.args(["-preset", "medium", "-crf", crf]);
                }
                _ => {
                    cmd.args(enc.quality_args);
                }
            }
            cmd.args(["-threads", &threads, "-c:a", "copy", "-c:s", "copy"]);
        }
    }

    cmd.arg("-y").arg(output);
    run(&mut cmd, ffmpeg.as_str()).await
}

// ── Images ─────────────────────────────────────────────────────────────

async fn convert_image(
    input: &Path,
    output: &Path,
    out_fmt: &str,
    quality: Quality,
    scale: ImageScale,
    keep_metadata: bool,
) -> Result<()> {
    let bin = util::magick_command();

    let mut cmd = Command::new(&bin);
    cmd.arg(input);

    if !keep_metadata {
        cmd.arg("-strip");
    }

    match scale {
        ImageScale::W1920 => {
            cmd.args(["-resize", "1920x>"]);
        }
        ImageScale::W1080 => {
            cmd.args(["-resize", "1080x>"]);
        }
        ImageScale::Original => {}
    }

    let q_val = match quality {
        Quality::High => "92",
        Quality::Medium => "82",
        Quality::Low => "65",
    };

    match out_fmt {
        "JPEG" => {
            cmd.args(["-quality", q_val, "-sampling-factor", "4:2:0"]);
        }
        "PNG" => {
            cmd.args(["-quality", q_val]);
        }
        "WEBP" => {
            cmd.args(["-quality", q_val]);
        }
        "AVIF" => {
            cmd.args(["-quality", q_val]);
        }
        "TIFF" => {
            cmd.args(["-compress", "lzw"]);
        }
        _ => {}
    }

    cmd.arg(output);
    run(&mut cmd, bin.as_str()).await
}

// ── Documents ──────────────────────────────────────────────────────────

async fn convert_document(input: &Path, output: &Path, in_fmt: &str, out_fmt: &str) -> Result<()> {
    // PDF → PDF (Optimized): 150 PPI image downsampling, skip if no savings
    if in_fmt.eq_ignore_ascii_case("pdf") && out_fmt == "PDF (Optimized)" {
        return optimize_pdf(
            input,
            output,
            &[
                "-dDownsampleColorImages=true",
                "-dColorImageResolution=150",
                "-dColorImageDownsampleThreshold=1.0",
                "-dDownsampleGrayImages=true",
                "-dGrayImageResolution=150",
                "-dGrayImageDownsampleThreshold=1.0",
                "-dDownsampleMonoImages=true",
                "-dMonoImageResolution=150",
                "-dMonoImageDownsampleThreshold=1.0",
            ],
            true,
        )
        .await;
    }

    // PDF → PDF: general optimization
    if in_fmt.eq_ignore_ascii_case("pdf") && out_fmt.eq_ignore_ascii_case("pdf") {
        return optimize_pdf(input, output, &["-dPDFSETTINGS=/ebook"], false).await;
    }

    // LibreOffice is NOT thread-safe — serialize with a mutex
    let _guard = LO_LOCK.lock().await;
    let out_dir = output.parent().unwrap_or(Path::new("."));
    let lo = util::lo_command();
    let mut cmd = Command::new(&lo);
    cmd.args(["--headless", "--convert-to"]);
    cmd.arg(out_fmt.to_lowercase());
    cmd.arg("--outdir").arg(out_dir);
    cmd.arg(input);
    run(&mut cmd, lo.as_str()).await
}

/// Run Ghostscript PDF optimization. Writes to a temp file, then renames.
/// If `skip_if_larger` is true, files that don't shrink are left untouched.
async fn optimize_pdf(
    input: &Path,
    output: &Path,
    settings: &[&str],
    skip_if_larger: bool,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp_dir = std::env::temp_dir();
    let proc_id = std::process::id();
    let tmp_in = tmp_dir.join(format!("crunch_in_{}_{}.pdf.tmp", proc_id, now));
    let tmp_out = tmp_dir.join(format!("crunch_out_{}_{}.pdf.tmp", proc_id, now));

    let input_size = input.metadata().map(|m| m.len()).unwrap_or(0);

    // Ghostscript on Windows can have issues with unicode chars or paths > 260 chars.
    // Copy the input to a clean temp path to avoid read errors.
    std::fs::copy(input, &tmp_in)?;

    let gs = util::gs_command();
    let mut cmd = Command::new(&gs);
    cmd.args([
        "-sDEVICE=pdfwrite",
        "-dCompatibilityLevel=1.4",
        "-dNOPAUSE",
        "-dQUIET",
        "-dBATCH",
    ]);
    cmd.args(settings);

    // Writing to a clean path in %TEMP% avoids write errors.
    cmd.arg(format!("-sOutputFile={}", tmp_out.display()));
    cmd.arg(&tmp_in);

    // Ghostscript can hang indefinitely on corrupted/complex PDFs. Add a generous 10-minute timeout.
    let run_future = run(&mut cmd, gs.as_str());
    let result = match tokio::time::timeout(std::time::Duration::from_secs(600), run_future).await {
        Ok(res) => res,
        Err(_) => {
            let _ = std::fs::remove_file(&tmp_in);
            let _ = std::fs::remove_file(&tmp_out);
            bail!("Ghostscript timed out after 10 minutes")
        },
    };

    // Always clean up the temp input file
    let _ = std::fs::remove_file(&tmp_in);

    if result.is_ok() {
        if skip_if_larger && input_size > 0 {
            let out_size = tmp_out.metadata().map(|m| m.len()).unwrap_or(0);
            if out_size >= input_size {
                let _ = std::fs::remove_file(&tmp_out);
                // Leave original untouched; copy only if paths differ
                if input != output {
                    std::fs::copy(input, output)?;
                }
                return Ok(());
            }
        }
        std::fs::copy(&tmp_out, output)?;
        let _ = std::fs::remove_file(&tmp_out);
    } else {
        let _ = std::fs::remove_file(&tmp_out);
    }
    result
}

async fn probe_video_dimensions(input: &Path) -> Option<(u32, u32)> {
    let ffprobe = util::ffprobe_command();
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(input)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = s.trim().split('x').collect();
    if parts.len() == 2 {
        let w = parts[0].parse::<u32>().ok()?;
        let h = parts[1].parse::<u32>().ok()?;
        Some((w, h))
    } else {
        None
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> std::path::PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rc-stage-test-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn staged_replacement_preserves_existing_file_until_publish() {
        let dir = scratch();
        let output = dir.join("result.txt");
        std::fs::write(&output, b"original").unwrap();
        let stage = staging_dir(&output).unwrap();
        let staged = stage.join("generated.txt");
        std::fs::write(&staged, b"replacement").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        publish_staged(&staged, &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"replacement");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_publish_does_not_delete_existing_destination() {
        let dir = scratch();
        let output = dir.join("result.txt");
        std::fs::write(&output, b"original").unwrap();
        let missing = dir.join("missing.txt");
        assert!(publish_staged(&missing, &output).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
