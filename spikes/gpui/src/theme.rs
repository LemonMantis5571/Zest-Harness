//! The native app's look, taken from the React UI's default "zest" theme
//! (`crates/desktop/ui/src/index.css`) so both frontends share one design.

use gpui::prelude::*;
use gpui::*;
use std::borrow::Cow;

pub use crate::icons_generated::Icon;

// Surfaces.
pub const BACKGROUND: u32 = 0x0c0c0e;
pub const CANVAS: u32 = 0x0f1012;
pub const SIDEBAR: u32 = 0x121314;
pub const HEADER: u32 = 0x121314;
pub const CARD: u32 = 0x141516;
pub const POPOVER: u32 = 0x16171a;
pub const SECONDARY: u32 = 0x1a1b1d;
pub const ACCENT: u32 = 0x1c1d20;
pub const USER_BUBBLE: u32 = 0x1c1e24;
pub const BORDER: u32 = 0x2a2c31;

// Text.
pub const FOREGROUND: u32 = 0xf7f8f8;
pub const SECONDARY_FOREGROUND: u32 = 0xd0d6e0;
pub const MUTED_FOREGROUND: u32 = 0x8a8f98;

// Accents (Tailwind 4 palette values where index.css refers to them).
pub const PRIMARY: u32 = 0x5e6ad2;
pub const PRIMARY_HOVER: u32 = 0x828fff;
pub const LINK: u32 = 0x82a0ff;
pub const DESTRUCTIVE: u32 = 0xe5484d;
pub const SUCCESS: u32 = 0x00bc7d;
pub const WARNING: u32 = 0xedb200;
pub const INFO: u32 = 0x8e51ff;

pub const RADIUS: f32 = 8.;
pub const SIDEBAR_WIDTH: f32 = 260.;
pub const CHAT_MAX: f32 = 720.;

pub const SANS: &str = "Geist";
pub const MONO: &str = "Consolas";

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
        .text_color(rgb(MUTED_FOREGROUND))
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

/// Registers Geist (SIL OFL 1.1, see `assets/fonts/Geist-OFL.txt`), the React UI's default face.
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
    gpui_platform::application().with_assets(Assets)
}

#[cfg(test)]
mod tests {
    use super::{Assets, Icon};
    use gpui::AssetSource;

    #[::core::prelude::v1::test]
    fn every_icon_is_bundled_as_svg() {
        for icon in [Icon::SquarePen, Icon::GitPullRequest, Icon::X] {
            let bytes = Assets.load(icon.path()).unwrap().expect("icon is bundled");
            assert!(bytes.starts_with(b"<svg"));
        }
        assert_eq!(Assets.list("icons/").unwrap().len(), crate::icons_generated::FILES.len());
    }
}
