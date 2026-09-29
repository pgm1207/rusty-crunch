#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MediaType {
    Audio,
    Video,
    Images,
    Documents,
}

impl MediaType {
    pub const ALL: [MediaType; 4] = [
        MediaType::Audio,
        MediaType::Video,
        MediaType::Images,
        MediaType::Documents,
    ];

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Audio => "🎵",
            Self::Video => "🎬",
            Self::Images => "🖼️ ",
            Self::Documents => "📄",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Images => "Images",
            Self::Documents => "Documents",
        }
    }

    pub fn formats(self) -> &'static [&'static str] {
        match self {
            Self::Audio => &[
                "MP3", "WAV", "AIFF", "OGG", "FLAC", "AAC", "M4A", "WMA", "OPUS",
            ],
            Self::Video => &["MP4", "MKV", "AVI", "MOV", "WEBM", "FLV", "WMV", "TS"],
            Self::Images => &["PNG", "JPEG", "BMP", "GIF", "WEBP", "TIFF", "AVIF", "ICO"],
            Self::Documents => &["PDF", "DOCX", "XLSX", "PPTX", "ODT", "ODS", "ODP", "EPUB"],
        }
    }

    /// Return only the output formats that make sense given the chosen input.
    /// Zero-allocation: returns a static slice.
    pub fn compatible_outputs(self, input: &str) -> &'static [&'static str] {
        match self {
            Self::Audio => match input {
                "WAV" => &["WAV", "MP3", "OGG", "FLAC", "AAC", "M4A", "OPUS"],
                "AIFF" => &["AIFF", "MP3", "OGG", "FLAC", "AAC", "M4A", "OPUS"],
                "FLAC" => &["FLAC", "MP3", "OGG", "AAC", "M4A", "OPUS"],
                "MP3" => &["MP3", "OGG", "AAC", "M4A", "OPUS"],
                "OGG" => &["OGG", "MP3", "AAC", "M4A", "OPUS"],
                "AAC" => &["AAC", "MP3", "OGG", "M4A", "OPUS"],
                "M4A" => &["M4A", "MP3", "OGG", "AAC", "OPUS"],
                "OPUS" => &["OPUS", "MP3", "OGG", "AAC", "M4A"],
                "WMA" => &["WMA", "MP3", "OGG", "FLAC", "AAC", "M4A", "OPUS"],
                _ => &[],
            },
            Self::Video => match input {
                "AVI" => &["AVI", "MP4", "MKV", "WEBM"],
                "MOV" => &["MOV", "MP4", "MKV", "WEBM"],
                "FLV" => &["FLV", "MP4", "MKV", "WEBM"],
                "WMV" => &["WMV", "MP4", "MKV", "WEBM"],
                "TS" => &["TS", "MP4", "MKV"],
                "MP4" => &["MP4", "MKV", "WEBM"],
                "MKV" => &["MKV", "MP4", "WEBM"],
                "WEBM" => &["WEBM", "MP4", "MKV"],
                _ => &[],
            },
            Self::Images => match input {
                "BMP" => &["PNG", "JPEG", "WEBP", "AVIF"],
                "TIFF" => &["PNG", "JPEG", "WEBP", "AVIF"],
                "PNG" => &["JPEG", "WEBP", "AVIF"],
                "JPEG" => &["WEBP", "AVIF"],
                "GIF" => &["WEBP", "AVIF", "PNG"],
                "WEBP" => &["JPEG", "AVIF", "PNG"],
                "AVIF" => &["JPEG", "WEBP", "PNG"],
                "ICO" => &["PNG", "WEBP"],
                _ => &[],
            },
            Self::Documents => match input {
                "PDF" => &["PDF", "PDF (Optimized)"],
                "DOCX" => &["PDF", "ODT"],
                "XLSX" => &["PDF", "ODS"],
                "PPTX" => &["PDF", "ODP"],
                "ODT" => &["PDF", "DOCX"],
                "ODS" => &["PDF", "XLSX"],
                "ODP" => &["PDF", "PPTX"],
                "EPUB" => &["PDF"],
                _ => &[],
            },
        }
    }

    /// Whether a specific format is lossless for this media type.
    pub fn is_lossless(self, fmt: &str) -> bool {
        match self {
            Self::Audio => matches!(fmt, "WAV" | "AIFF" | "FLAC"),
            Self::Video => false, // all our video codecs are lossy
            Self::Images => matches!(fmt, "PNG" | "BMP" | "TIFF" | "ICO"),
            Self::Documents => fmt != "PDF (Optimized)", // 150 PPI downsample is lossy
        }
    }

    /// Human-friendly warning when converting from lossless → lossy.
    /// Returns `None` when no warning is needed.
    pub fn lossy_warning(self, input: &str, output: &str) -> Option<&'static str> {
        if self == Self::Documents {
            if output == "PDF (Optimized)" {
                return Some(
                    "\u{26a0}  PDF (Optimized) downsamples images to 150 PPI.\n   \
                     This is irreversible \u{2014} keep a backup if you need full resolution.",
                );
            }
            return None;
        }
        let in_lossless = self.is_lossless(input);
        let out_lossless = self.is_lossless(output);

        if in_lossless && !out_lossless {
            Some(
                "⚠  You are converting from a lossless format to a lossy one.\n   \
                 This is a one-way operation — you cannot recover the original quality.\n   \
                 Lossless → lossless (e.g. WAV→FLAC) is recommended unless you need smaller files.",
            )
        } else if !in_lossless && !out_lossless {
            Some(
                "⚠  Both formats are lossy — each re-encode degrades quality slightly.\n   \
                 Consider keeping a lossless master copy of your originals.",
            )
        } else {
            None // lossless → lossless or lossy → lossless (rare but fine)
        }
    }

    pub fn display_item(self) -> String {
        format!("{} {}", self.icon(), self.label())
    }
}

/// Sentinel returned by `prompt::select_input_format` when the user picks "All Lossless → FLAC".
pub const LOSSLESS_AUDIO_SENTINEL: &str = "ALL_LOSSLESS_AUDIO";
/// The input formats that map to the lossless-to-FLAC batch conversion.
pub const LOSSLESS_AUDIO_INPUTS: &[&str] = &["WAV", "AIFF"];

pub const LOSSY_AUDIO_SENTINEL: &str = "ALL_LOSSY_AUDIO";
pub const LOSSY_AUDIO_INPUTS: &[&str] = &["MP3", "OGG", "AAC", "M4A", "WMA"];

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Optimal conversion table for Recommended Crunch mode.
/// Each entry is (MediaType, input_format, output_format).
pub fn recommended_conversions() -> &'static [(MediaType, &'static str, &'static str)] {
    &[
        // Audio: lossless → FLAC
        (MediaType::Audio, "WAV", "FLAC"),
        (MediaType::Audio, "AIFF", "FLAC"),
        // Audio: lossy → OPUS
        (MediaType::Audio, "MP3", "OPUS"),
        (MediaType::Audio, "OGG", "OPUS"),
        (MediaType::Audio, "AAC", "OPUS"),
        (MediaType::Audio, "M4A", "OPUS"),
        (MediaType::Audio, "WMA", "OPUS"),
        // Video: legacy containers → MKV
        (MediaType::Video, "AVI", "MKV"),
        (MediaType::Video, "MOV", "MKV"),
        (MediaType::Video, "FLV", "MKV"),
        (MediaType::Video, "WMV", "MKV"),
        (MediaType::Video, "TS", "MKV"),
        // Images: uncompressed/legacy → PNG (lossless)
        (MediaType::Images, "BMP", "PNG"),
        (MediaType::Images, "TIFF", "PNG"),
        (MediaType::Images, "ICO", "PNG"),
        (MediaType::Images, "GIF", "PNG"),
        // Images: lossy → AVIF (most efficient)
        (MediaType::Images, "JPEG", "AVIF"),
        // Documents: PDF optimization
        (MediaType::Documents, "PDF", "PDF (Optimized)"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mediatype_icons() {
        assert_eq!(MediaType::Audio.icon(), "🎵");
        assert_eq!(MediaType::Video.icon(), "🎬");
        assert_eq!(MediaType::Images.icon(), "🖼️ ");
        assert_eq!(MediaType::Documents.icon(), "📄");
    }

    #[test]
    fn test_mediatype_labels() {
        assert_eq!(MediaType::Audio.label(), "Audio");
        assert_eq!(MediaType::Video.label(), "Video");
        assert_eq!(MediaType::Images.label(), "Images");
        assert_eq!(MediaType::Documents.label(), "Documents");
    }

    #[test]
    fn test_mediatype_formats() {
        let audio_formats = MediaType::Audio.formats();
        assert!(audio_formats.contains(&"MP3"));
        assert!(audio_formats.contains(&"WAV"));
        assert!(audio_formats.contains(&"FLAC"));
        assert!(audio_formats.contains(&"OGG"));
        assert!(!audio_formats.contains(&"MP4")); // Video format

        let video_formats = MediaType::Video.formats();
        assert!(video_formats.contains(&"MP4"));
        assert!(video_formats.contains(&"MKV"));
        assert!(video_formats.contains(&"WEBM"));
        assert!(!video_formats.contains(&"MP3")); // Audio format

        let image_formats = MediaType::Images.formats();
        assert!(image_formats.contains(&"PNG"));
        assert!(image_formats.contains(&"JPEG"));
        assert!(image_formats.contains(&"WEBP"));
        assert!(!image_formats.contains(&"MP4")); // Video format

        let doc_formats = MediaType::Documents.formats();
        assert!(doc_formats.contains(&"PDF"));
        assert!(doc_formats.contains(&"DOCX"));
        assert!(doc_formats.contains(&"XLSX"));
    }

    #[test]
    fn test_mediatype_all() {
        assert_eq!(MediaType::ALL.len(), 4);
        assert!(MediaType::ALL.contains(&MediaType::Audio));
        assert!(MediaType::ALL.contains(&MediaType::Video));
        assert!(MediaType::ALL.contains(&MediaType::Images));
        assert!(MediaType::ALL.contains(&MediaType::Documents));
    }

    #[test]
    fn test_compatible_outputs_audio() {
        let audio = MediaType::Audio;
        assert!(audio.compatible_outputs("MP3").contains(&"OGG"));
        assert!(audio.compatible_outputs("MP3").contains(&"AAC"));
        assert!(audio.compatible_outputs("MP3").contains(&"MP3")); // Can normalize in place

        assert!(audio.compatible_outputs("WAV").contains(&"FLAC"));
        assert!(audio.compatible_outputs("WAV").contains(&"MP3"));

        // Invalid input format returns empty slice
        assert!(audio.compatible_outputs("INVALID").is_empty());
    }

    #[test]
    fn test_compatible_outputs_video() {
        let video = MediaType::Video;
        assert!(video.compatible_outputs("MP4").contains(&"MKV"));
        assert!(video.compatible_outputs("MP4").contains(&"WEBM"));
        assert!(video.compatible_outputs("MP4").contains(&"MP4"));

        assert!(video.compatible_outputs("AVI").contains(&"MP4"));
        assert!(video.compatible_outputs("AVI").contains(&"MKV"));
        assert!(video.compatible_outputs("AVI").contains(&"AVI"));

        assert!(video.compatible_outputs("INVALID").is_empty());
    }

    #[test]
    fn test_compatible_outputs_images() {
        let images = MediaType::Images;
        assert!(images.compatible_outputs("JPEG").contains(&"WEBP"));
        assert!(images.compatible_outputs("JPEG").contains(&"AVIF"));
        assert!(!images.compatible_outputs("JPEG").contains(&"JPEG"));

        assert!(images.compatible_outputs("PNG").contains(&"JPEG"));
        assert!(images.compatible_outputs("PNG").contains(&"WEBP"));

        assert!(images.compatible_outputs("INVALID").is_empty());
    }

    #[test]
    fn test_compatible_outputs_documents() {
        let docs = MediaType::Documents;
        assert!(docs.compatible_outputs("DOCX").contains(&"PDF"));
        assert!(docs.compatible_outputs("DOCX").contains(&"ODT"));

        assert!(docs.compatible_outputs("XLSX").contains(&"PDF"));
        assert!(docs.compatible_outputs("XLSX").contains(&"ODS"));

        // PDF can only go to "PDF (Optimized)"
        let pdf_compat = docs.compatible_outputs("PDF");
        assert!(pdf_compat.contains(&"PDF"));
        assert!(pdf_compat.contains(&"PDF (Optimized)"));

        assert!(docs.compatible_outputs("INVALID").is_empty());
    }

    #[test]
    fn test_is_lossless_audio() {
        let audio = MediaType::Audio;
        assert!(audio.is_lossless("WAV"));
        assert!(audio.is_lossless("AIFF"));
        assert!(audio.is_lossless("FLAC"));

        assert!(!audio.is_lossless("MP3"));
        assert!(!audio.is_lossless("OGG"));
        assert!(!audio.is_lossless("AAC"));
        assert!(!audio.is_lossless("OPUS"));
    }

    #[test]
    fn test_is_lossless_video() {
        let video = MediaType::Video;
        // All video formats are lossy in our system
        assert!(!video.is_lossless("MP4"));
        assert!(!video.is_lossless("MKV"));
        assert!(!video.is_lossless("WEBM"));
        assert!(!video.is_lossless("AVI"));
    }

    #[test]
    fn test_is_lossless_images() {
        let images = MediaType::Images;
        assert!(images.is_lossless("PNG"));
        assert!(images.is_lossless("BMP"));
        assert!(images.is_lossless("TIFF"));
        assert!(images.is_lossless("ICO"));

        assert!(!images.is_lossless("JPEG"));
        assert!(!images.is_lossless("WEBP"));
        assert!(!images.is_lossless("AVIF"));
        assert!(!images.is_lossless("GIF"));
    }

    #[test]
    fn test_is_lossless_documents() {
        let docs = MediaType::Documents;
        // All basic formats are lossless except "PDF (Optimized)"
        assert!(docs.is_lossless("PDF"));
        assert!(docs.is_lossless("DOCX"));
        assert!(docs.is_lossless("XLSX"));
        assert!(docs.is_lossless("PPTX"));
        assert!(docs.is_lossless("ODT"));
        assert!(docs.is_lossless("ODS"));
        assert!(docs.is_lossless("ODP"));
        assert!(docs.is_lossless("EPUB"));

        assert!(!docs.is_lossless("PDF (Optimized)"));
    }

    #[test]
    fn test_lossy_warning_audio() {
        let audio = MediaType::Audio;

        // Lossless to lossy should have warning
        assert!(audio.lossy_warning("WAV", "MP3").is_some());
        assert!(audio.lossy_warning("FLAC", "OGG").is_some());

        // Lossy to lossy should have warning
        assert!(audio.lossy_warning("MP3", "OGG").is_some());

        // Lossless to lossless should not
        assert!(audio.lossy_warning("WAV", "FLAC").is_none());

        // Lossy to lossless should not (rare but valid)
        assert!(audio.lossy_warning("MP3", "FLAC").is_none());
    }

    #[test]
    fn test_lossy_warning_video() {
        let video = MediaType::Video;
        // All video conversions have warning (all are lossy)
        assert!(video.lossy_warning("MP4", "MKV").is_some());
        assert!(video.lossy_warning("AVI", "MP4").is_some());
    }

    #[test]
    fn test_lossy_warning_images() {
        let images = MediaType::Images;

        // Lossless to lossy
        assert!(images.lossy_warning("PNG", "JPEG").is_some());
        assert!(images.lossy_warning("BMP", "AVIF").is_some());

        // Lossy to lossy
        assert!(images.lossy_warning("JPEG", "WEBP").is_some());

        // Lossless to lossless
        assert!(images.lossy_warning("PNG", "BMP").is_none());
    }

    #[test]
    fn test_lossy_warning_documents() {
        let docs = MediaType::Documents;

        // PDF (Optimized) downsamples to 150 PPI (lossy)
        let warning = docs.lossy_warning("PDF", "PDF (Optimized)");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("150 PPI"));

        // Other document conversions
        assert!(docs.lossy_warning("DOCX", "PDF").is_none());
        assert!(docs.lossy_warning("XLSX", "ODS").is_none());
    }

    #[test]
    fn test_display_item() {
        assert!(MediaType::Audio.display_item().contains("Audio"));
        assert!(MediaType::Audio.display_item().contains("🎵"));

        assert!(MediaType::Video.display_item().contains("Video"));
        assert!(MediaType::Video.display_item().contains("🎬"));

        assert!(MediaType::Images.display_item().contains("Images"));
        assert!(MediaType::Images.display_item().contains("🖼️ "));

        assert!(MediaType::Documents.display_item().contains("Documents"));
        assert!(MediaType::Documents.display_item().contains("📄"));
    }

    #[test]
    fn test_mediatype_display_fmt() {
        assert_eq!(format!("{}", MediaType::Audio), "Audio");
        assert_eq!(format!("{}", MediaType::Video), "Video");
        assert_eq!(format!("{}", MediaType::Images), "Images");
        assert_eq!(format!("{}", MediaType::Documents), "Documents");
    }

    #[test]
    fn test_recommended_conversions_not_empty() {
        let recs = recommended_conversions();
        assert!(!recs.is_empty());
        assert!(recs.len() >= 10); // Should have at least 10 recommendations
    }

    #[test]
    fn test_recommended_conversions_valid() {
        let recs = recommended_conversions();
        for (media_type, input, output) in recs {
            // Input should be in the mediatype's formats
            assert!(
                media_type.formats().contains(input),
                "Input {} not in {:?} formats",
                input,
                media_type
            );

            // Output should be in compatible outputs
            assert!(
                media_type.compatible_outputs(input).contains(output),
                "Output {} not compatible with {} for {:?}",
                output,
                input,
                media_type
            );
        }
    }

    #[test]
    fn test_lossless_audio_sentinel() {
        assert_eq!(LOSSLESS_AUDIO_SENTINEL, "ALL_LOSSLESS_AUDIO");
        assert!(!LOSSLESS_AUDIO_INPUTS.is_empty());
        assert!(LOSSLESS_AUDIO_INPUTS.contains(&"WAV"));
        assert!(LOSSLESS_AUDIO_INPUTS.contains(&"AIFF"));
    }

    #[test]
    fn test_compatible_audio_all_documented() {
        let audio = MediaType::Audio;
        for fmt in audio.formats() {
            let compatible = audio.compatible_outputs(fmt);
            // Every format should have some compatible outputs (or empty for invalid)
            // but documented ones should have outputs
            matches!(
                fmt,
                &"MP3" | &"WAV" | &"FLAC" | &"OGG" | &"AAC" | &"M4A" | &"OPUS" | &"AIFF" | &"WMA"
            );
            if [
                "MP3", "WAV", "FLAC", "OGG", "AAC", "M4A", "OPUS", "AIFF", "WMA",
            ]
            .contains(fmt)
            {
                assert!(
                    !compatible.is_empty(),
                    "Format {} has no compatible outputs",
                    fmt
                );
            }
        }
    }
}
