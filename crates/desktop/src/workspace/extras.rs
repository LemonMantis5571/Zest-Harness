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
                    Err(error) => ws.toast(ToastKind::Error, error, cx),
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
        use crate::theme::{self, Icon, alpha};
        use crate::ui::{self, Variant};

        let mut body = div().flex().flex_col().gap_4();
        if self.project.demo {
            return body.child(ui::empty_state(
                Icon::Puzzle,
                "Add-ons unavailable in offline demo",
                "Open a project window to turn on optional local integrations like Now Playing or Wallpaper.",
            ));
        }
        body = body.child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_3()
                .child(ui::section_heading(
                    "Extras",
                    "Optional local integrations. They stay on this PC, and each one is off until you turn it on.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .child(ui::button(
                            "open-plugins",
                            Variant::Secondary,
                            Some(Icon::FolderOpen),
                            "Open folder",
                            cx.listener(|ws, _, _, cx| match plugins::ensure_plugin_folder() {
                                Ok(path) => cx.open_with_system(&path),
                                Err(e) => {
                                    ws.toast(ToastKind::Error, e, cx);
                                    cx.notify();
                                }
                            }),
                        ))
                        .child(ui::button(
                            "refresh-extras",
                            Variant::Outline,
                            Some(if self.extras.busy { Icon::LoaderCircle } else { Icon::RefreshCw }),
                            if self.extras.busy { "Refreshing…" } else { "Refresh" },
                            cx.listener(|ws, _, _, cx| ws.load_extras(cx)),
                        )),
                ),
        );
        if self.extras.busy && self.extras.rows.is_empty() {
            return body.child(ui::loading_row("addons-loading", "Loading add-ons…"));
        }
        if self.extras.rows.is_empty() {
            return body.child(ui::empty_state(
                Icon::Puzzle,
                "No add-ons installed",
                "Open the add-ons folder to place local Zest plugins, then click Refresh.",
            ));
        }

        let mut list = div().flex().flex_col().gap_3();
        for (i, row) in self.extras.rows.iter().enumerate() {
            let id = row.id.clone();
            let enable = !row.enabled;
            let action = if !row.available {
                ui::pill(None, "Not ready").into_any_element()
            } else {
                ui::button(
                    ("plugin-toggle", i),
                    if row.enabled { Variant::Outline } else { Variant::Primary },
                    None,
                    if enable { "Turn on" } else { "Turn off" },
                    cx.listener(move |ws, _, _, cx| {
                        ws.apply_extra(Action::Enable(id.clone(), enable), cx);
                    }),
                )
                .into_any_element()
            };

            let mut card = ui::card()
                .rounded(px(10.))
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(theme::foreground()))
                                                .child(row.name.clone()),
                                        )
                                        .when(row.enabled, |d| d.child(ui::pill(None, "On"))),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .line_height(px(17.))
                                        .text_color(rgb(theme::muted_foreground()))
                                        .child(row.description.clone()),
                                )
                                .when(row.detail != "Ready" && !row.detail.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .mt(px(2.))
                                            .text_size(px(11.))
                                            .text_color(rgb(theme::muted_foreground()))
                                            .child(row.detail.clone()),
                                    )
                                }),
                        )
                        .child(action),
                );

            if row.id == "now-playing" && row.enabled
                && let Some(media) = &self.extras.media
            {
                let summary = [
                    media.title.as_deref(),
                    media.artist.as_deref(),
                    Some(media.status.as_str()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let mut controls = div().flex().flex_wrap().items_center().gap_2();
                for (action_name, label, supported) in [
                    ("previous", "Previous", media.can_previous),
                    ("toggle", "Play / pause", media.can_toggle),
                    ("next", "Next", media.can_next),
                ] {
                    if supported != Some(false) {
                        controls = controls.child(ui::button(
                            action_name,
                            Variant::Outline,
                            None,
                            label,
                            cx.listener(move |ws, _, _, cx| ws.apply_extra(Action::Play(action_name), cx)),
                        ));
                    }
                }
                if let Some(volume) = media.volume_percent {
                    for (label, step) in [("Quieter", -10.), ("Louder", 10.)] {
                        controls = controls.child(ui::button(
                            label,
                            Variant::Ghost,
                            None,
                            label,
                            cx.listener(move |ws, _, _, cx| {
                                ws.apply_extra(Action::Volume(volume + step), cx)
                            }),
                        ));
                    }
                    controls = controls.child(ui::pill(None, format!("Volume {:.0}%", volume)));
                }
                card = card.child(
                    div()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(alpha(theme::border(), 0.7))
                        .bg(alpha(theme::background(), 0.5))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(theme::icon(Icon::Music, 14.).text_color(rgb(theme::primary())))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(if summary.is_empty() { media.detail.clone() } else { summary }),
                                ),
                        )
                        .when(!media.detail.is_empty(), |d| d.child(ui::hint(media.detail.clone())))
                        .child(controls),
                );
            }

            if row.id == "wallpaper" && row.enabled
                && let Some(wall) = &self.extras.wallpaper
            {
                let mut wall_box = div()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(alpha(theme::border(), 0.7))
                    .bg(alpha(theme::background(), 0.5))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme::muted_foreground()))
                                    .child(
                                        wall.source_name
                                            .clone()
                                            .unwrap_or_else(|| wall.detail.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(ui::button(
                                        "choose-wallpaper",
                                        Variant::Outline,
                                        Some(Icon::FolderOpen),
                                        "Choose image",
                                        cx.listener(|ws, _, _, cx| ws.choose_wallpaper(cx)),
                                    ))
                                    .when(self.extras.image.is_some(), |d| {
                                        d.child(ui::button(
                                            "clear-wallpaper",
                                            Variant::Ghost,
                                            Some(Icon::Trash2),
                                            "Clear",
                                            cx.listener(|ws, _, _, cx| ws.apply_extra(Action::Clear, cx)),
                                        ))
                                    }),
                            ),
                    );
                if self.extras.image.is_some() {
                    let mut looks = div().flex().flex_wrap().gap_2();
                    for &look in LOOKS {
                        looks = looks.child(ui::select_chip(
                            look,
                            look,
                            wall.filter == look,
                            cx.listener(move |ws, _, _, cx| ws.apply_extra(Action::Look(look), cx)),
                        ));
                    }
                    wall_box = wall_box.child(looks);
                }
                card = card.child(wall_box);
            }

            list = list.child(card);
        }

        body.child(list)
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
