//! Persistent state for followed channels and player settings.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Followed {
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResumeVideo {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub subtitle: String,
    pub position: f64,
    pub duration: Option<f64>,
}

/// A replay that was downloaded for offline playback.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DownloadItem {
    /// Stable identifier, also used as the name of the storage directory.
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_slug: String,
    pub subtitle: String,
    /// Human-readable quality label chosen at download time.
    pub quality: String,
    /// Original online master URL (used to avoid duplicate downloads).
    pub master: String,
    /// Directory under `downloads_dir()` holding the playlist and segments.
    pub dir: String,
    pub duration: Option<f64>,
    #[serde(default)]
    pub position: f64,
    /// Total number of bytes downloaded.
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub followed: Vec<Followed>,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
    /// Automatically skip silent parts in VODs.
    #[serde(default)]
    pub skip_silence: bool,
    /// Derive the noise threshold from the audio instead of using a fixed value.
    #[serde(default)]
    pub auto_silence: bool,
    /// Audio level threshold (dB) below which audio counts as silence.
    #[serde(default = "default_silence_noise")]
    pub silence_noise_db: i32,
    /// Minimum silence duration to report (seconds).
    #[serde(default = "default_silence_duration")]
    pub silence_duration: f64,
    #[serde(default)]
    pub resume_positions: HashMap<String, f64>,
    #[serde(default)]
    pub resume_videos: Vec<ResumeVideo>,
    #[serde(default)]
    pub downloads: Vec<DownloadItem>,
}

fn default_volume() -> f64 {
    100.0
}

fn default_silence_noise() -> i32 {
    -35
}

fn default_silence_duration() -> f64 {
    0.5
}

impl Default for Config {
    fn default() -> Self {
        Self {
            followed: Vec::new(),
            language: "auto".to_string(),
            volume: default_volume(),
            muted: false,
            skip_silence: false,
            auto_silence: false,
            silence_noise_db: default_silence_noise(),
            silence_duration: default_silence_duration(),
            resume_positions: HashMap::new(),
            resume_videos: Vec::new(),
            downloads: Vec::new(),
        }
    }
}

impl Config {
    pub fn is_following(&self, slug: &str) -> bool {
        self.followed.iter().any(|f| f.slug == slug)
    }

    pub fn add(&mut self, slug: &str, name: &str) -> bool {
        if self.is_following(slug) {
            return false;
        }
        self.followed.push(Followed {
            slug: slug.to_string(),
            name: name.to_string(),
        });
        self.followed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        true
    }

    pub fn remove(&mut self, slug: &str) -> bool {
        let before = self.followed.len();
        self.followed.retain(|f| f.slug != slug);
        before != self.followed.len()
    }
}

pub fn config_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("kick-lite")
}

/// Where downloaded HLS playlists and their segments are stored.
/// Media files belong in the data directory, not next to the settings.
pub fn downloads_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("kick-lite").join("downloads")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("Could not read settings file ({}): {err}", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let tmp = dir.join("config.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(cfg)?)?;
    fs::rename(tmp, config_path())?;
    Ok(())
}
