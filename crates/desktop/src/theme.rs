//! The app's look: theme palettes, fonts, icons and saved appearance preferences.
//! Colors are read from the active palette at render time, so switching theme repaints the app.

use gpui::prelude::*;
use gpui::*;
use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use crate::icons_generated::Icon;

/// One theme's color tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub dark: bool,
    pub background: u32,
    pub canvas: u32,
    pub sidebar: u32,
    pub header: u32,
    pub card: u32,
    pub popover: u32,
    pub secondary: u32,
    pub accent: u32,
    pub user_bubble: u32,
    pub border: u32,
    pub foreground: u32,
    pub secondary_foreground: u32,
    pub muted_foreground: u32,
    pub primary: u32,
    pub primary_hover: u32,
    pub link: u32,
    pub destructive: u32,
}

pub const THEMES: [Palette; 3] = [
    Palette {
        id: "zest",
        name: "Zest",
        description: "The default dark canvas.",
        dark: true,
        background: 0x0c0c0e,
        canvas: 0x0f1012,
        sidebar: 0x121314,
        header: 0x121314,
        card: 0x141516,
        popover: 0x16171a,
        secondary: 0x1a1b1d,
        accent: 0x1c1d20,
        user_bubble: 0x1c1e24,
        border: 0x2a2c31,
        foreground: 0xf7f8f8,
        secondary_foreground: 0xd0d6e0,
        muted_foreground: 0x8a8f98,
        primary: 0x5e6ad2,
        primary_hover: 0x828fff,
        link: 0x82a0ff,
        destructive: 0xe5484d,
    },
    Palette {
        id: "nights",
        name: "Nights",
        description: "Midnight purple, magenta, and gold.",
        dark: true,
        background: 0x12081c,
        canvas: 0x1b102c,
        sidebar: 0x160c28,
        header: 0x160c28,
        card: 0x1b102c,
        popover: 0x221436,
        secondary: 0x2a1844,
        accent: 0x2e1a4c,
        user_bubble: 0x2e1648,
        border: 0x4a3270,
        foreground: 0xf3eef8,
        secondary_foreground: 0xe8dcf4,
        muted_foreground: 0xc0a8d4,
        primary: 0xe4458c,
        primary_hover: 0xf05aa0,
        link: 0x7ec8f8,
        destructive: 0xe85a7a,
    },
    Palette {
        id: "oceanic",
        name: "Oceanic",
        description: "Clear-sky blues inspired by Ocean, Java's Metal look.",
        dark: false,
        background: 0xeaf6ff,
        canvas: 0xf7fcff,
        sidebar: 0xeef8ff,
        header: 0xeef8ff,
        card: 0xf7fcff,
        popover: 0xfbfdff,
        secondary: 0xdbeeff,
        accent: 0xdbeeff,
        user_bubble: 0xdbeeff,
        border: 0xc9d3dd,
        foreground: 0x0d3151,
        secondary_foreground: 0x0d3151,
        muted_foreground: 0x4c6b89,
        primary: 0x1367c7,
        primary_hover: 0x1e85f0,
        link: 0x1367c7,
        destructive: 0xc22a49,
    },
];

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// The palette the app is drawing with.
pub fn palette() -> &'static Palette {
    &THEMES[ACTIVE.load(Ordering::Relaxed).min(THEMES.len() - 1)]
}

/// Switches to the theme with `id`; an unknown id falls back to Zest.
pub fn set_theme(id: &str) -> &'static Palette {
    let index = THEMES.iter().position(|t| t.id == id).unwrap_or(0);
    ACTIVE.store(index, Ordering::Relaxed);
    &THEMES[index]
}

/// `a` mixed with `b`, `amount` of the way toward `a` (CSS `color-mix(in srgb, a amount, b)`).
pub fn mix(a: u32, b: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x * amount + y * (1. - amount)).round() as u32).min(255) << shift
    };
    channel(16) | channel(8) | channel(0)
}

macro_rules! tokens {
    ($($name:ident),* $(,)?) => {
        $(#[allow(dead_code)] pub fn $name() -> u32 { palette().$name })*
    };
}
tokens!(
    background, canvas, sidebar, header, card, popover, secondary, accent, user_bubble, border,
    foreground, secondary_foreground, muted_foreground, primary, primary_hover, link, destructive,
);

// Derived colors: token blends at fixed opacities over a surface.
/// `border-border/60` on the sidebar.
pub fn sidebar_line() -> u32 { mix(border(), sidebar(), 0.6) }
/// `border-border/40` on the sidebar.
pub fn section_line() -> u32 { mix(border(), sidebar(), 0.4) }
/// `border-border/60` on the canvas.
pub fn canvas_line() -> u32 { mix(border(), canvas(), 0.6) }
/// Selected chat row: `bg-sidebar-primary/18`.
pub fn selected_row() -> u32 { mix(primary(), sidebar(), 0.18) }
/// `text-muted-foreground/60` on the sidebar.
pub fn faint() -> u32 { mix(muted_foreground(), sidebar(), 0.6) }
/// The "Zest" label above replies: `text-muted-foreground/80` on the canvas.
pub fn label() -> u32 { mix(muted_foreground(), canvas(), 0.8) }
/// Input and tool-output fill: `bg-input/30` on the canvas.
pub fn input_bg() -> u32 { mix(border(), canvas(), 0.3) }
/// `hover:bg-input/50` on the canvas.
pub fn input_hover() -> u32 { mix(border(), canvas(), 0.5) }
/// Table and artifact outline: `border-border/80` on the canvas.
pub fn soft_border() -> u32 { mix(border(), canvas(), 0.8) }
/// Table body: `bg-card/30` on the canvas.
pub fn table_bg() -> u32 { mix(card(), canvas(), 0.3) }
/// Table header: `bg-secondary/80` on the canvas.
pub fn table_head() -> u32 { mix(secondary(), canvas(), 0.8) }
/// Code language chip: `bg-muted/60` on a card.
pub fn chip_bg() -> u32 { mix(secondary(), card(), 0.6) }
/// Model picker text: `text-foreground/85` on a card.
pub fn model_text() -> u32 { mix(foreground(), card(), 0.85) }
/// Text selection: the primary accent dimmed onto the canvas.
pub fn selection() -> u32 { mix(primary(), canvas(), 0.35) }

// Shared by every theme.
pub const WARNING: u32 = 0xedb200;
/// Text on primary buttons.
pub const ON_PRIMARY: u32 = 0xffffff;

pub const SIDEBAR_WIDTH: f32 = 260.;
pub const CHAT_MAX: f32 = 720.;

pub const MONO: &str = "Consolas";

/// A text face the Appearance tab offers. `family` is the name GPUI resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub id: &'static str,
    pub name: &'static str,
    pub family: &'static str,
    /// Bundled with the app, so always available.
    pub bundled: bool,
}

/// The interface fonts on offer. Geist ships with the app; the others are
/// offered when installed on this computer.
pub const FACES: [Face; 6] = [
    Face { id: "geist", name: "Geist", family: "Geist", bundled: true },
    Face { id: "inter", name: "Inter", family: "Inter", bundled: false },
    Face { id: "plus-jakarta", name: "Plus Jakarta Sans", family: "Plus Jakarta Sans", bundled: false },
    Face { id: "jetbrains-mono", name: "JetBrains Mono", family: "JetBrains Mono", bundled: false },
    Face { id: "fira-code", name: "Fira Code", family: "Fira Code", bundled: false },
    Face { id: "system", name: "System UI", family: SYSTEM_UI, bundled: true },
];

#[cfg(target_os = "windows")]
const SYSTEM_UI: &str = "Segoe UI";
#[cfg(target_os = "macos")]
const SYSTEM_UI: &str = ".SystemUIFont";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const SYSTEM_UI: &str = "sans-serif";

static ACTIVE_FACE: AtomicUsize = AtomicUsize::new(0);

pub fn face() -> &'static Face {
    &FACES[ACTIVE_FACE.load(Ordering::Relaxed).min(FACES.len() - 1)]
}

/// The interface font family.
pub fn sans() -> &'static str {
    face().family
}

/// Switches the interface font; an unknown id falls back to Geist.
pub fn set_face(id: &str) -> &'static Face {
    let index = FACES.iter().position(|f| f.id == id).unwrap_or(0);
    ACTIVE_FACE.store(index, Ordering::Relaxed);
    &FACES[index]
}

/// Faces usable on this computer: bundled ones plus installed families.
pub fn available_faces(cx: &App) -> Vec<&'static Face> {
    let installed = cx.text_system().all_font_names();
    FACES.iter().filter(|f| f.bundled || installed.iter().any(|n| n == f.family)).collect()
}

static BLOCK_STREAMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static COMPACT_SIDEBAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Customize > Chat view > "Stream complete blocks" (default on): a streaming reply shows only
/// its complete Markdown blocks.
pub fn block_streaming() -> bool {
    BLOCK_STREAMING.load(Ordering::Relaxed)
}

pub fn set_block_streaming(on: bool) {
    BLOCK_STREAMING.store(on, Ordering::Relaxed);
}

/// Customize > Chat view > Chat history: the compact one-line list instead of the project tree.
pub fn compact_sidebar() -> bool {
    COMPACT_SIDEBAR.load(Ordering::Relaxed)
}

pub fn set_compact_sidebar(on: bool) {
    COMPACT_SIDEBAR.store(on, Ordering::Relaxed);
}

fn yes() -> bool {
    true
}

/// The saved look and chat-view choices, per machine.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub font: String,
    /// "project" (default) or "compact", as `zest.chatViewMode`.
    #[serde(default)]
    pub chat_view: String,
    #[serde(default = "yes")]
    pub block_streaming: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { theme: String::new(), font: String::new(), chat_view: String::new(), block_streaming: true }
    }
}

fn appearance_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("zest").join("gpui-appearance.json"))
}

/// Applies the saved theme and font, falling back to the defaults when the file is missing or unreadable.
pub fn load_appearance() {
    let saved = appearance_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|body| serde_json::from_str::<Appearance>(&body).ok())
        .unwrap_or_default();
    set_theme(&saved.theme);
    set_face(&saved.font);
    set_compact_sidebar(saved.chat_view == "compact");
    set_block_streaming(saved.block_streaming);
}

/// Saves the active theme and font for the next launch.
pub fn save_appearance() -> Result<(), String> {
    let path = appearance_path().ok_or("This system has no configuration folder.")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let value = Appearance {
        theme: palette().id.into(),
        font: face().id.into(),
        chat_view: if compact_sidebar() { "compact" } else { "project" }.into(),
        block_streaming: block_streaming(),
    };
    zest_core::fsutil::atomic_write_json(&path, &value).map_err(|e| format!("Could not save appearance: {e}"))
}

/// `color` at `alpha` opacity, like Tailwind's `bg-foreground/5`.
pub fn alpha(color: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(color).into();
    c.a = alpha;
    c
}

/// A Lucide icon, muted unless recolored with `.text_color()`. SVGs do not inherit their
/// parent's text color.
pub fn icon(icon: Icon, size: f32) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size))
        .flex_none()
        .text_color(rgb(muted_foreground()))
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(crate::icons_generated::FILES
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(crate::icons_generated::FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// Registers Geist (SIL OFL 1.1, see `assets/fonts/Geist-OFL.txt`), the default face.
pub fn load_fonts(cx: &mut App) {
    let fonts = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-SemiBold.ttf").as_slice()),
    ];
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        eprintln!("Could not load the Geist font: {error}");
    }
}

/// Starts the app with Zest's assets and fonts.
pub fn application() -> Application {
    load_appearance();
    gpui_platform::application().with_assets(Assets)
}

#[cfg(test)]
mod tests {
    use super::{Appearance, Assets, Icon, THEMES, mix, set_face, set_theme};
    use gpui::AssetSource;

    #[::core::prelude::v1::test]
    fn zest_blends_match_their_reference_colors() {
        let zest = &THEMES[0];
        assert_eq!(mix(zest.border, zest.sidebar, 0.6), 0x202225);
        assert_eq!(mix(zest.border, zest.canvas, 0.6), 0x1f2125);
        assert_eq!(mix(zest.primary, zest.sidebar, 0.18), 0x202336);
        assert_eq!(mix(zest.muted_foreground, zest.canvas, 0.8), 0x71767d);
        assert_eq!(mix(0xffffff, 0x000000, 0.5), 0x808080);
    }

    #[::core::prelude::v1::test]
    fn unknown_theme_and_font_fall_back_to_the_defaults() {
        assert_eq!(set_theme("oceanic").id, "oceanic");
        assert_eq!(set_theme("no-such-theme").id, "zest");
        assert_eq!(set_face("fira-code").id, "fira-code");
        assert_eq!(set_face("").id, "geist");
        let saved: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(saved, Appearance::default());
        assert!(saved.block_streaming, "block streaming defaults on");
        let older: Appearance = serde_json::from_str(r#"{"theme":"nights","font":"inter"}"#).unwrap();
        assert!(older.block_streaming && older.chat_view.is_empty());
    }

    #[::core::prelude::v1::test]
    fn every_icon_is_bundled_as_svg() {
        for icon in [Icon::SquarePen, Icon::GitPullRequest, Icon::X] {
            let bytes = Assets.load(icon.path()).unwrap().expect("icon is bundled");
            assert!(bytes.starts_with(b"<svg"));
        }
        assert_eq!(Assets.list("icons/").unwrap().len(), crate::icons_generated::FILES.len());
    }
}
