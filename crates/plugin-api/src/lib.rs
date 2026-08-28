//! The small, line-oriented contract between Zest and an installed add-on.
//!
//! Add-ons are separate processes on purpose. Zest can discover and stop one
//! without loading third-party code into the desktop process or giving it
//! project content or credentials through the protocol.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const NOW_PLAYING_ID: &str = "now-playing";
pub const WALLPAPER_ID: &str = "wallpaper";

/// Background looks the wallpaper add-on can render, `none` first.
pub const WALLPAPER_FILTERS: [&str; 4] = ["none", "print", "frosted", "noir"];

/// The matching filter id, or `none` for anything unrecognised.
///
/// Both sides normalise: the host refuses to forward a filter it does not know,
/// and a plugin built against a newer list still has a defined behaviour when an
/// older host asks for one it has never heard of.
pub fn wallpaper_filter(value: &str) -> &'static str {
    WALLPAPER_FILTERS
        .into_iter()
        .find(|filter| *filter == value)
        .unwrap_or("none")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub protocol: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub executable: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaCommand {
    Previous,
    Toggle,
    Next,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum PluginRequest {
    Get,
    Control {
        command: MediaCommand,
    },
    SetVolume {
        volume_percent: f64,
    },
    #[serde(rename_all = "camelCase")]
    SetWallpaper {
        image_path: String,
        filter: String,
    },
    SetWallpaperFilter {
        filter: String,
    },
    ClearWallpaper,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingView {
    pub status: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork_data_url: Option<String>,
    pub source_app: Option<String>,
    pub position_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    pub volume_percent: Option<f64>,
    #[serde(default)]
    pub can_previous: Option<bool>,
    #[serde(default)]
    pub can_toggle: Option<bool>,
    #[serde(default)]
    pub can_next: Option<bool>,
    pub detail: String,
    pub observed_at: u64,
}

/// Wallpaper plugin payload. The processed image stays on disk in the plugin
/// folder; `image_file` is a relative name such as `wallpaper.png`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperView {
    pub status: String,
    pub source_name: Option<String>,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub image_file: Option<String>,
    pub detail: String,
    pub observed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResponse<T> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> PluginResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}
