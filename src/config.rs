use crate::deps;
use crate::formats::MediaType;
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    Verbose,
    #[default]
    Clean,
}

impl std::fmt::Display for DisplayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verbose => write!(f, "Verbose"),
            Self::Clean => write!(f, "Clean"),
        }
    }
}

// ── Thread mode ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThreadMode {
    /// 100% of available cores.
    #[default]
    Full,
    /// 50% of available cores.
    Balanced,
    /// 25% of available cores.
    Saver,
}

impl std::fmt::Display for ThreadMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "Power (100% — all cores)"),
            Self::Balanced => write!(f, "Balanced (50% of cores)"),
            Self::Saver => write!(f, "Power saver (25% of cores)"),
        }
    }
}

impl ThreadMode {
    pub fn to_threads(self, total: usize) -> usize {
        match self {
            Self::Full => total,
            Self::Balanced => (total / 2).max(1),
            Self::Saver => total.div_ceil(4),
        }
    }
}

// ── Conflict Resolution ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConflictStrategy {
    /// Skip files where output already exists
    #[default]
    Skip,
    /// Overwrite existing output files
    Overwrite,
    /// Rename output with `.1`, `.2`, etc. suffix
    Rename,
}

impl std::fmt::Display for ConflictStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skip => write!(f, "Skip existing files"),
            Self::Overwrite => write!(f, "Overwrite existing files"),
            Self::Rename => write!(f, "Rename with suffix (.1, .2, ...)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRule {
    pub folder: String,
    pub media_type: MediaType,
    pub input_fmt: String,
    pub output_fmt: String,
    pub recursive: bool,
    pub delete_originals: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentTrigger {
    #[default]
    Watch,
    Periodic(u64),
}

const fn default_config_version() -> u32 {
    1
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    #[serde(default = "default_true")]
    pub default_recursive: bool,
    #[serde(default)]
    pub default_delete_originals: bool,
    #[serde(default)]
    pub default_folder: Option<String>,
    #[serde(default)]
    pub display_mode: DisplayMode,
    #[serde(default)]
    pub thread_mode: ThreadMode,
    #[serde(default)]
    pub agent_rules: Vec<AgentRule>,
    #[serde(default)]
    pub agent_trigger: AgentTrigger,
    #[serde(default)]
    pub conflict_strategy: ConflictStrategy,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: default_config_version(),
            default_recursive: true,
            default_delete_originals: false,
            default_folder: None,
            display_mode: DisplayMode::Clean,
            thread_mode: ThreadMode::Full,
            agent_rules: Vec::new(),
            agent_trigger: AgentTrigger::default(),
            conflict_strategy: ConflictStrategy::default(),
        }
    }
}

fn config_path() -> PathBuf {
    if let Some(custom) = std::env::var_os("RUSTY_CRUNCH_CONFIG") {
        let custom = PathBuf::from(custom);
        if !custom.as_os_str().is_empty() {
            return custom;
        }
    }
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rusty-crunch");
    dir.join("config.json")
}

fn sanitize_config(mut cfg: Config) -> Config {
    cfg.config_version = cfg.config_version.max(default_config_version());

    // Keep periodic agent trigger sane.
    if let AgentTrigger::Periodic(secs) = cfg.agent_trigger {
        cfg.agent_trigger = AgentTrigger::Periodic(secs.max(60));
    }

    // Drop clearly invalid agent rules instead of crashing at runtime.
    cfg.agent_rules.retain(|r| {
        !r.folder.trim().is_empty()
            && !r.input_fmt.trim().is_empty()
            && !r.output_fmt.trim().is_empty()
    });

    cfg
}

pub fn load() -> Config {
    let path = config_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(cfg) => sanitize_config(cfg),
                Err(e) => {
                    eprintln!(
                        "Warning: could not parse config {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    Config::default()
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: could not read config {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                Config::default()
            }
        }
    } else {
        Config::default()
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, json)?;
    Ok(())
}

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

pub fn edit_settings() -> Result<()> {
    let mut cfg = load();

    println!(
        "\n  {} {}\n",
        style("⚙").cyan(),
        style("Settings").cyan().bold(),
    );

    let action = Select::with_theme(&theme())
        .with_prompt("What would you like to do?")
        .items(&[
            "✏️  Edit defaults",
            "📤 Export settings",
            "📥 Import settings",
            "🔄 Reset to defaults",
            "🗑  Clean / Uninstall tools",
            "↩  Back",
        ])
        .default(0)
        .interact()?;

    match action {
        0 => { /* edit — fall through */ }
        1 => return export_settings(),
        2 => return import_settings(),
        3 => {
            cfg = Config::default();
            save(&cfg)?;
            println!("\n  {} Settings reset to defaults\n", style("✓").green(),);
            return Ok(());
        }
        4 => return deps::clean_installed(),
        _ => return Ok(()),
    }

    cfg.default_recursive = Confirm::with_theme(&theme())
        .with_prompt("Default: scan sub-folders?")
        .default(cfg.default_recursive)
        .interact()?;

    cfg.default_delete_originals = Confirm::with_theme(&theme())
        .with_prompt("Default: delete originals after compression?")
        .default(cfg.default_delete_originals)
        .interact()?;

    let folder_default = cfg.default_folder.clone().unwrap_or_default();
    let folder_input: String = Input::with_theme(&theme())
        .with_prompt("Default folder (leave empty for current directory)")
        .default(folder_default)
        .allow_empty(true)
        .interact_text()?;

    cfg.default_folder = if folder_input.trim().is_empty() {
        None
    } else {
        Some(folder_input.trim().to_string())
    };

    let mode_items = [
        "🔍 Verbose — show everything (best for troubleshooting)",
        "✨ Clean — clear screen between interactions",
    ];
    let mode_default = match cfg.display_mode {
        DisplayMode::Verbose => 0,
        DisplayMode::Clean => 1,
    };
    let mode_idx = Select::with_theme(&theme())
        .with_prompt("Display mode")
        .items(&mode_items)
        .default(mode_default)
        .interact()?;
    cfg.display_mode = match mode_idx {
        1 => DisplayMode::Clean,
        _ => DisplayMode::Verbose,
    };

    let thread_items = [
        format!("\u{26a1} {}", ThreadMode::Full),
        format!("\u{2696}  {}", ThreadMode::Balanced),
        format!("🌿 {}", ThreadMode::Saver),
    ];
    let thread_default = match cfg.thread_mode {
        ThreadMode::Full => 0,
        ThreadMode::Balanced => 1,
        ThreadMode::Saver => 2,
    };
    let thread_idx = Select::with_theme(&theme())
        .with_prompt("Thread usage")
        .items(&thread_items)
        .default(thread_default)
        .interact()?;
    cfg.thread_mode = match thread_idx {
        1 => ThreadMode::Balanced,
        2 => ThreadMode::Saver,
        _ => ThreadMode::Full,
    };

    cfg.conflict_strategy = crate::prompt::select_conflict_strategy()?;

    save(&cfg)?;

    println!(
        "\n  {} Settings saved to {}\n",
        style("✓").green(),
        style(config_path().display()).dim(),
    );

    Ok(())
}

fn export_settings() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        println!(
            "  {} No settings file found — using defaults",
            style("⚠").yellow()
        );
        return Ok(());
    }
    let dest: String = Input::with_theme(&theme())
        .with_prompt("Export to (file path)")
        .interact_text()?;
    let dest = dest.trim();
    if dest.is_empty() {
        return Ok(());
    }
    std::fs::copy(&path, dest)?;
    println!(
        "\n  {} Exported to {}\n",
        style("✓").green(),
        style(dest).dim(),
    );
    Ok(())
}

fn import_settings() -> Result<()> {
    let src: String = Input::with_theme(&theme())
        .with_prompt("Import from (file path)")
        .interact_text()?;
    let src = src.trim();
    if src.is_empty() {
        return Ok(());
    }
    let content =
        std::fs::read_to_string(src).map_err(|e| anyhow::anyhow!("Cannot read {}: {}", src, e))?;
    // Validate that it parses as a valid Config
    let _: Config = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid settings file: {}", e))?;
    let dest = config_path();
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&dest, &content)?;
    println!(
        "\n  {} Settings imported from {}\n",
        style("✓").green(),
        style(src).dim(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_display_mode_is_clean() {
        let cfg = Config::default();
        assert_eq!(cfg.display_mode, DisplayMode::Clean);
    }

    #[test]
    fn deserialize_config_with_agent_rules() {
        let json = r#"{
            "default_recursive": true,
            "default_delete_originals": false,
            "default_folder": null,
            "display_mode": "Clean",
            "agent_rules": [
                {
                    "folder": "/tmp/test",
                    "media_type": "Audio",
                    "input_fmt": "WAV",
                    "output_fmt": "FLAC",
                    "recursive": true,
                    "delete_originals": false
                }
            ],
            "agent_trigger": "Watch"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.agent_rules.len(), 1);
        assert_eq!(cfg.agent_rules[0].input_fmt, "WAV");
        assert_eq!(cfg.agent_rules[0].output_fmt, "FLAC");
        assert_eq!(cfg.agent_trigger, AgentTrigger::Watch);
    }

    #[test]
    fn deserialize_periodic_trigger() {
        let json = r#"{
            "default_recursive": true,
            "default_delete_originals": false,
            "default_folder": null,
            "agent_trigger": {"Periodic": 300}
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.agent_trigger, AgentTrigger::Periodic(300));
    }

    #[test]
    fn deserialize_legacy_config_without_agent_fields() {
        let json = r#"{
            "default_recursive": true,
            "default_delete_originals": false,
            "default_folder": null,
            "display_mode": "Verbose"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.agent_rules.is_empty());
        assert_eq!(cfg.agent_trigger, AgentTrigger::Watch);
    }

    #[test]
    fn roundtrip_serialize_config() {
        let cfg = Config {
            config_version: 1,
            default_recursive: false,
            default_delete_originals: true,
            default_folder: Some("/test".into()),
            display_mode: DisplayMode::Clean,
            agent_rules: vec![AgentRule {
                folder: "/tmp".into(),
                media_type: MediaType::Images,
                input_fmt: "BMP".into(),
                output_fmt: "PNG".into(),
                recursive: true,
                delete_originals: false,
            }],
            agent_trigger: AgentTrigger::Periodic(600),
            thread_mode: ThreadMode::Balanced,
            conflict_strategy: ConflictStrategy::Rename,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let cfg2: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg2.agent_rules.len(), 1);
        assert_eq!(cfg2.agent_trigger, AgentTrigger::Periodic(600));
    }

    #[test]
    fn test_thread_mode_calculation() {
        assert_eq!(ThreadMode::Full.to_threads(8), 8);
        assert_eq!(ThreadMode::Balanced.to_threads(8), 4);
        assert_eq!(ThreadMode::Saver.to_threads(8), 2);

        // Ensure minimum of 1 thread
        assert_eq!(ThreadMode::Balanced.to_threads(1), 1);
        assert_eq!(ThreadMode::Saver.to_threads(1), 1);
    }

    #[test]
    fn test_thread_mode_display() {
        assert!(ThreadMode::Full.to_string().contains("100%"));
        assert!(ThreadMode::Balanced.to_string().contains("50%"));
        assert!(ThreadMode::Saver.to_string().contains("25%"));
    }

    #[test]
    fn test_conflict_strategy_default() {
        assert_eq!(ConflictStrategy::default(), ConflictStrategy::Skip);
    }

    #[test]
    fn test_conflict_strategy_display() {
        assert_eq!(ConflictStrategy::Skip.to_string(), "Skip existing files");
        assert_eq!(
            ConflictStrategy::Overwrite.to_string(),
            "Overwrite existing files"
        );
        assert_eq!(
            ConflictStrategy::Rename.to_string(),
            "Rename with suffix (.1, .2, ...)"
        );
    }

    #[test]
    fn test_thread_mode_edge_cases() {
        // Test with very small core counts
        assert!(ThreadMode::Full.to_threads(1) >= 1);
        assert!(ThreadMode::Balanced.to_threads(1) >= 1);
        assert!(ThreadMode::Saver.to_threads(1) >= 1);

        // Test with large core counts
        assert_eq!(ThreadMode::Full.to_threads(64), 64);
        assert_eq!(ThreadMode::Balanced.to_threads(64), 32);
        assert_eq!(ThreadMode::Saver.to_threads(64), 16);

        // Test that percentages are correct
        for cores in [1, 2, 4, 8, 16, 32, 64] {
            assert_eq!(ThreadMode::Full.to_threads(cores), cores);
            assert_eq!(ThreadMode::Balanced.to_threads(cores), cores.div_ceil(2));
            assert_eq!(ThreadMode::Saver.to_threads(cores), cores.div_ceil(4));
        }
    }

    #[test]
    fn test_conflict_strategy_all_variants() {
        let strategies = [
            ConflictStrategy::Skip,
            ConflictStrategy::Overwrite,
            ConflictStrategy::Rename,
        ];

        for strategy in &strategies {
            // Each should have a non-empty display string
            let display = strategy.to_string();
            assert!(!display.is_empty());
        }
    }

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.thread_mode, ThreadMode::Full);
        assert!(cfg.agent_rules.is_empty());
        // Default trigger should be Watch
        assert_eq!(cfg.agent_trigger, AgentTrigger::Watch);
    }

    #[test]
    fn test_agent_rule_creation() {
        let rule = AgentRule {
            folder: "/some/folder".to_string(),
            media_type: crate::formats::MediaType::Audio,
            input_fmt: "MP3".into(),
            output_fmt: "FLAC".into(),
            recursive: true,
            delete_originals: false,
        };

        assert_eq!(rule.media_type, crate::formats::MediaType::Audio);
        assert_eq!(rule.input_fmt, "MP3");
        assert_eq!(rule.output_fmt, "FLAC");
        assert!(rule.recursive);
        assert!(!rule.delete_originals);
    }

    #[test]
    fn test_agent_trigger_periodic() {
        let trigger = AgentTrigger::Periodic(300);
        assert_eq!(trigger, AgentTrigger::Periodic(300));
    }

    #[test]
    fn test_agent_trigger_watch() {
        let trigger = AgentTrigger::Watch;
        assert_eq!(trigger, AgentTrigger::Watch);
    }

    #[test]
    fn test_config_with_multiple_rules() {
        let mut cfg = Config::default();

        // Add multiple rules
        for i in 0..5 {
            cfg.agent_rules.push(AgentRule {
                folder: format!("/folder{}", i),
                media_type: crate::formats::MediaType::Images,
                input_fmt: "JPEG".into(),
                output_fmt: "WEBP".into(),
                recursive: false,
                delete_originals: i % 2 == 0,
            });
        }

        assert_eq!(cfg.agent_rules.len(), 5);

        // Verify all rules are stored correctly
        for (i, rule) in cfg.agent_rules.iter().enumerate() {
            assert_eq!(rule.folder, format!("/folder{}", i));
            assert_eq!(rule.delete_originals, i % 2 == 0);
        }
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let original = Config {
            config_version: 1,
            default_recursive: true,
            default_delete_originals: true,
            default_folder: Some("/default".to_string()),
            display_mode: DisplayMode::Verbose,
            thread_mode: ThreadMode::Saver,
            agent_rules: vec![
                AgentRule {
                    folder: "/music".to_string(),
                    media_type: crate::formats::MediaType::Audio,
                    input_fmt: "WAV".into(),
                    output_fmt: "FLAC".into(),
                    recursive: true,
                    delete_originals: true,
                },
                AgentRule {
                    folder: "/images".to_string(),
                    media_type: crate::formats::MediaType::Images,
                    input_fmt: "BMP".into(),
                    output_fmt: "PNG".into(),
                    recursive: false,
                    delete_originals: false,
                },
            ],
            agent_trigger: AgentTrigger::Periodic(600),
            conflict_strategy: ConflictStrategy::Rename,
        };

        // Serialize
        let json = serde_json::to_string(&original).unwrap();

        // Deserialize
        let restored: Config = serde_json::from_str(&json).unwrap();

        // Verify all fields match
        assert_eq!(restored.agent_rules.len(), 2);
        assert_eq!(restored.thread_mode, ThreadMode::Saver);
        assert_eq!(restored.agent_rules[0].folder, "/music");
        assert_eq!(restored.agent_rules[1].folder, "/images");
        assert_eq!(restored.display_mode, DisplayMode::Verbose);
    }

    #[test]
    fn test_agent_rule_various_media_types() {
        let media_types = [
            crate::formats::MediaType::Audio,
            crate::formats::MediaType::Video,
            crate::formats::MediaType::Images,
            crate::formats::MediaType::Documents,
        ];

        for media_type in &media_types {
            let rule = AgentRule {
                folder: "/test".to_string(),
                media_type: *media_type,
                input_fmt: "TEST".into(),
                output_fmt: "OUT".into(),
                recursive: true,
                delete_originals: false,
            };

            assert_eq!(rule.media_type, *media_type);
        }
    }

    #[test]
    fn test_config_folder_paths() {
        let paths = vec![
            "/absolute/path/music",
            "relative/path/images",
            "./current/dir/videos",
            "/path/with spaces/and-dashes_underscores",
        ];

        for path_str in paths {
            let rule = AgentRule {
                folder: path_str.to_string(),
                media_type: crate::formats::MediaType::Audio,
                input_fmt: "MP3".into(),
                output_fmt: "FLAC".into(),
                recursive: true,
                delete_originals: false,
            };

            assert_eq!(rule.folder, path_str);
        }
    }

    #[test]
    fn test_display_mode_transitions() {
        assert_eq!(format!("{}", DisplayMode::Verbose), "Verbose");
        assert_eq!(format!("{}", DisplayMode::Clean), "Clean");
    }

    #[test]
    fn test_agent_trigger_variants() {
        let watch = AgentTrigger::Watch;
        let periodic = AgentTrigger::Periodic(300);

        assert_ne!(watch, periodic);
        assert_eq!(watch, AgentTrigger::Watch);
        assert_eq!(periodic, AgentTrigger::Periodic(300));
    }

    #[test]
    fn test_config_with_optional_folder() {
        let mut cfg = Config::default();
        assert!(cfg.default_folder.is_none());

        cfg.default_folder = Some("/home/user".to_string());
        assert_eq!(cfg.default_folder, Some("/home/user".to_string()));
    }

    #[test]
    fn test_agent_rule_flags() {
        let all_combos = vec![(true, true), (true, false), (false, true), (false, false)];

        for (recursive, delete) in all_combos {
            let rule = AgentRule {
                folder: "/test".to_string(),
                media_type: crate::formats::MediaType::Audio,
                input_fmt: "IN".into(),
                output_fmt: "OUT".into(),
                recursive,
                delete_originals: delete,
            };

            assert_eq!(rule.recursive, recursive);
            assert_eq!(rule.delete_originals, delete);
        }
    }
}
