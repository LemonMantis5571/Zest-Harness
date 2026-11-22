use super::*;
use crate::plugins;
use base64::Engine as _;
use zest_plugin_api::{NOW_PLAYING_ID, WALLPAPER_ID};

const LOOKS: &[&str] = &[
    "none", "print", "frosted", "noir", "sepia", "warm", "cool", "muted",
];

#[derive(Default)]
pub(super) struct Extras {
    rows: Vec<plugins::PluginView>,
    media: Option<plugins::NowPlayingView>,
    wallpaper: Option<plugins::WallpaperView>,
    pub(super) image: Option<Arc<Image>>,
    busy: bool,
    poll: Option<Task<()>>,
}

enum Action {
    Refresh,
    Enable(String, bool),
    Play(&'static str),
    Volume(f64),
    Choose(PathBuf),
    Look(&'static str),
    Clear,
}

enum Update {
    List(
        Vec<plugins::PluginView>,
        Option<plugins::NowPlayingView>,
        Option<plugins::WallpaperView>,
    ),
    Media(plugins::NowPlayingView),
    Wallpaper(plugins::WallpaperView),
}

impl Action {
    fn run(self) -> Result<Update, String> {
        match self {
            Self::Refresh | Self::Enable(..) => {
                let rows = if let Self::Enable(id, enabled) = self {
                    plugins::set_enabled(&id, enabled)?
                } else {
                    plugins::list()
                };
                let enabled = |id| rows.iter().any(|r| r.id == id && r.enabled);
                let media = enabled(NOW_PLAYING_ID).then(plugins::now_playing);
                let wallpaper = enabled(WALLPAPER_ID).then(plugins::wallpaper);
                Ok(Update::List(rows, media, wallpaper))
            }
            Self::Play(action) => plugins::control(action).map(Update::Media),
            Self::Volume(value) => plugins::set_volume(value).map(Update::Media),
            Self::Choose(path) => plugins::set_wallpaper(path).map(Update::Wallpaper),
            Self::Look(look) => plugins::set_wallpaper_filter(look).map(Update::Wallpaper),
            Self::Clear => plugins::clear_wallpaper().map(Update::Wallpaper),
        }
    }
}

fn image_from_url(url: &str) -> Option<Arc<Image>> {
    let (prefix, data) = url.split_once(',')?;
    let format = match prefix {
        "data:image/png;base64" => ImageFormat::Png,
        "data:image/jpeg;base64" => ImageFormat::Jpeg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

impl Extras {
    fn set_wallpaper(&mut self, mut wallpaper: Option<plugins::WallpaperView>) {
        self.image = wallpaper
            .as_mut()
            .and_then(|w| w.image_data_url.take())
            .and_then(|url| image_from_url(&url));
        self.wallpaper = wallpaper;
    }

    pub(super) fn track_label(&self) -> Option<String> {
        self.media
            .as_ref()
            .map(|m| m.title.clone().unwrap_or_else(|| "Now Playing".into()))
    }
}

impl Workspace {
    pub(super) fn load_extras(&mut self, cx: &mut Context<Self>) {
        self.apply_extra(Action::Refresh, cx);
    }

    fn apply_extra(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.project.demo || self.extras.busy {
            return;
        }
        self.extras.busy = true;
        self.extras.poll = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { action.run() })
                .await;
            let _ = this.update(cx, |ws, cx| {
                ws.extras.busy = false;
                match result {
                    Ok(Update::List(rows, media, wallpaper)) => {
                        ws.extras.rows = rows;
                        ws.extras.media = media;
                        ws.extras.set_wallpaper(wallpaper);
                    }
                    Ok(Update::Media(media)) => ws.extras.media = Some(media),
                    Ok(Update::Wallpaper(wallpaper)) => ws.extras.set_wallpaper(Some(wallpaper)),
                    Err(error) => ws.note = error,
                }
                if ws.extras.media.is_some() {
                    ws.poll_music(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn poll_music(&mut self, cx: &mut Context<Self>) {
        self.extras.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                let media = cx
                    .background_executor()
                    .spawn(async { plugins::now_playing() })
                    .await;
                let keep = this
                    .update(cx, |ws, cx| {
                        if media.status == "disabled" {
                            ws.extras.media = None;
                        } else {
                            ws.extras.media = Some(media);
                        }
                        cx.notify();
                        ws.extras.media.is_some()
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn choose_wallpaper(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose wallpaper image".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |ws, cx| ws.apply_extra(Action::Choose(path), cx));
            }
        })
        .detach();
    }

    pub(super) fn extras_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex().flex_col().gap_2();
        if self.project.demo {
            return body.child("Local add-ons are available in project windows.");
        }
        body = body
            .child(button(
                "refresh-extras",
                "Refresh add-ons",
                cx.listener(|ws, _, _, cx| ws.load_extras(cx)),
            ))
            .child(button(
                "open-plugins",
                "Open add-ons folder",
                cx.listener(|ws, _, _, cx| match plugins::ensure_plugin_folder() {
                    Ok(path) => cx.open_with_system(&path),
                    Err(e) => {
                        ws.note = e;
                        cx.notify();
                    }
                }),
            ));
        if self.extras.busy {
            body = body.child(crate::ui::hint("Loading add-ons…"));
        } else if self.extras.rows.is_empty() {
            body = body.child(crate::ui::hint("No add-ons installed."));
        }
        for (i, row) in self.extras.rows.iter().enumerate() {
            body = body.child(row.name.clone()).child(row.description.clone());
            if !row.available {
                body = body.child(row.detail.clone());
                continue;
            }
            let id = row.id.clone();
            let enable = !row.enabled;
            body = body.child(button(
                ("plugin-toggle", i),
                if enable { "Turn on" } else { "Turn off" },
                cx.listener(move |ws, _, _, cx| {
                    ws.apply_extra(Action::Enable(id.clone(), enable), cx);
                }),
            ));
        }
        if let Some(media) = &self.extras.media {
            body = body
                .child(
                    [
                        media.title.as_deref(),
                        media.artist.as_deref(),
                        Some(media.status.as_str()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                )
                .child(media.detail.clone());
            let mut controls = div().flex().gap_1().flex_wrap();
            for (action, label, supported) in [
                ("previous", "Previous", media.can_previous),
                ("toggle", "Play / pause", media.can_toggle),
                ("next", "Next", media.can_next),
            ] {
                if supported != Some(false) {
                    controls = controls.child(button(
                        action,
                        label,
                        cx.listener(move |ws, _, _, cx| ws.apply_extra(Action::Play(action), cx)),
                    ));
                }
            }
            if let Some(volume) = media.volume_percent {
                for (label, step) in [("Quieter", -10.), ("Louder", 10.)] {
                    controls = controls.child(button(
                        label,
                        label,
                        cx.listener(move |ws, _, _, cx| {
                            ws.apply_extra(Action::Volume(volume + step), cx)
                        }),
                    ));
                }
                controls = controls.child(format!("Volume {:.0}%", volume));
            }
            body = body.child(controls);
        }
        if let Some(wall) = &self.extras.wallpaper {
            body = body
                .child(
                    wall.source_name
                        .clone()
                        .unwrap_or_else(|| wall.detail.clone()),
                )
                .child(button(
                    "choose-wallpaper",
                    "Choose image",
                    cx.listener(|ws, _, _, cx| ws.choose_wallpaper(cx)),
                ));
            if self.extras.image.is_some() {
                let mut looks = div().flex().flex_wrap().gap_1();
                for &look in LOOKS {
                    looks = looks.child(button(
                        look,
                        mark(look, wall.filter == look),
                        cx.listener(move |ws, _, _, cx| ws.apply_extra(Action::Look(look), cx)),
                    ));
                }
                body = body.child(looks).child(button(
                    "clear-wallpaper",
                    "Clear wallpaper",
                    cx.listener(|ws, _, _, cx| ws.apply_extra(Action::Clear, cx)),
                ));
            }
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::image_from_url;
    #[test]
    fn wallpaper_accepts_only_the_host_image_formats() {
        assert!(image_from_url("data:image/png;base64,AA==").is_some());
        assert!(image_from_url("data:image/jpeg;base64,AA==").is_some());
        assert!(image_from_url("data:image/png;base64,%").is_none());
        assert!(image_from_url("data:text/html;base64,AA==").is_none());
    }
}
