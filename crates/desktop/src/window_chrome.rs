//! Keep the native window chrome on the selected colour theme.
//!
//! Cursor paints `titleBar.activeBackground`. We keep OS decorations and
//! retint them: `set_theme` / `set_background_color` everywhere, plus Windows
//! 11 caption, text, and border colours.

use tauri::{window::Color, Theme, WebviewWindow};

#[tauri::command]
pub fn set_window_chrome(
    window: WebviewWindow,
    background: String,
    appearance: String,
) -> Result<(), String> {
    apply_window_chrome(&window, &background, &appearance)
}

pub(crate) fn apply_window_chrome(
    window: &WebviewWindow,
    background: &str,
    appearance: &str,
) -> Result<(), String> {
    let (r, g, b) = parse_hex_rgb(background)?;
    let dark = appearance != "light";
    window
        .set_theme(Some(if dark { Theme::Dark } else { Theme::Light }))
        .map_err(|error| error.to_string())?;
    window
        .set_background_color(Some(Color(r, g, b, 255)))
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    apply_windows_caption(window, r, g, b, dark);
    Ok(())
}

pub(crate) fn parse_hex_rgb(value: &str) -> Result<(u8, u8, u8), String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return Err("expected #RRGGBB".into());
    }
    let n = u32::from_str_radix(hex, 16).map_err(|_| "expected #RRGGBB".to_string())?;
    Ok((
        ((n >> 16) & 0xff) as u8,
        ((n >> 8) & 0xff) as u8,
        (n & 0xff) as u8,
    ))
}

#[cfg(windows)]
fn apply_windows_caption(window: &WebviewWindow, r: u8, g: u8, b: u8, dark: bool) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let caption = colorref(r, g, b);
    let text = if dark {
        colorref(0xf4, 0xf4, 0xf5)
    } else {
        colorref(0x17, 0x17, 0x1a)
    };
    let immersive: i32 = i32::from(dark);
    let raw = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    unsafe {
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
        };

        dwm_set(raw, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &immersive);
        dwm_set(raw, DWMWA_CAPTION_COLOR as u32, &caption);
        dwm_set(raw, DWMWA_TEXT_COLOR as u32, &text);
        dwm_set(raw, DWMWA_BORDER_COLOR as u32, &caption);
    }
}

#[cfg(windows)]
fn colorref(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

#[cfg(windows)]
unsafe fn dwm_set<T>(hwnd: windows_sys::Win32::Foundation::HWND, attribute: u32, value: &T) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

    let _ = DwmSetWindowAttribute(
        hwnd,
        attribute,
        (value as *const T).cast(),
        std::mem::size_of::<T>() as u32,
    );
}

#[cfg(test)]
mod tests {
    use super::parse_hex_rgb;

    #[test]
    fn parse_hex_rgb_reads_six_digit_colours() {
        assert_eq!(parse_hex_rgb("#0c0c0e"), Ok((0x0c, 0x0c, 0x0e)));
        assert_eq!(parse_hex_rgb("eaf6ff"), Ok((0xea, 0xf6, 0xff)));
        assert_eq!(parse_hex_rgb("#12081c"), Ok((0x12, 0x08, 0x1c)));
    }

    #[test]
    fn parse_hex_rgb_rejects_junk() {
        assert!(parse_hex_rgb("#fff").is_err());
        assert!(parse_hex_rgb("not-a-colour").is_err());
        assert!(parse_hex_rgb("").is_err());
    }
}
