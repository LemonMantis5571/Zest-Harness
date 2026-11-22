//! The profile screen: activity across this computer's projects, a 27-week heatmap, tokens by
//! provider, and the user's name and photo.

use super::*;
use crate::theme::{self, Icon, alpha, icon};
use crate::ui;
use std::collections::HashMap;
use zest_core::profile::{ChatFacts, DayPoint, ProfileStats};

/// Weeks the heatmap shows, ending with this week.
const WEEKS: i64 = 27;
/// Avatar limits: a small JPEG keeps `avatar.jpg` cheap to load at startup.
const AVATAR_MAX_BYTES: usize = 48_000;
const AVATAR_EDGE: u32 = 128;

#[derive(Default)]
pub(super) struct ProfileState {
    pub data: Option<Result<ProfileData, String>>,
    pub loading: bool,
    pub tokens_mode: bool,
    pub name: String,
    pub avatar: Option<Arc<Image>>,
}

#[derive(Clone, Debug)]
pub(super) struct ProfileData {
    pub stats: ProfileStats,
    pub providers: Vec<(String, u64)>,
    pub workers: Vec<(String, u64, u64)>,
    pub skills: usize,
}

fn zest_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("zest"))
}

/// The saved display name and photo (`user-profile.json`, `avatar.jpg`).
pub(super) fn load_user() -> (String, Option<Arc<Image>>) {
    let Some(dir) = zest_dir() else { return (String::new(), None) };
    let name = std::fs::read_to_string(dir.join("user-profile.json"))
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| v.get("displayName").and_then(|n| n.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let avatar = std::fs::read(dir.join("avatar.jpg"))
        .ok()
        .filter(|bytes| bytes.starts_with(&[0xFF, 0xD8, 0xFF]))
        .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes)));
    (name, avatar)
}

pub(super) fn save_name(name: &str) -> Result<(), String> {
    let dir = zest_dir().ok_or("This system has no configuration folder.")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&serde_json::json!({ "displayName": name.trim() }))
        .map_err(|e| e.to_string())?;
    zest_core::atomic_write(&dir.join("user-profile.json"), body.as_bytes()).map_err(|e| e.to_string())
}

/// Resizes a picked image to a 128 px JPEG under 48,000 bytes before saving.
pub(super) fn avatar_jpeg(source: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(source).map_err(|_| "That file is not an image Zest can read.".to_string())?;
    let image = image.thumbnail(AVATAR_EDGE, AVATAR_EDGE).to_rgb8();
    for quality in [82u8, 70, 55, 40] {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&image)
            .map_err(|e| e.to_string())?;
        if out.len() <= AVATAR_MAX_BYTES {
            return Ok(out);
        }
    }
    Err("The photo is still too large after resizing.".into())
}

pub(super) fn save_avatar(jpeg: Option<&[u8]>) -> Result<(), String> {
    let path = zest_dir().ok_or("This system has no configuration folder.")?.join("avatar.jpg");
    match jpeg {
        Some(bytes) => {
            if bytes.is_empty() || bytes.len() > AVATAR_MAX_BYTES || !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
                return Err("The photo must be a JPEG of at most 48,000 bytes.".into());
            }
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            zest_core::atomic_write(&path, bytes).map_err(|e| e.to_string())
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}

/// Projects to count: the known workspaces plus the open one.
fn profile_roots(current: &std::path::Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = zest_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("known-workspaces.json")).ok())
        .and_then(|body| serde_json::from_str::<Vec<String>>(&body).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if !roots.iter().any(|r| r == current) {
        roots.push(current.to_path_buf());
    }
    roots
}

/// Gathers the profile; runs on a background thread.
pub(super) fn collect(current: &std::path::Path) -> Result<ProfileData, String> {
    let mut chats = Vec::new();
    for root in profile_roots(current).into_iter().filter(|r| r.is_dir()) {
        let Ok(store) = zest_core::ThreadStore::open(&root) else { continue };
        for summary in store.list().unwrap_or_default() {
            chats.push(ChatFacts {
                created_at: summary.created_at,
                updated_at: summary.updated_at,
                message_count: summary.message_count,
            });
        }
    }
    let ledger = zest_core::usage::Ledger::load();
    let (tokens, requests) = ledger.lifetime();
    let today = zest_core::usage::local_day_number(chrome::now_secs());
    let stats = zest_core::profile::derive(&chats, ledger.daily(), tokens, requests, today);
    let snapshot = ledger.snapshot();
    let providers = snapshot
        .providers
        .iter()
        .map(|p| (p.provider_id.clone(), p.measured.total_tokens))
        .collect();
    let workers = snapshot
        .external_workers
        .iter()
        .map(|w| (w.worker_id.clone(), w.invocations, w.token_reports))
        .collect();
    let skills = zest_core::SkillSet::discover().summaries().len();
    Ok(ProfileData { stats, providers, workers, skills })
}

/// Heatmap tone, as `lib/heatmap.ts`: 0 no record, 1 recorded zero, 2-5 quartiles of the peak.
pub(super) fn tone(value: Option<u64>, peak: u64) -> u8 {
    match value {
        None => 0,
        Some(0) => 1,
        Some(v) => {
            let share = v as f64 / peak.max(1) as f64;
            if share > 0.75 {
                5
            } else if share > 0.5 {
                4
            } else if share > 0.25 {
                3
            } else {
                2
            }
        }
    }
}

/// Day numbers for the grid, column per week with Sunday on top, ending this week's Saturday.
pub(super) fn grid_days(today: i64) -> Vec<i64> {
    // Day 0 (1970-01-01) was a Thursday.
    let weekday = (today + 4).rem_euclid(7);
    let end = today + (6 - weekday);
    let start = end - WEEKS * 7 + 1;
    (start..=end).collect()
}

fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 { format!("{h} h {m} min") } else { format!("{m} min") }
}

impl Workspace {
    pub(super) fn load_profile(&mut self, cx: &mut Context<Self>) {
        let (name, avatar) = load_user();
        self.profile.name = name.clone();
        self.profile.avatar = avatar;
        self.profile.loading = true;
        self.filter.update(cx, |field, cx| {
            field.text = name.clone();
            field.sel = name.len()..name.len();
            field.placeholder = "Your name";
            cx.notify();
        });
        let path = self.project.path.clone();
        let task = cx.background_spawn(async move { collect(&path) });
        cx.spawn(async move |this, cx| {
            let data = task.await;
            let _ = this.update(cx, |ws, cx| {
                ws.profile.data = Some(data);
                ws.profile.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn save_profile_name(&mut self, name: &str, cx: &mut Context<Self>) {
        match save_name(name) {
            Ok(()) => self.profile.name = name.trim().to_owned(),
            Err(e) => self.toast(ToastKind::Error, e, cx),
        }
        cx.notify();
    }

    fn pick_avatar(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose photo".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let result = cx
                .background_spawn(async move {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    let jpeg = avatar_jpeg(&bytes)?;
                    save_avatar(Some(&jpeg))?;
                    Ok::<_, String>(jpeg)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                match result {
                    Ok(jpeg) => ws.profile.avatar = Some(Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg))),
                    Err(e) => ws.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn profile_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.chat(cx).options.lock().unwrap().provider.clone();
        let shown_name = if self.profile.name.is_empty() { "Zest".to_string() } else { self.profile.name.clone() };
        let initial = self.profile.name.chars().next().map(|c| c.to_uppercase().to_string());
        let avatar = div()
            .size(px(64.))
            .flex_none()
            .rounded_full()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(theme::card()))
            .border_1()
            .border_color(rgb(theme::border()))
            .text_size(px(24.))
            .font_weight(FontWeight::SEMIBOLD)
            .map(|d| match (&self.profile.avatar, initial) {
                (Some(image), _) => d.child(img(image.clone()).size_full().object_fit(ObjectFit::Cover)),
                (None, Some(letter)) => d.child(letter),
                (None, None) => d.child(icon(Icon::User, 28.)),
            });
        let header = div()
            .flex()
            .items_center()
            .gap_4()
            .child(avatar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(shown_name))
                    .child(ui::hint(provider))
                    .child(div().max_w(px(320.)).child(self.filter.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(ui::button("choose-avatar", ui::Variant::Outline, Some(Icon::Image), "Choose photo", cx.listener(|ws, _, _, cx| ws.pick_avatar(cx))))
                            .when(self.profile.avatar.is_some(), |d| {
                                d.child(ui::button("remove-avatar", ui::Variant::Ghost, Some(Icon::Trash2), "Remove photo", cx.listener(|ws, _, _, cx| {
                                    match save_avatar(None) {
                                        Ok(()) => ws.profile.avatar = None,
                                        Err(e) => ws.toast(ToastKind::Error, e, cx),
                                    }
                                    cx.notify();
                                })))
                            }),
                    ),
            );
        let body = match &self.profile.data {
            None => ui::hint("Loading activity…").into_any_element(),
            Some(Err(_)) => ui::hint("Could not load profile activity. Try again.").into_any_element(),
            Some(Ok(data)) => self.profile_stats(data, cx),
        };
        div().flex().flex_col().gap_6().child(header).child(body).into_any_element()
    }

    fn profile_stats(&self, data: &ProfileData, cx: &mut Context<Self>) -> AnyElement {
        let s = &data.stats;
        let tile = |label: &'static str, value: String| {
            ui::card()
                .rounded(px(8.))
                .w(px(128.))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(ui::hint(label))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(value))
        };
        let tiles = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(tile("Zest tokens", compact(s.total_tokens)))
            .child(tile("Requests", compact(s.total_requests)))
            .child(tile("Busiest day", compact(s.peak_day_tokens)))
            .child(tile("Longest chat", duration(s.longest_chat_secs)))
            .child(tile("Current streak", format!("{}d", s.current_streak_days)))
            .child(tile("Longest streak", format!("{}d", s.longest_streak_days)));

        let tokens_mode = self.profile.tokens_mode && s.peak_day_tokens > 0;
        let by_date: HashMap<&str, &DayPoint> = s.days.iter().map(|d| (d.date.as_str(), d)).collect();
        let days = grid_days(zest_core::usage::local_day_number(chrome::now_secs()));
        let value = |day: Option<&&DayPoint>| -> Option<u64> {
            let day = day?;
            if tokens_mode { day.tokens } else { Some(day.chats as u64) }
        };
        let cells: Vec<(String, Option<u64>)> = days
            .iter()
            .map(|d| {
                let key = zest_core::usage::day_key_from_number(*d);
                let v = value(by_date.get(key.as_str()));
                (key, v)
            })
            .collect();
        let peak = cells.iter().filter_map(|(_, v)| *v).max().unwrap_or(0);
        let shade = |t: u8| match t {
            0 => alpha(theme::foreground(), 0.04),
            1 => alpha(theme::primary(), 0.12),
            2 => alpha(theme::primary(), 0.35),
            3 => alpha(theme::primary(), 0.55),
            4 => alpha(theme::primary(), 0.75),
            _ => rgb(theme::primary()).into(),
        };
        let heatmap = div().flex().gap(px(3.)).children(cells.chunks(7).map(|week| {
            div().flex().flex_col().gap(px(3.)).children(week.iter().map(|(_, v)| {
                div().size(px(11.)).rounded(px(2.)).bg(shade(tone(*v, peak)))
            }))
        }));
        let toggle = div()
            .flex()
            .gap_1()
            .child(
                ui::button("heatmap-chats", if tokens_mode { ui::Variant::Ghost } else { ui::Variant::Secondary }, None, "Chats", cx.listener(|ws, _, _, cx| {
                    ws.profile.tokens_mode = false;
                    cx.notify();
                })),
            )
            .child(
                ui::button("heatmap-tokens", if tokens_mode { ui::Variant::Secondary } else { ui::Variant::Ghost }, None, if s.peak_day_tokens == 0 { "No metered days yet" } else { "Tokens" }, cx.listener(|ws, _, _, cx| {
                    ws.profile.tokens_mode = true;
                    cx.notify();
                }))
                .when(s.peak_day_tokens == 0, |d| d.opacity(0.5)),
            );
        let first_used = s
            .first_activity
            .map(|t| zest_core::usage::day_key_from_number(zest_core::usage::local_day_number(t)))
            .unwrap_or_else(|| "—".into());
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .py_1()
                .text_size(px(13.))
                .child(div().text_color(rgb(theme::muted_foreground())).child(label))
                .child(value)
        };
        let activity = ui::card()
            .rounded(px(8.))
            .p_3()
            .flex_1()
            .min_w(px(260.))
            .flex()
            .flex_col()
            .child(ui::section_title("Activity"))
            .child(row("Total chats", s.total_chats.to_string()))
            .child(row("Messages", s.total_messages.to_string()))
            .child(row("Requests", s.total_requests.to_string()))
            .child(row("Skills available", data.skills.to_string()))
            .child(row("First used", first_used));
        let providers = ui::card()
            .rounded(px(8.))
            .p_3()
            .flex_1()
            .min_w(px(260.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::section_title("Zest tokens by provider"))
                    .child(ui::button("usage-report", ui::Variant::Ghost, None, "Full report", cx.listener(|ws, _, w, cx| ws.page(Panel::Usage, w, cx)))),
            )
            .children(data.providers.iter().map(|(label, tokens)| row_owned(label.clone(), compact(*tokens))))
            .when(data.providers.is_empty(), |d| d.child(ui::hint("No metered requests yet.")))
            .when(!data.workers.is_empty(), |d| {
                d.child(div().mt_2().child(ui::hint("External workers"))).children(data.workers.iter().map(|(id, runs, reports)| {
                    row_owned(
                        id.clone(),
                        format!("{runs} runs · {}", if *reports > 0 { format!("{reports} reported") } else { "Not reported".into() }),
                    )
                }))
            });
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(tiles)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().flex().items_center().justify_between().child(ui::section_title("Activity")).child(toggle))
                    .child(heatmap)
                    .when(tokens_mode, |d| {
                        d.child(ui::hint(format!(
                            "Token metering began {}.",
                            s.metering_since.clone().unwrap_or_else(|| "recently".into())
                        )))
                    }),
            )
            .child(div().flex().flex_wrap().gap_3().child(activity).child(providers))
            .into_any_element()
    }
}

fn row_owned(label: String, value: String) -> Div {
    div()
        .flex()
        .justify_between()
        .py_1()
        .text_size(px(13.))
        .child(div().text_color(rgb(theme::muted_foreground())).child(label))
        .child(value)
}

#[cfg(test)]
mod tests {
    use super::{avatar_jpeg, grid_days, tone};

    #[::core::prelude::v1::test]
    fn heatmap_tones_follow_quartiles() {
        assert_eq!(tone(None, 10), 0);
        assert_eq!(tone(Some(0), 10), 1);
        assert_eq!((tone(Some(2), 10), tone(Some(3), 10), tone(Some(6), 10), tone(Some(10), 10)), (2, 3, 4, 5));
    }

    #[::core::prelude::v1::test]
    fn grid_is_27_weeks_ending_on_saturday() {
        // 2026-11-21 is day 20_778 and a Saturday.
        let days = grid_days(20_778);
        assert_eq!(days.len(), 27 * 7);
        assert_eq!(*days.last().unwrap(), 20_778);
        assert_eq!((days[0] + 4).rem_euclid(7), 0, "columns start on Sunday");
    }

    #[::core::prelude::v1::test]
    fn avatars_become_small_jpegs() {
        let big = image::RgbImage::from_fn(900, 600, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 128]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let jpeg = avatar_jpeg(&png).unwrap();
        assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) && jpeg.len() <= 48_000);
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert!(decoded.width() <= 128 && decoded.height() <= 128);
        assert!(avatar_jpeg(b"not an image").is_err());
    }
}
