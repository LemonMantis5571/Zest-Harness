use std::io::{self, Read};

use zest_plugin_api::{MediaCommand, NowPlayingView, PluginRequest, PluginResponse};

const MAX_REQUEST_BYTES: usize = 32 * 1024;

fn main() {
    let response = match read_request().and_then(handle) {
        Ok(data) => PluginResponse::success(data),
        Err(error) => PluginResponse::failure(error),
    };

    // stdout is the protocol. Diagnostics must never be printed here because
    // Zest treats the first JSON value as the whole response.
    println!(
        "{}",
        serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"ok":false,"data":null,"error":"The add-on could not reply."}"#.into()
        })
    );
}

fn read_request() -> Result<PluginRequest, String> {
    let mut raw = String::new();
    io::stdin()
        .take(MAX_REQUEST_BYTES as u64)
        .read_to_string(&mut raw)
        .map_err(|error| format!("could not read request: {error}"))?;
    serde_json::from_str(raw.trim()).map_err(|error| format!("invalid request: {error}"))
}

#[cfg(windows)]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn handle(request: PluginRequest) -> Result<NowPlayingView, String> {
    match request {
        PluginRequest::Get => read_now_playing(),
        PluginRequest::Control { command } => control(command),
        PluginRequest::SetVolume { volume_percent } => set_volume(volume_percent),
        PluginRequest::SetWallpaper { .. }
        | PluginRequest::SetWallpaperFilter { .. }
        | PluginRequest::ClearWallpaper => Err("This add-on does not support that.".into()),
    }
}

#[cfg(not(windows))]
fn read_now_playing() -> Result<NowPlayingView, String> {
    Err("This add-on only works on Windows.".into())
}

#[cfg(not(windows))]
fn control(_command: MediaCommand) -> Result<NowPlayingView, String> {
    Err("This add-on only works on Windows.".into())
}

#[cfg(not(windows))]
fn set_volume(_volume_percent: f64) -> Result<NowPlayingView, String> {
    Err("This add-on only works on Windows.".into())
}

#[cfg(windows)]
fn read_now_playing() -> Result<NowPlayingView, String> {
    Ok(read_windows_now_playing())
}

#[cfg(windows)]
fn control(command: MediaCommand) -> Result<NowPlayingView, String> {
    use std::thread;
    use std::time::Duration;

    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus;

    let session = current_session()?;
    let playback_info = session
        .GetPlaybackInfo()
        .map_err(|error| format!("could not read playback state: {error}"))?;
    let capabilities = playback_capabilities(&playback_info);
    if matches!(command_enabled(&capabilities, &command), Some(false)) {
        return Err("This music app does not support that button.".into());
    }

    let ok = match command {
        MediaCommand::Previous => session
            .TrySkipPreviousAsync()
            .map_err(|error| format!("could not skip back: {error}"))?
            .get()
            .map_err(|error| format!("could not skip back: {error}"))?,
        MediaCommand::Next => session
            .TrySkipNextAsync()
            .map_err(|error| format!("could not skip forward: {error}"))?
            .get()
            .map_err(|error| format!("could not skip forward: {error}"))?,
        MediaCommand::Toggle => {
            let status = playback_info
                .PlaybackStatus()
                .map_err(|error| format!("could not read playback state: {error}"))?;
            if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
                session
                    .TryPauseAsync()
                    .map_err(|error| format!("could not pause: {error}"))?
                    .get()
                    .map_err(|error| format!("could not pause: {error}"))?
            } else {
                session
                    .TryPlayAsync()
                    .map_err(|error| format!("could not play: {error}"))?
                    .get()
                    .map_err(|error| format!("could not play: {error}"))?
            }
        }
    };

    if !ok {
        return Err(match command {
            MediaCommand::Previous | MediaCommand::Next => {
                "This music app does not support that button.".into()
            }
            MediaCommand::Toggle => "This music app did not accept the button.".into(),
        });
    }

    // Media apps update their session data asynchronously. A short pause gives
    // the next read a chance to return the new title and play state.
    thread::sleep(Duration::from_millis(120));
    Ok(read_windows_now_playing())
}

#[cfg(windows)]
fn set_volume(volume_percent: f64) -> Result<NowPlayingView, String> {
    set_windows_volume(volume_percent.clamp(0.0, 100.0) / 100.0)?;
    Ok(read_windows_now_playing())
}

#[cfg(windows)]
fn current_session(
) -> Result<windows::Media::Control::GlobalSystemMediaTransportControlsSession, String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|error| format!("could not access media controls: {error}"))?
        .get()
        .map_err(|error| format!("could not access media controls: {error}"))?
        .GetCurrentSession()
        .map_err(|error| format!("no music app is active: {error}"))
}

#[cfg(windows)]
fn read_windows_now_playing() -> NowPlayingView {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    let result: Result<NowPlayingView, String> = (|| {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .map_err(|error| error.to_string())?
            .get()
            .map_err(|error| error.to_string())?;
        let session = manager
            .GetCurrentSession()
            .map_err(|error| error.to_string())?;
        let playback_info = session
            .GetPlaybackInfo()
            .map_err(|error| error.to_string())?;
        let playback_status = playback_info
            .PlaybackStatus()
            .map_err(|error| error.to_string())?;
        let capabilities = playback_capabilities(&playback_info);
        let properties = session
            .TryGetMediaPropertiesAsync()
            .map_err(|error| error.to_string())?
            .get()
            .map_err(|error| error.to_string())?;
        let timeline = session.GetTimelineProperties().ok();
        let status = match playback_status {
            status
                if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing =>
            {
                "playing"
            }
            status if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => {
                "paused"
            }
            status
                if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped =>
            {
                "stopped"
            }
            _ => "idle",
        };

        Ok(NowPlayingView {
            status: status.into(),
            title: non_empty(properties.Title().ok().map(|value| value.to_string())),
            artist: non_empty(properties.Artist().ok().map(|value| value.to_string())),
            album: non_empty(properties.AlbumTitle().ok().map(|value| value.to_string())),
            artwork_data_url: properties.Thumbnail().ok().and_then(read_thumbnail),
            source_app: non_empty(
                session
                    .SourceAppUserModelId()
                    .ok()
                    .map(|value| value.to_string()),
            ),
            position_secs: timeline
                .as_ref()
                .and_then(|value| value.Position().ok())
                .and_then(time_span_secs),
            duration_secs: timeline
                .as_ref()
                .and_then(|value| value.EndTime().ok())
                .and_then(time_span_secs),
            volume_percent: read_windows_volume().ok().map(|value| value * 100.0),
            can_previous: capabilities.can_previous,
            can_toggle: capabilities.can_toggle,
            can_next: capabilities.can_next,
            detail: "Ready".into(),
            observed_at: now_secs(),
        })
    })();

    result.unwrap_or_else(|_| NowPlayingView {
        status: "idle".into(),
        title: None,
        artist: None,
        album: None,
        artwork_data_url: None,
        source_app: None,
        position_secs: None,
        duration_secs: None,
        volume_percent: read_windows_volume().ok().map(|value| value * 100.0),
        can_previous: None,
        can_toggle: None,
        can_next: None,
        detail: "No music playing.".into(),
        observed_at: now_secs(),
    })
}

#[cfg(windows)]
#[derive(Default)]
struct PlaybackCapabilities {
    can_previous: Option<bool>,
    can_toggle: Option<bool>,
    can_next: Option<bool>,
}

#[cfg(windows)]
fn playback_capabilities(
    playback_info: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackInfo,
) -> PlaybackCapabilities {
    let Ok(controls) = playback_info.Controls() else {
        return PlaybackCapabilities::default();
    };

    let can_toggle = match (
        controls.IsPlayPauseToggleEnabled().ok(),
        controls.IsPlayEnabled().ok(),
        controls.IsPauseEnabled().ok(),
    ) {
        (Some(toggle), Some(play), Some(pause)) => Some(toggle || play || pause),
        (toggle, play, pause) => toggle.or(play).or(pause),
    };

    PlaybackCapabilities {
        can_previous: controls.IsPreviousEnabled().ok(),
        can_toggle,
        can_next: controls.IsNextEnabled().ok(),
    }
}

#[cfg(windows)]
fn command_enabled(capabilities: &PlaybackCapabilities, command: &MediaCommand) -> Option<bool> {
    match command {
        MediaCommand::Previous => capabilities.can_previous,
        MediaCommand::Toggle => capabilities.can_toggle,
        MediaCommand::Next => capabilities.can_next,
    }
}

#[cfg(windows)]
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(windows)]
fn time_span_secs(value: windows::Foundation::TimeSpan) -> Option<f64> {
    (value.Duration > 0).then_some(value.Duration as f64 / 10_000_000.0)
}

#[cfg(windows)]
fn read_thumbnail(
    reference: windows::Storage::Streams::IRandomAccessStreamReference,
) -> Option<String> {
    use base64::Engine as _;
    use windows::Storage::Streams::DataReader;

    const MAX_ARTWORK_BYTES: u64 = 1_500_000;
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?.min(MAX_ARTWORK_BYTES);
    if size == 0 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let loaded = reader.LoadAsync(size as u32).ok()?.get().ok()?;
    if loaded == 0 {
        return None;
    }
    let mut bytes = vec![0; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let content_type = stream
        .ContentType()
        .ok()
        .map(|value| value.to_string())
        .filter(|value| value.starts_with("image/"))
        .unwrap_or_else(|| "image/jpeg".into());
    Some(format!(
        "data:{content_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(windows)]
fn with_audio_endpoint<T>(
    operation: impl FnOnce(
        &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
    ) -> Result<T, String>,
) -> Result<T, String> {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        if initialized.is_err() {
            return Err(format!(
                "could not initialize audio controls: 0x{:08x}",
                initialized.0 as u32
            ));
        }
        let result = (|| {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|error| {
                    format!("could not access the default audio device: {error}")
                })?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .map_err(|error| format!("could not access the default audio device: {error}"))?;
            let endpoint = device
                .Activate::<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume>(
                    CLSCTX_ALL, None,
                )
                .map_err(|error| format!("could not access system volume: {error}"))?;
            operation(&endpoint)
        })();
        CoUninitialize();
        result
    }
}

#[cfg(windows)]
fn read_windows_volume() -> Result<f64, String> {
    with_audio_endpoint(|endpoint| unsafe {
        endpoint
            .GetMasterVolumeLevelScalar()
            .map(|value| value.clamp(0.0, 1.0) as f64)
            .map_err(|error| format!("could not read system volume: {error}"))
    })
}

#[cfg(windows)]
fn set_windows_volume(volume: f64) -> Result<(), String> {
    with_audio_endpoint(|endpoint| unsafe {
        endpoint
            .SetMasterVolumeLevelScalar(volume as f32, std::ptr::null())
            .map_err(|error| format!("could not set system volume: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zest_plugin_api::PROTOCOL_VERSION;

    #[test]
    fn request_round_trips() {
        let request = PluginRequest::Control {
            command: MediaCommand::Next,
        };
        let raw = serde_json::to_string(&request).expect("request should serialize");
        assert_eq!(raw, r#"{"action":"control","command":"next"}"#);
        let decoded: PluginRequest = serde_json::from_str(&raw).expect("request should parse");
        assert!(matches!(
            decoded,
            PluginRequest::Control {
                command: MediaCommand::Next
            }
        ));
    }

    #[test]
    fn protocol_version_is_stable() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
