//! Kick.com'un herkese açık (resmî olmayan) API'siyle konuşan ince katman.

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

pub const UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const BASE: &str = "https://kick.com";

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .user_agent(UA)
            .build()
    })
}

fn request(url: &str) -> Result<ureq::Response> {
    // Not: sıkıştırma isteği (accept-encoding) göndermiyoruz; Cloudflare bu
    // başlığın küçük harfli biçimini engelliyor.
    match agent()
        .get(url)
        .set("Accept", "application/json, text/plain, */*")
        .set("Referer", "https://kick.com/")
        .set("Origin", "https://kick.com")
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(404, _)) => bail!("bulunamadı (404)"),
        Err(ureq::Error::Status(429, _)) => {
            bail!("Kick istek sınırına takıldı (429), biraz bekleyip tekrar deneyin")
        }
        Err(ureq::Error::Status(code, _)) => bail!("Kick API hatası (HTTP {code})"),
        Err(ureq::Error::Transport(t)) => bail!("bağlantı hatası: {t}"),
    }
}

fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let text = request(url)?.into_string()?;
    serde_json::from_str(&text).with_context(|| "Kick yanıtı çözümlenemedi")
}

fn get_text(url: &str) -> Result<String> {
    Ok(request(url)?.into_string()?)
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Channel {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub playback_url: Option<String>,
    #[serde(default)]
    pub followers_count: Option<serde_json::Value>,
    #[serde(default)]
    pub livestream: Option<Livestream>,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub vod_enabled: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Livestream {
    #[serde(default)]
    pub session_title: Option<String>,
    #[serde(default)]
    pub viewer_count: Option<u64>,
    #[serde(default)]
    pub categories: Vec<Category>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Category {
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct User {
    #[serde(default)]
    pub username: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Vod {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub session_title: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Milisaniye
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub viewer_count: Option<u64>,
    #[serde(default)]
    pub is_live: bool,
}

impl Channel {
    pub fn display_name(&self) -> &str {
        match self.user.as_ref().map(|u| u.username.trim()) {
            Some(name) if !name.is_empty() => name,
            _ => &self.slug,
        }
    }

    pub fn is_live(&self) -> bool {
        self.livestream.is_some()
    }

    pub fn live_url(&self) -> Option<&str> {
        self.livestream
            .as_ref()
            .and_then(|_| self.playback_url.as_deref())
            .filter(|u| !u.is_empty())
    }

    pub fn stream_title(&self) -> &str {
        self.livestream
            .as_ref()
            .and_then(|l| l.session_title.as_deref())
            .unwrap_or("")
    }

    pub fn category(&self) -> &str {
        self.livestream
            .as_ref()
            .and_then(|l| l.categories.first())
            .map(|c| c.name.as_str())
            .unwrap_or("")
    }

    pub fn viewers(&self) -> u64 {
        self.livestream
            .as_ref()
            .and_then(|l| l.viewer_count)
            .unwrap_or(0)
    }

    pub fn followers(&self) -> u64 {
        match self.followers_count.as_ref() {
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            _ => 0,
        }
    }
}

impl Vod {
    pub fn title(&self) -> &str {
        self.session_title.as_deref().unwrap_or("(başlıksız yayın)")
    }

    pub fn url(&self) -> Option<&str> {
        self.source.as_deref().filter(|u| !u.is_empty())
    }

    /// Saniye cinsinden süre.
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration.map(|ms| ms as f64 / 1000.0)
    }

    pub fn view_count(&self) -> u64 {
        self.views.or(self.viewer_count).unwrap_or(0)
    }
}

pub fn channel(slug: &str) -> Result<Channel> {
    let slug = slug.trim().trim_start_matches('@').to_lowercase();
    if slug.is_empty() {
        bail!("Kanal adı boş");
    }
    let url = format!("{BASE}/api/v2/channels/{slug}");
    let mut ch: Channel = get_json(&url).with_context(|| format!("Kanal bilgisi alınamadı: {slug}"))?;
    if ch.slug.is_empty() {
        ch.slug = slug;
    }
    Ok(ch)
}

pub fn vods(slug: &str) -> Result<Vec<Vod>> {
    let slug = slug.trim().trim_start_matches('@').to_lowercase();
    let url = format!("{BASE}/api/v2/channels/{slug}/videos");
    let list: Vec<Vod> = get_json(&url).with_context(|| format!("Yayın tekrarları alınamadı: {slug}"))?;
    Ok(list)
}

/// Kanalın VOD listesinden tek bir tekrarı bulur (VOD bağlantısı yapıştırıldığında).
pub fn find_vod(slug: &str, video: &str) -> Result<Vod> {
    let list = vods(slug)?;
    list.into_iter()
        .find(|v| {
            v.slug.as_deref() == Some(video) || v.id.map(|id| id.to_string()) == Some(video.to_string())
        })
        .ok_or_else(|| anyhow!("Yayın tekrarı bulunamadı (kanal VOD'ları kapatmış olabilir)"))
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub width: u32,
    pub height: u32,
    pub bandwidth: u64,
    pub url: String,
}

impl Variant {
    pub fn label(&self) -> String {
        if self.height > 0 {
            format!("{}p", self.height)
        } else if self.bandwidth > 0 {
            format!("{} kbps", self.bandwidth / 1000)
        } else {
            "Kaynak".to_string()
        }
    }
}

/// HLS master listesinden kalite varyantlarını çıkarır.
/// Tek varyantlı (media) listelerde boş döner; o zaman mpv kendi seçer.
pub fn hls_variants(master_url: &str) -> Result<Vec<Variant>> {
    let text = get_text(master_url).context("Yayın kaliteleri alınamadı")?;
    let mut out: Vec<Variant> = Vec::new();
    let mut pending: Option<(u32, u32, u64)> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let mut width = 0;
            let mut height = 0;
            let mut bandwidth = 0;
            for part in attrs.split(',') {
                if let Some(v) = part.strip_prefix("RESOLUTION=") {
                    if let Some((w, h)) = v.split_once('x') {
                        width = w.parse().unwrap_or(0);
                        height = h.parse().unwrap_or(0);
                    }
                } else if let Some(v) = part.strip_prefix("BANDWIDTH=") {
                    bandwidth = v.parse().unwrap_or(0);
                }
            }
            pending = Some((width, height, bandwidth));
        } else if line.is_empty() || line.starts_with('#') {
            continue;
        } else if let Some((width, height, bandwidth)) = pending.take() {
            out.push(Variant {
                width,
                height,
                bandwidth,
                url: resolve_url(master_url, line),
            });
        }
    }

    out.sort_by(|a, b| b.height.cmp(&a.height).then(b.bandwidth.cmp(&a.bandwidth)));
    out.dedup_by(|a, b| a.width == b.width && a.height == b.height);
    Ok(out)
}

fn resolve_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        return rel.to_string();
    }
    if rel.starts_with('/') {
        if let Some(scheme_end) = base.find("://") {
            let host_start = scheme_end + 3;
            if let Some(host_end) = base[host_start..].find('/') {
                return format!("{}{}", &base[..host_start + host_end], rel);
            }
        }
    }
    match base.rfind('/') {
        Some(i) => format!("{}{}", &base[..=i], rel),
        None => rel.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Channel(String),
    Vod { channel: String, video: String },
}

/// Kullanıcının yazdığı kanal adını veya Kick bağlantısını çözümler.
pub fn parse_target(input: &str) -> Option<Target> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.contains("kick.com"))
    {
        let slug = raw
            .trim_start_matches('@')
            .trim()
            .trim_matches('/')
            .to_lowercase();
        if slug.is_empty() || slug.contains(char::is_whitespace) || slug.contains('/') {
            return None;
        }
        return Some(Target::Channel(slug));
    }

    let no_scheme = raw.split("://").nth(1).unwrap_or(raw);
    let host_end = no_scheme.find('/').unwrap_or(no_scheme.len());
    let host = no_scheme[..host_end].to_lowercase();
    if !(host == "kick.com" || host.ends_with(".kick.com")) {
        return None;
    }
    let rest = &no_scheme[host_end..];
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let channel = segments
        .first()?
        .trim_start_matches('@')
        .trim()
        .to_lowercase();
    if channel.is_empty() {
        return None;
    }
    if let Some(video) = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("video="))
        .filter(|v| !v.is_empty())
    {
        return Some(Target::Vod {
            channel,
            video: video.to_string(),
        });
    }
    if segments.len() >= 3 && segments[1] == "videos" {
        return Some(Target::Vod {
            channel,
            video: segments[2].to_string(),
        });
    }
    Some(Target::Channel(channel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(
            parse_target("xqc"),
            Some(Target::Channel("xqc".into()))
        );
        assert_eq!(
            parse_target("https://kick.com/xQc"),
            Some(Target::Channel("xqc".into()))
        );
        assert_eq!(
            parse_target("https://kick.com/xqc?video=abc-123"),
            Some(Target::Vod {
                channel: "xqc".into(),
                video: "abc-123".into()
            })
        );
        assert_eq!(
            parse_target("https://kick.com/xqc/videos/abc-123"),
            Some(Target::Vod {
                channel: "xqc".into(),
                video: "abc-123".into()
            })
        );
        assert_eq!(parse_target("iki kelime"), None);
    }

    #[test]
    fn resolves_relative_urls() {
        assert_eq!(
            resolve_url("https://a.b/c/master.m3u8", "v/720.m3u8"),
            "https://a.b/c/v/720.m3u8"
        );
        assert_eq!(
            resolve_url("https://a.b/c/master.m3u8", "/x/720.m3u8"),
            "https://a.b/x/720.m3u8"
        );
    }
}
