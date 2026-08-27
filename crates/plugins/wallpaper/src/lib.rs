//! Optional wallpaper add-on: copy a chosen image and give it an optional look.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{imageops, DynamicImage, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use zest_plugin_api::{wallpaper_filter, PluginRequest, WallpaperView};

const MAX_REQUEST_BYTES: usize = 32 * 1024;
const MAX_SOURCE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_EDGE: u32 = 1600;
/// Total pixels allowed out, on top of [`MAX_EDGE`]. Equivalent to 1600×900.
const MAX_PIXELS: u64 = 1_440_000;
const STATE_FILE: &str = "state.json";
const OUTPUT_PNG: &str = "wallpaper.png";
const OUTPUT_JPG: &str = "wallpaper.jpg";
/// Bump when any look changes so an already-rendered file is redone.
const FILTER_VERSION: u32 = 4;
/// Print-look steps per RGB channel. 4 keeps color instead of flattening to 1-bit gray.
const TONE_LEVELS: u32 = 4;
/// Frosted blur radius at the longest edge, as a fraction of that edge.
const FROST_RADIUS_RATIO: f32 = 0.012;

/// 8×8 Bayer matrix, values 0..=63.
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WallpaperState {
    source_name: String,
    source_file: String,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    filter_version: u32,
}

pub fn read_request() -> Result<PluginRequest, String> {
    let mut raw = String::new();
    io::stdin()
        .take(MAX_REQUEST_BYTES as u64)
        .read_to_string(&mut raw)
        .map_err(|_| "Could not read the request.".to_string())?;
    serde_json::from_str(raw.trim()).map_err(|_| "The request was not valid.".to_string())
}

pub fn handle(request: PluginRequest) -> Result<WallpaperView, String> {
    match request {
        PluginRequest::Get => load(),
        PluginRequest::SetWallpaper { image_path, filter } => set_image(&image_path, &filter),
        PluginRequest::SetWallpaperFilter { filter } => set_filter(&filter),
        PluginRequest::ClearWallpaper => clear(),
        PluginRequest::Control { .. } | PluginRequest::SetVolume { .. } => {
            Err("This add-on does not support that.".into())
        }
    }
}

fn load() -> Result<WallpaperView, String> {
    let Some(mut state) = read_state()? else {
        return Ok(empty_view("Choose an image."));
    };
    let source = PathBuf::from(&state.source_file);
    if !source.is_file() {
        return Ok(empty_view("Choose an image."));
    }
    let filter = wallpaper_filter(&state.filter);
    let output = output_name(filter);
    // A processed file from an older pass is not what the user would see now, so
    // the version gate re-renders it before the host reads it.
    let stale = filter != "none" && state.filter_version != FILTER_VERSION;
    if stale || !Path::new(output).is_file() {
        render(&source, filter)?;
        state.filter = filter.into();
        state.filter_version = FILTER_VERSION;
        write_state(&state)?;
    }
    Ok(ready_view(&state, output))
}

fn set_image(image_path: &str, filter: &str) -> Result<WallpaperView, String> {
    let filter = wallpaper_filter(filter);
    let (source_name, source_file) = copy_source(image_path)?;
    let state = WallpaperState {
        source_name,
        source_file: source_file.to_string_lossy().into_owned(),
        filter: filter.into(),
        filter_version: FILTER_VERSION,
    };
    write_state(&state)?;
    let output = render(&source_file, filter)?;
    Ok(ready_view(&state, &output))
}

fn set_filter(filter: &str) -> Result<WallpaperView, String> {
    let filter = wallpaper_filter(filter);
    let Some(mut state) = read_state()? else {
        return Err("Choose an image first.".into());
    };
    let source = PathBuf::from(&state.source_file);
    if !source.is_file() {
        return Err("Choose an image first.".into());
    }
    state.filter = filter.into();
    state.filter_version = FILTER_VERSION;
    write_state(&state)?;
    let output = render(&source, filter)?;
    Ok(ready_view(&state, &output))
}

fn clear() -> Result<WallpaperView, String> {
    remove_known_files();
    Ok(empty_view("Choose an image."))
}

fn copy_source(image_path: &str) -> Result<(String, PathBuf), String> {
    let source = PathBuf::from(image_path);
    if !source.is_file() {
        return Err("Choose an image file.".into());
    }
    let meta = fs::metadata(&source).map_err(|_| "Could not read that image.".to_string())?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err("That image is too large.".into());
    }
    let ext = source
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| {
            matches!(
                value.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
        .ok_or_else(|| "Use a PNG, JPEG, WebP, GIF, or BMP image.".to_string())?;
    let dest_ext = if ext == "jpeg" { "jpg" } else { ext.as_str() };
    remove_sources();
    let dest = PathBuf::from(format!("source.{dest_ext}"));
    fs::copy(&source, &dest).map_err(|_| "Could not copy that image.".to_string())?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("wallpaper")
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect::<String>();
    Ok((name, dest))
}

fn render(source: &Path, filter: &str) -> Result<String, String> {
    let image = image::open(source).map_err(|_| "Could not read that image.".to_string())?;
    let mut rgb = downscale_for_output(image.to_rgb8());
    match filter {
        "print" => apply_print_look(&mut rgb),
        "noir" => apply_noir(&mut rgb),
        "frosted" => rgb = frosted(&rgb),
        _ => {}
    }

    // Print and noir carry per-pixel detail that JPEG smears, so they get PNG;
    // the smooth looks stay JPEG to keep the data URL the host inlines small.
    let output = output_name(filter);
    // Noir is grey by definition, so one channel carries everything three did
    // and the file it has to fit into is a third the size.
    let encoded = if filter == "noir" {
        DynamicImage::ImageLuma8(DynamicImage::ImageRgb8(rgb).into_luma8())
    } else {
        DynamicImage::ImageRgb8(rgb)
    };
    encoded
        .save(output)
        .map_err(|_| "Could not save the wallpaper.".to_string())?;
    let stale = if output == OUTPUT_PNG {
        OUTPUT_JPG
    } else {
        OUTPUT_PNG
    };
    let _ = fs::remove_file(stale);
    Ok(output.to_string())
}

/// Bounds the long edge *and* the pixel count.
///
/// The edge cap alone lets a square photo through at 1600×1600, which is 2.8×
/// the pixels a landscape one becomes. That matters because the print look
/// dithers every pixel and dither noise is close to incompressible — PNG came
/// out at 0.92 of raw on a real 8K photo — so pixel count, not edge length, is
/// what decides whether the file still fits the host's limit.
fn downscale_for_output(image: RgbImage) -> RgbImage {
    let (width, height) = image.dimensions();
    let long = f64::from(width.max(height));
    let pixels = u64::from(width) * u64::from(height);
    let by_edge = (f64::from(MAX_EDGE) / long).min(1.0);
    let by_pixels = (MAX_PIXELS as f64 / pixels as f64).sqrt().min(1.0);
    let scale = by_edge.min(by_pixels);
    if scale >= 1.0 {
        return image;
    }
    let next_width = ((f64::from(width) * scale).round() as u32).max(1);
    let next_height = ((f64::from(height) * scale).round() as u32).max(1);
    imageops::resize(
        &image,
        next_width,
        next_height,
        imageops::FilterType::Triangle,
    )
}

/// Bayer print look per RGB channel, then a sparse lighten/darken speckle.
pub fn apply_print_look(image: &mut RgbImage) {
    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = image.get_pixel(x, y);
            let mut rgb = [
                dither_channel(x, y, pixel[0]),
                dither_channel(x, y, pixel[1]),
                dither_channel(x, y, pixel[2]),
            ];
            if let Some(lighten) = speckle_kind(x, y) {
                let target = if lighten { 255 } else { 0 };
                rgb = rgb.map(|channel| mix_toward(channel, target));
            }
            image.put_pixel(x, y, Rgb(rgb));
        }
    }
}

pub fn dither_channel(x: u32, y: u32, value: u8) -> u8 {
    let n = TONE_LEVELS - 1;
    let bayer = u32::from(BAYER8[(y as usize) % 8][(x as usize) % 8]);
    let q = (u32::from(value) * n * 64 + (bayer + 1) * 255) / (255 * 64);
    let q = q.min(n);
    ((q * 255) / n) as u8
}

/// About one pixel in 43 is lightened or darkened. `true` means lighten.
pub fn speckle_kind(x: u32, y: u32) -> Option<bool> {
    let mut hash = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    hash ^= hash >> 33;
    if hash.is_multiple_of(43) {
        Some((hash >> 8).is_multiple_of(2))
    } else {
        None
    }
}

fn mix_toward(channel: u8, target: u8) -> u8 {
    const WEIGHT: u16 = 35;
    ((u16::from(channel) * (100 - WEIGHT) + u16::from(target) * WEIGHT) / 100) as u8
}

/// Black and white with a lifted contrast curve and a little film grain.
pub fn apply_noir(image: &mut RgbImage) {
    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = image.get_pixel(x, y);
            let mut tone = contrast(luma8(pixel[0], pixel[1], pixel[2]));
            if let Some(lighten) = speckle_kind(x, y) {
                tone = mix_toward(tone, if lighten { 255 } else { 0 });
            }
            image.put_pixel(x, y, Rgb([tone, tone, tone]));
        }
    }
}

/// Frosted glass: a wide blur that keeps the colour but drops the detail, so
/// text layered over it has far less to compete with.
pub fn frosted(image: &RgbImage) -> RgbImage {
    let (width, height) = image.dimensions();
    let sigma = (width.max(height) as f32 * FROST_RADIUS_RATIO).max(1.0);
    imageops::blur(image, sigma)
}

fn luma8(red: u8, green: u8, blue: u8) -> u8 {
    ((u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114) / 1000) as u8
}

/// Push tones away from mid grey without clipping either end.
fn contrast(tone: u8) -> u8 {
    let normalized = f32::from(tone) / 255.0;
    let curved = normalized + 0.55 * normalized * (1.0 - normalized) * (2.0 * normalized - 1.0);
    (curved.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn read_state() -> Result<Option<WallpaperState>, String> {
    if !Path::new(STATE_FILE).is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(STATE_FILE)
        .map_err(|_| "Could not read wallpaper state.".to_string())?;
    serde_json::from_str(&raw).map_err(|_| "Wallpaper state is not valid.".to_string())
}

fn write_state(state: &WallpaperState) -> Result<(), String> {
    let raw =
        serde_json::to_string(state).map_err(|_| "Could not save wallpaper state.".to_string())?;
    fs::write(STATE_FILE, raw).map_err(|_| "Could not save wallpaper state.".to_string())
}

fn remove_known_files() {
    remove_sources();
    let _ = fs::remove_file(OUTPUT_PNG);
    let _ = fs::remove_file(OUTPUT_JPG);
    let _ = fs::remove_file(STATE_FILE);
}

fn remove_sources() {
    for name in [
        "source.png",
        "source.jpg",
        "source.jpeg",
        "source.webp",
        "source.gif",
        "source.bmp",
    ] {
        let _ = fs::remove_file(name);
    }
}

fn output_name(filter: &str) -> &'static str {
    match filter {
        "print" | "noir" => OUTPUT_PNG,
        _ => OUTPUT_JPG,
    }
}

fn ready_view(state: &WallpaperState, image_file: &str) -> WallpaperView {
    WallpaperView {
        status: "ready".into(),
        source_name: Some(state.source_name.clone()),
        filter: wallpaper_filter(&state.filter).into(),
        image_file: Some(image_file.into()),
        detail: "Ready".into(),
        observed_at: now_secs(),
    }
}

fn empty_view(detail: &str) -> WallpaperView {
    WallpaperView {
        status: "empty".into(),
        source_name: None,
        filter: "none".into(),
        image_file: None,
        detail: detail.into(),
        observed_at: now_secs(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zest_plugin_api::PROTOCOL_VERSION;

    #[test]
    fn protocol_version_is_stable() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn set_wallpaper_request_round_trips() {
        let request = PluginRequest::SetWallpaper {
            image_path: "C:/photo.jpg".into(),
            filter: "frosted".into(),
        };
        let raw = serde_json::to_string(&request).expect("request should serialize");
        assert_eq!(
            raw,
            r#"{"action":"setWallpaper","imagePath":"C:/photo.jpg","filter":"frosted"}"#
        );
        let decoded: PluginRequest = serde_json::from_str(&raw).expect("request should parse");
        let PluginRequest::SetWallpaper { filter, .. } = decoded else {
            panic!("the request should decode as setWallpaper");
        };
        assert_eq!(filter, "frosted");
    }

    #[test]
    fn an_unknown_filter_falls_back_to_none() {
        assert_eq!(wallpaper_filter("frosted"), "frosted");
        assert_eq!(wallpaper_filter("noir"), "noir");
        assert_eq!(wallpaper_filter("print"), "print");
        assert_eq!(wallpaper_filter(""), "none");
        assert_eq!(wallpaper_filter("kaleidoscope"), "none");
    }

    #[test]
    fn the_pixel_budget_holds_whatever_shape_the_photo_is() {
        // A square 8K photo is the case the edge cap alone missed: it passes
        // 1600x1600, and the print look then dithers 2.8x the pixels a
        // landscape one would into a file that barely compresses.
        for (width, height) in [(7680, 4320), (4320, 4320), (2000, 8000), (8000, 1000)] {
            let out = downscale_for_output(RgbImage::new(width, height));
            let (out_width, out_height) = out.dimensions();
            let pixels = u64::from(out_width) * u64::from(out_height);
            assert!(
                pixels <= MAX_PIXELS,
                "{width}x{height} came out {out_width}x{out_height} = {pixels} pixels"
            );
            assert!(out_width.max(out_height) <= MAX_EDGE);
            // Cropping is not resizing; the shape has to survive.
            let before = f64::from(width) / f64::from(height);
            let after = f64::from(out_width) / f64::from(out_height);
            assert!((before - after).abs() < 0.01, "{before} became {after}");
        }
    }

    #[test]
    fn a_photo_already_inside_the_budget_is_left_alone() {
        let out = downscale_for_output(RgbImage::new(1280, 720));
        assert_eq!(out.dimensions(), (1280, 720));
    }

    #[test]
    fn only_the_detailed_looks_are_written_as_png() {
        assert_eq!(output_name("print"), OUTPUT_PNG);
        assert_eq!(output_name("noir"), OUTPUT_PNG);
        assert_eq!(output_name("frosted"), OUTPUT_JPG);
        assert_eq!(output_name("none"), OUTPUT_JPG);
    }

    #[test]
    fn dither_channel_keeps_black_and_white_ends() {
        assert_eq!(dither_channel(0, 0, 0), 0);
        assert_eq!(dither_channel(0, 0, 255), 255);
        let mid = (0..8)
            .flat_map(|y| (0..8).map(move |x| dither_channel(x, y, 128)))
            .collect::<Vec<_>>();
        assert!(mid.iter().any(|tone| *tone != 0 && *tone != 255));
    }

    #[test]
    fn speckle_kind_is_sparse_and_deterministic() {
        let first = speckle_kind(3, 7);
        let second = speckle_kind(3, 7);
        assert_eq!(first, second);
        let speckled = (0..64)
            .flat_map(|y| (0..64).filter_map(move |x| speckle_kind(x, y)))
            .count();
        assert!(speckled > 20);
        assert!(speckled < 200);
    }

    #[test]
    fn print_look_keeps_a_blue_photo_in_color() {
        let mut image = RgbImage::from_pixel(16, 16, Rgb([90, 140, 200]));
        apply_print_look(&mut image);
        let unequal = image
            .pixels()
            .filter(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2])
            .count();
        let blue_strongest = image
            .pixels()
            .filter(|pixel| pixel[2] >= pixel[0] && pixel[2] >= pixel[1])
            .count();
        let not_one_bit = image
            .pixels()
            .any(|pixel| pixel.0.iter().any(|channel| *channel != 0 && *channel != 255));
        assert!(unequal > 50);
        assert!(blue_strongest > 100);
        assert!(not_one_bit);
    }

    #[test]
    fn print_look_keeps_mid_gray_neutral() {
        let mut image = RgbImage::from_pixel(16, 16, Rgb([128, 128, 128]));
        apply_print_look(&mut image);
        for pixel in image.pixels() {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
        }
        assert!(image.pixels().any(|pixel| pixel[0] != 0 && pixel[0] != 255));
    }

    #[test]
    fn noir_is_grey_but_never_one_bit() {
        let mut image = RgbImage::from_pixel(24, 24, Rgb([90, 140, 200]));
        apply_noir(&mut image);
        for pixel in image.pixels() {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
        }
        assert!(image.pixels().any(|pixel| pixel[0] != 0 && pixel[0] != 255));
    }

    #[test]
    fn contrast_pushes_away_from_mid_grey_without_clipping() {
        assert_eq!(contrast(0), 0);
        assert_eq!(contrast(255), 255);
        assert_eq!(contrast(128), 128);
        assert!(contrast(80) < 80);
        assert!(contrast(180) > 180);
    }

    #[test]
    fn frosted_keeps_colour_and_softens_detail() {
        let mut image = RgbImage::from_pixel(64, 64, Rgb([20, 20, 30]));
        for y in 0..64 {
            for x in 0..32 {
                image.put_pixel(x, y, Rgb([220, 90, 40]));
            }
        }
        let blurred = frosted(&image);
        assert_eq!(blurred.dimensions(), image.dimensions());

        // The hard seam down the middle should become a gradient, and the warm
        // half should still read as warm rather than grey.
        let seam = (28..36)
            .map(|x| blurred.get_pixel(x, 32)[0])
            .collect::<Vec<_>>();
        assert!(seam.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!(seam.iter().any(|red| *red != 220 && *red != 20));
        let warm = blurred.get_pixel(4, 32);
        assert!(warm[0] > warm[1] && warm[1] > warm[2]);
    }

    #[test]
    fn set_image_renders_each_look_and_replaces_the_last_file() {
        let root = tempfile::tempdir().expect("temp folder should exist");
        let source = root.path().join("photo.png");
        RgbImage::from_pixel(16, 16, Rgb([90, 140, 200]))
            .save(&source)
            .expect("source png should save");
        let original = std::env::current_dir().expect("cwd should exist");
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let _restore = Restore(original);
        std::env::set_current_dir(root.path()).expect("cwd should change");

        let view = handle(PluginRequest::SetWallpaper {
            image_path: source.to_string_lossy().into_owned(),
            filter: "print".into(),
        })
        .expect("a valid png should render");
        assert_eq!(view.status, "ready");
        assert_eq!(view.filter, "print");
        assert_eq!(view.image_file.as_deref(), Some("wallpaper.png"));
        assert!(root.path().join("wallpaper.png").is_file());

        let view = handle(PluginRequest::SetWallpaperFilter {
            filter: "frosted".into(),
        })
        .expect("switching looks should re-render");
        assert_eq!(view.filter, "frosted");
        assert_eq!(view.image_file.as_deref(), Some("wallpaper.jpg"));
        assert!(root.path().join("wallpaper.jpg").is_file());
        assert!(!root.path().join("wallpaper.png").exists());
    }
}
