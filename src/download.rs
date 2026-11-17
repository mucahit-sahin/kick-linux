//! Built-in HLS downloader used for offline replay playback.
//!
//! A Kick VOD is an HLS stream: a playlist (`.m3u8`) that references a number
//! of media segments. We download the chosen quality's playlist and all of its
//! segments into a local directory, rewrite the playlist so that it points at
//! the local files, and let mpv play that local playlist when offline.
//!
//! No external tools (ffmpeg/yt-dlp) are required.

use crate::{api, config};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many segments are fetched at the same time.
const WORKERS: usize = 8;
/// Emit a progress event at most this often (the UI redraws on each event).
const PROGRESS_INTERVAL_MS: u128 = 250;

pub enum Event {
    Progress {
        done: usize,
        total: usize,
        bytes: u64,
    },
    Finished(Result<DownloadOutcome, String>),
}

pub struct DownloadOutcome {
    pub item: config::DownloadItem,
}

/// Handle returned by `begin`; dropping it does not stop the download.
pub struct Download {
    pub cancel: Arc<AtomicBool>,
    pub rx: async_channel::Receiver<Event>,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub master: String,
    /// Concrete media playlist URL. `None` selects the best available quality.
    pub variant: Option<String>,
    pub quality: String,
    pub title: String,
    pub channel: String,
    pub channel_slug: String,
    pub subtitle: String,
    pub duration: Option<f64>,
    pub id: String,
    pub dir: String,
}

/// Creates a unique id and the matching storage directory name.
pub fn new_ids(slug: &str) -> (String, String) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let clean: String = slug
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let id = format!("{clean}-{millis}-{counter}");
    (id.clone(), id)
}

/// Starts a download on a background thread. Progress and the final result
/// are delivered through the returned channel.
pub fn begin(params: Params) -> Download {
    let (tx, rx) = async_channel::unbounded::<Event>();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let result = run(&params, &worker_cancel, &tx);
        let _ = tx.send_blocking(Event::Finished(result.map_err(|err| format!("{err:#}"))));
    });
    Download { cancel, rx }
}

pub fn playlist_path(item: &config::DownloadItem) -> PathBuf {
    config::downloads_dir().join(&item.dir).join("playlist.m3u8")
}

/// Removes download directories that are no longer referenced in the config.
/// This also cleans up incomplete downloads left behind by a previous run.
pub fn cleanup(items: &[config::DownloadItem]) {
    let Ok(entries) = fs::read_dir(config::downloads_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !items.iter().any(|item| item.dir == name) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub fn remove(item: &config::DownloadItem) {
    let _ = fs::remove_dir_all(config::downloads_dir().join(&item.dir));
}

fn run(
    params: &Params,
    cancel: &AtomicBool,
    tx: &async_channel::Sender<Event>,
) -> Result<DownloadOutcome> {
    let playlist_url = choose_playlist_url(params)?;
    let text = api::get_text(&playlist_url).context("Could not download the stream playlist")?;
    let (playlist, assets) = build_playlist(&text, &playlist_url);
    if assets.is_empty() {
        bail!("This quality has no media segments to download");
    }

    let dir = config::downloads_dir().join(&params.dir);
    fs::create_dir_all(&dir).with_context(|| format!("Could not create {}", dir.display()))?;

    let total = assets.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let started = Instant::now();
    let last_emit = Mutex::new(started);
    let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let workers = WORKERS.min(total).max(1);
    let error_for_workers = first_error.clone();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let assets = &assets;
            let dir = &dir;
            let cancel = &cancel;
            let next = &next;
            let done = &done;
            let bytes = &bytes;
            let last_emit = &last_emit;
            let error = &error_for_workers;
            let tx = &tx;
            scope.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= assets.len() {
                    break;
                }
                let asset = &assets[index];
                match download_asset(asset, dir) {
                    Ok(size) => {
                        bytes.fetch_add(size, Ordering::Relaxed);
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        let emit = {
                            let mut last = last_emit.lock().unwrap();
                            if finished == assets.len() || last.elapsed() > std::time::Duration::from_millis(PROGRESS_INTERVAL_MS as u64) {
                                *last = Instant::now();
                                true
                            } else {
                                false
                            }
                        };
                        if emit {
                            let _ = tx.send_blocking(Event::Progress {
                                done: finished,
                                total: assets.len(),
                                bytes: bytes.load(Ordering::Relaxed),
                            });
                        }
                    }
                    Err(err) => {
                        let mut first = error.lock().unwrap();
                        if first.is_none() {
                            *first = Some(format!("{}: {err:#}", asset.url));
                        }
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            });
        }
    });

    if let Some(err) = first_error.lock().unwrap().take() {
        let _ = fs::remove_dir_all(&dir);
        bail!(err);
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_dir_all(&dir);
        bail!("Download was cancelled");
    }

    fs::write(dir.join("playlist.m3u8"), playlist).context("Could not write the local playlist")?;

    let total_bytes = bytes.load(Ordering::Relaxed);
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let item = config::DownloadItem {
        id: params.id.clone(),
        title: params.title.clone(),
        channel: params.channel.clone(),
        channel_slug: params.channel_slug.clone(),
        subtitle: params.subtitle.clone(),
        quality: params.quality.clone(),
        master: params.master.clone(),
        dir: params.dir.clone(),
        duration: params.duration,
        position: 0.0,
        bytes: total_bytes,
        created_at,
    };
    Ok(DownloadOutcome { item })
}

fn choose_playlist_url(params: &Params) -> Result<String> {
    if let Some(url) = &params.variant {
        return Ok(url.clone());
    }
    let variants = api::hls_variants(&params.master).unwrap_or_default();
    if let Some(best) = variants.first() {
        return Ok(best.url.clone());
    }
    // With no variants the master URL is itself a media playlist.
    Ok(params.master.clone())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AssetKind {
    Segment,
    Map,
    Key,
}

#[derive(Clone, Debug, PartialEq)]
struct Asset {
    url: String,
    name: String,
    kind: AssetKind,
}

fn download_asset(asset: &Asset, dir: &Path) -> Result<u64> {
    let data = api::fetch_bytes(&asset.url)
        .with_context(|| format!("Could not download {}", asset.url))?;
    let size = data.len() as u64;
    fs::write(dir.join(&asset.name), data)
        .with_context(|| format!("Could not write {}", asset.name))?;
    Ok(size)
}

/// Rewrites an HLS playlist so that every referenced resource points at a
/// local file, and returns the list of resources that must be downloaded.
/// Pure function so the parsing can be unit tested without network access.
fn build_playlist(text: &str, base: &str) -> (String, Vec<Asset>) {
    let mut out: Vec<String> = Vec::new();
    let mut assets: Vec<Asset> = Vec::new();
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut segment_index = 0usize;
    let mut map_index = 0usize;
    let mut key_index = 0usize;

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#EXT-X-MAP:") {
            if let Some((prefix, uri, suffix)) = uri_attribute(rest) {
                let url = api::resolve_url(base, uri);
                let name = if let Some(existing) = seen.get(&url) {
                    existing.clone()
                } else {
                    let name = format!("init_{map_index}.bin");
                    map_index += 1;
                    seen.insert(url.clone(), name.clone());
                    assets.push(Asset { url: url.clone(), name: name.clone(), kind: AssetKind::Map });
                    name
                };
                out.push(format!("#EXT-X-MAP:{prefix}{name}{suffix}"));
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("#EXT-X-KEY:") {
            if let Some((prefix, uri, suffix)) = uri_attribute(rest) {
                let url = api::resolve_url(base, uri);
                let name = if let Some(existing) = seen.get(&url) {
                    existing.clone()
                } else {
                    let name = format!("key_{key_index}.bin");
                    key_index += 1;
                    seen.insert(url.clone(), name.clone());
                    assets.push(Asset { url: url.clone(), name: name.clone(), kind: AssetKind::Key });
                    name
                };
                out.push(format!("#EXT-X-KEY:{prefix}{name}{suffix}"));
                continue;
            }
        }
        if trimmed.starts_with('#') {
            out.push(line.to_string());
            continue;
        }
        // A media segment URL. Keep tags such as EXT-X-BYTERANGE untouched;
        // mpv/ffmpeg can apply them to the local file in the same way.
        let url = api::resolve_url(base, trimmed);
        let name = if let Some(existing) = seen.get(&url) {
            existing.clone()
        } else {
            let name = segment_name(&url, segment_index);
            segment_index += 1;
            seen.insert(url.clone(), name.clone());
            assets.push(Asset { url: url.clone(), name: name.clone(), kind: AssetKind::Segment });
            name
        };
        out.push(name);
    }

    let mut playlist = out.join("\n");
    playlist.push('\n');
    (playlist, assets)
}

/// Splits a comma-separated tag attribute list at `URI="..."`, returning the
/// text before the URI value (including `URI="`), the value, and the rest.
fn uri_attribute(text: &str) -> Option<(&str, &str, &str)> {
    let start = text.find("URI=\"")?;
    let value_start = start + 5;
    let remaining = &text[value_start..];
    let end = remaining.find('"')?;
    Some((&text[..value_start], &remaining[..end], &remaining[end..]))
}

fn segment_name(url: &str, index: usize) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    let extension = last
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .filter(|ext| {
            !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or_else(|| "ts".to_string());
    format!("seg_{index:05}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_a_media_playlist() {
        let text = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n\
                    #EXTINF:4.0,\nseg1.ts?token=abc\n\
                    #EXTINF:4.0,\n/vod/seg2.ts\n#EXT-X-ENDLIST\n";
        let (playlist, assets) = build_playlist(text, "https://cdn.example.com/vod/master.m3u8");
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].url, "https://cdn.example.com/vod/seg1.ts?token=abc");
        assert_eq!(assets[0].name, "seg_00000.ts");
        assert_eq!(assets[1].url, "https://cdn.example.com/vod/seg2.ts");
        assert_eq!(assets[1].name, "seg_00001.ts");
        assert!(playlist.contains("#EXTINF:4.0,\nseg_00000.ts"));
        assert!(playlist.contains("\nseg_00001.ts\n"));
        assert!(playlist.starts_with("#EXTM3U\n"));
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn deduplicates_byte_range_segments() {
        let text = "#EXTM3U\n#EXT-X-BYTERANGE:100@0\n#EXTINF:2.0,\nvideo.ts\n\
                    #EXT-X-BYTERANGE:100@100\n#EXTINF:2.0,\nvideo.ts\n";
        let (playlist, assets) = build_playlist(text, "https://cdn.example.com/vod/list.m3u8");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].name, "seg_00000.ts");
        assert_eq!(playlist.matches("seg_00000.ts").count(), 2);
        assert_eq!(playlist.matches("#EXT-X-BYTERANGE").count(), 2);
    }

    #[test]
    fn rewrites_map_and_key_uris() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://cdn.example.com/key.bin\",IV=0x00\n\
                    #EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\na/part.m4s\n";
        let (playlist, assets) = build_playlist(text, "https://cdn.example.com/vod/list.m3u8");
        assert_eq!(assets.len(), 3);
        assert_eq!(assets[0].kind, AssetKind::Key);
        assert_eq!(assets[0].url, "https://cdn.example.com/key.bin");
        assert_eq!(assets[1].kind, AssetKind::Map);
        assert_eq!(assets[1].url, "https://cdn.example.com/vod/init.mp4");
        assert_eq!(assets[2].name, "seg_00000.m4s");
        assert!(playlist.contains("URI=\"key_0.bin\""));
        assert!(playlist.contains("URI=\"init_0.bin\""));
        assert!(playlist.contains("seg_00000.m4s"));
    }
}
