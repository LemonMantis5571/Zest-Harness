//! The small, line-oriented contract between Zest and an installed add-on.
//!
//! Add-ons are separate processes on purpose. Zest can discover and stop one
//! without loading third-party code into the desktop process or giving it
//! project content or credentials through the protocol.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const NOW_PLAYING_ID: &str = "now-playing";

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
    Control { command: MediaCommand },
    SetVolume { volume_percent: f64 },
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResponse {
    pub ok: bool,
    pub data: Option<NowPlayingView>,
    pub error: Option<String>,
}

impl PluginResponse {
    pub fn success(data: NowPlayingView) -> Self {
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
