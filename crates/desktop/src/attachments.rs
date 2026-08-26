//! File attachment prep for the composer.
//!
//! PDFs go through the local PDF inspector for classification + Markdown
//! extraction (no OCR). Images become Messages API image blocks. Other files
//! are read as UTF-8 text when possible.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use zest_core::truncate_chars;

/// Soft cap so a single text attach cannot blow the context window.
const MAX_ATTACHMENT_CHARS: usize = 100_000;

/// Per-image ceiling.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Ceiling across everything attached to one message.
///
/// Per-file limits do not compose. Five images at the per-file maximum are
/// 40 MiB of pixels and roughly 53 MiB once base64 encoded — and that encoding
/// crosses the IPC boundary, is held in webview state, and is copied again into
/// a `data:` URL for the preview. A per-file limit alone bounds none of that.
pub const MAX_TOTAL_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// Ceiling on how many images one message may carry.
pub const MAX_IMAGES: usize = 8;

/// Ceiling on either edge, in pixels.
///
/// Providers downsample large images before looking at them, so a 12000px scan
/// costs full price to encode, ship and store in order to be thrown away at the
/// other end. Measured from the file header — decoding to find out would mean
/// taking on an image codec, and the header is where the answer already is.
pub const MAX_IMAGE_EDGE: u32 = 8_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedAttachment {
    pub id: String,
    pub name: String,
    pub path: String,
    /// `pdf` | `text` | `image` | `error`
    pub kind: String,
    /// `done` | `error`
    pub status: String,
    /// Short label for chips / display message.
    pub detail: String,
    /// Text body for pdf/text; unused for images.
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Raw base64 (no data-URL prefix) for image blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    pub name: String,
    pub detail: String,
    pub content: Option<String>,
    pub status: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub data_base64: Option<String>,
}

pub fn prepare_paths(paths: &[PathBuf]) -> Vec<PreparedAttachment> {
    let mut prepared = Vec::with_capacity(paths.len());
    let mut images = 0usize;
    let mut image_bytes = 0usize;

    for path in paths {
        let one = prepare_one(path);
        // Counted only for images that were actually accepted, so a rejected
        // file never consumes part of the budget for the ones after it.
        if one.kind == "image" && one.status == "done" {
            let bytes = decoded_len(one.data_base64.as_deref());
            match refuse_over_budget(&one, images, image_bytes, bytes) {
                Some(refusal) => {
                    prepared.push(refusal);
                    continue;
                }
                None => {
                    images += 1;
                    image_bytes += bytes;
                }
            }
        }
        prepared.push(one);
    }
    prepared
}

/// Turn an accepted image into a refusal when it would exceed a batch budget.
fn refuse_over_budget(
    one: &PreparedAttachment,
    images_so_far: usize,
    bytes_so_far: usize,
    bytes: usize,
) -> Option<PreparedAttachment> {
    let detail = if images_so_far >= MAX_IMAGES {
        format!("too many images (max {MAX_IMAGES})")
    } else if bytes_so_far + bytes > MAX_TOTAL_IMAGE_BYTES {
        format!(
            "images exceed {} MB in total",
            MAX_TOTAL_IMAGE_BYTES / (1024 * 1024)
        )
    } else {
        return None;
    };

    Some(PreparedAttachment {
        id: one.id.clone(),
        name: one.name.clone(),
        path: one.path.clone(),
        kind: "error".into(),
        status: "error".into(),
        detail,
        content: None,
        media_type: None,
        data_base64: None,
    })
}

/// How many bytes a base64 payload decodes to, without decoding it.
fn decoded_len(base64: Option<&str>) -> usize {
    let Some(encoded) = base64 else { return 0 };
    let padding = encoded.bytes().rev().take_while(|b| *b == b'=').count();
    encoded.len() / 4 * 3 - padding.min(2)
}

/// Pixel dimensions read from a file header.
///
/// `None` for a format this does not recognise, which is treated as "no opinion"
/// rather than as a rejection — refusing an image because the header could not
/// be read would turn an unknown format into a broken one.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    let le16 = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };

    // PNG: signature, then the IHDR chunk carries the size at a fixed offset.
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    // GIF: logical screen descriptor, little-endian, right after the version.
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return webp_dimensions(bytes);
    }
    if bytes.starts_with(b"\xff\xd8") {
        return jpeg_dimensions(bytes);
    }
    None
}

/// JPEG keeps its size in a start-of-frame marker, which sits after a variable
/// number of other segments, so the segment chain has to be walked.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2usize;
    loop {
        // Markers may be preceded by any number of 0xFF fill bytes.
        while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if bytes.get(at)? != &0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        let length = u16::from_be_bytes(bytes.get(at + 2..at + 4)?.try_into().ok()?) as usize;

        // SOF0..SOF15, excluding the four that are not frame headers.
        let is_frame =
            (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC | 0xD8 | 0xD9);
        if is_frame {
            let height = u16::from_be_bytes(bytes.get(at + 5..at + 7)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(at + 7..at + 9)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        at = at.checked_add(2)?.checked_add(length)?;
    }
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        // Extended format states the size directly, minus one, over 24 bits.
        // Indexed through `get` because a truncated file is a file someone can
        // hand us, and panicking on it would take the whole app down.
        b"VP8X" => {
            let w = bytes.get(24..27)?;
            let h = bytes.get(27..30)?;
            Some((
                u32::from_le_bytes([w[0], w[1], w[2], 0]) + 1,
                u32::from_le_bytes([h[0], h[1], h[2], 0]) + 1,
            ))
        }
        b"VP8 " => {
            let w = u32::from(u16::from_le_bytes(bytes.get(26..28)?.try_into().ok()?) & 0x3FFF);
            let h = u32::from(u16::from_le_bytes(bytes.get(28..30)?.try_into().ok()?) & 0x3FFF);
            Some((w, h))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

pub fn prepare_image_bytes(bytes: &[u8], media_type: &str, name: &str) -> PreparedAttachment {
    let id = format!("att-{}", zest_core::new_id("file"));
    if bytes.is_empty() {
        return PreparedAttachment {
            id,
            name: name.to_string(),
            path: name.to_string(),
            kind: "error".into(),
            status: "error".into(),
            detail: "empty image".into(),
            content: None,
            media_type: None,
            data_base64: None,
        };
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return PreparedAttachment {
            id,
            name: name.to_string(),
            path: name.to_string(),
            kind: "error".into(),
            status: "error".into(),
            detail: format!(
                "image too large (max {} MB)",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ),
            content: None,
            media_type: None,
            data_base64: None,
        };
    }
    if let Some((width, height)) = image_dimensions(bytes) {
        if width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE {
            return PreparedAttachment {
                id,
                name: name.to_string(),
                path: name.to_string(),
                kind: "error".into(),
                status: "error".into(),
                detail: format!("image is {width}x{height} (max {MAX_IMAGE_EDGE}px per side)"),
                content: None,
                media_type: None,
                data_base64: None,
            };
        }
    }
    let mt = normalize_media_type(media_type);
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let kb = bytes.len().div_ceil(1024);
    PreparedAttachment {
        id,
        name: name.to_string(),
        path: name.to_string(),
        kind: "image".into(),
        status: "done".into(),
        detail: format!("{kb} KB · {mt}"),
        content: None,
        media_type: Some(mt),
        data_base64: Some(b64),
    }
}

fn prepare_one(path: &Path) -> PreparedAttachment {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let display = path.display().to_string();
    let id = format!("att-{}", zest_core::new_id("file"));

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "pdf" {
        return prepare_pdf(path, id, name, display);
    }
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return prepare_image_path(path, id, name, display, &ext);
    }

    match read_text_file(path) {
        Ok(text) => {
            let chars = text.chars().count();
            let body = truncate_chars(&text, MAX_ATTACHMENT_CHARS);
            PreparedAttachment {
                id,
                name,
                path: display,
                kind: "text".into(),
                status: "done".into(),
                detail: format!("{chars} chars"),
                content: Some(body),
                media_type: None,
                data_base64: None,
            }
        }
        Err(err) => PreparedAttachment {
            id,
            name,
            path: display,
            kind: "error".into(),
            status: "error".into(),
            detail: err,
            content: None,
            media_type: None,
            data_base64: None,
        },
    }
}

fn prepare_image_path(
    path: &Path,
    id: String,
    name: String,
    display: String,
    ext: &str,
) -> PreparedAttachment {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut att = prepare_image_bytes(&bytes, media_type_for_ext(ext), &name);
            att.id = id;
            att.path = display;
            att
        }
        Err(err) => PreparedAttachment {
            id,
            name,
            path: display,
            kind: "error".into(),
            status: "error".into(),
            detail: err.to_string(),
            content: None,
            media_type: None,
            data_base64: None,
        },
    }
}

fn prepare_pdf(path: &Path, id: String, name: String, display: String) -> PreparedAttachment {
    match pdf_inspector::process_pdf(path) {
        Ok(result) => {
            let kind_label = format!("{:?}", result.pdf_type);
            let pages = result.page_count;
            match result.markdown {
                Some(md) if !md.trim().is_empty() => {
                    let body = truncate_chars(&md, MAX_ATTACHMENT_CHARS);
                    PreparedAttachment {
                        id,
                        name,
                        path: display,
                        kind: "pdf".into(),
                        status: "done".into(),
                        detail: format!("{kind_label}, {pages} pages"),
                        content: Some(body),
                        media_type: None,
                        data_base64: None,
                    }
                }
                _ => PreparedAttachment {
                    id,
                    name,
                    path: display,
                    kind: "pdf".into(),
                    status: "error".into(),
                    detail: format!(
                        "{kind_label}, {pages} pages — no extractable text (OCR not available)"
                    ),
                    content: None,
                    media_type: None,
                    data_base64: None,
                },
            }
        }
        Err(err) => PreparedAttachment {
            id,
            name,
            path: display,
            kind: "error".into(),
            status: "error".into(),
            detail: format!("PDF read failed: {err}"),
            content: None,
            media_type: None,
            data_base64: None,
        },
    }
}

fn read_text_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("binary file — only text, images, and PDF are supported".into());
    }
    String::from_utf8(bytes).map_err(|_| "not valid UTF-8 text".into())
}

fn media_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn normalize_media_type(raw: &str) -> String {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "image/jpg" => "image/jpeg".into(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => t,
        other if other.starts_with("image/") => other.to_string(),
        _ => "image/png".into(),
    }
}

/// Compact line shown in the chat bubble.
pub fn format_display_message(text: &str, attachments: &[AttachmentInput]) -> String {
    let mut out = text.trim().to_string();
    if attachments.is_empty() {
        return out;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    for att in attachments {
        out.push_str(&format!("Attached: {} ({})", att.name, att.detail));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Build Messages API user content blocks (text + images).
pub fn build_user_content(text: &str, attachments: &[AttachmentInput]) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut text_body = text.trim().to_string();

    let text_atts: Vec<_> = attachments
        .iter()
        .filter(|a| {
            let kind = a.kind.as_deref().unwrap_or("");
            kind != "image"
                && a.status == "done"
                && a.content.as_ref().is_some_and(|c| !c.trim().is_empty())
        })
        .collect();
    let images: Vec<_> = attachments
        .iter()
        .filter(|a| {
            a.kind.as_deref() == Some("image")
                && a.status == "done"
                && a.data_base64.as_ref().is_some_and(|d| !d.is_empty())
        })
        .collect();
    let failed: Vec<_> = attachments
        .iter()
        .filter(|a| {
            !(a.status == "done"
                && (a.content.as_ref().is_some_and(|c| !c.trim().is_empty())
                    || (a.kind.as_deref() == Some("image")
                        && a.data_base64.as_ref().is_some_and(|d| !d.is_empty()))))
        })
        .collect();

    if !text_atts.is_empty() {
        if !text_body.is_empty() {
            text_body.push_str("\n\n");
        }
        text_body.push_str("---\nAttached files:\n");
        for att in &text_atts {
            let content = att.content.as_deref().unwrap_or("");
            text_body.push_str(&format!(
                "\n### {}\n({})\n\n{}\n",
                att.name, att.detail, content
            ));
        }
    }
    if !failed.is_empty() {
        if !text_body.is_empty() {
            text_body.push_str("\n\n");
        }
        text_body.push_str("Could not extract:\n");
        for att in failed {
            text_body.push_str(&format!("- {} — {}\n", att.name, att.detail));
        }
    }

    if !text_body.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": text_body.trim() }));
    } else if images.is_empty() {
        blocks.push(json!({ "type": "text", "text": "(empty)" }));
    }

    for img in images {
        let media = img.media_type.clone().unwrap_or_else(|| "image/png".into());
        let data = img.data_base64.clone().unwrap_or_default();
        blocks.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media,
                "data": data,
            }
        }));
    }

    blocks
}

pub fn has_images(attachments: &[AttachmentInput]) -> bool {
    attachments.iter().any(|a| {
        a.kind.as_deref() == Some("image")
            && a.status == "done"
            && a.data_base64.as_ref().is_some_and(|d| !d.is_empty())
    })
}

pub fn has_usable_attachment(attachments: &[AttachmentInput]) -> bool {
    attachments.iter().any(|a| {
        a.status == "done"
            && (a.content.as_ref().is_some_and(|c| !c.trim().is_empty())
                || (a.kind.as_deref() == Some("image")
                    && a.data_base64.as_ref().is_some_and(|d| !d.is_empty())))
    })
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    /// A minimal PNG header. Only the IHDR size field is read, so the pixel
    /// data is irrelevant — which is the point of reading the header.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 6, 0, 0, 0]);
        out
    }

    fn gif(width: u16, height: u16) -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        // An APP0 segment first, so the walk has to skip something real before
        // it reaches the frame header.
        out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        out.extend_from_slice(&[0u8; 14]);
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out
    }

    #[test]
    fn dimensions_come_out_of_the_header_for_each_known_format() {
        assert_eq!(image_dimensions(&png(1920, 1080)), Some((1920, 1080)));
        assert_eq!(image_dimensions(&gif(640, 480)), Some((640, 480)));
        assert_eq!(image_dimensions(&jpeg(4032, 3024)), Some((4032, 3024)));
    }

    #[test]
    fn an_unreadable_header_is_no_opinion_rather_than_a_rejection() {
        // Refusing what cannot be measured would turn an unknown format into a
        // broken one.
        assert_eq!(image_dimensions(b"not an image at all"), None);
        assert_eq!(image_dimensions(&[]), None);
        // Truncated PNG: the signature matches but the size field is missing.
        assert_eq!(image_dimensions(b"\x89PNG\r\n\x1a\n\x00\x00"), None);
        // Truncated WebP, for each chunk type. A malformed file is one a user
        // can hand us, so every one of these must return rather than panic.
        for chunk in [&b"VP8X"[..], b"VP8 ", b"VP8L"] {
            let mut truncated = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
            truncated.extend_from_slice(chunk);
            assert_eq!(image_dimensions(&truncated), None, "{chunk:?}");
        }
        // A JPEG whose segment chain runs off the end must terminate too.
        assert_eq!(image_dimensions(b"\xff\xd8\xff\xe0\x00\xff"), None);
    }

    #[test]
    fn a_well_formed_webp_still_reads() {
        let mut vp8x = b"RIFF\x00\x00\x00\x00WEBPVP8X".to_vec();
        vp8x.extend_from_slice(&[0u8; 8]);
        // 24-bit little-endian, stored as size minus one.
        vp8x.extend_from_slice(&[0x3F, 0x00, 0x00]); // 63 -> 64
        vp8x.extend_from_slice(&[0x1F, 0x00, 0x00]); // 31 -> 32
        assert_eq!(image_dimensions(&vp8x), Some((64, 32)));
    }

    #[test]
    fn an_oversized_image_is_refused_before_it_is_encoded() {
        let huge = png(MAX_IMAGE_EDGE + 1, 100);
        let prepared = prepare_image_bytes(&huge, "image/png", "huge.png");
        assert_eq!(prepared.status, "error");
        assert!(
            prepared.detail.contains("max 8000px"),
            "{}",
            prepared.detail
        );
        // The refusal must not carry the payload it refused.
        assert!(prepared.data_base64.is_none());

        let fine = prepare_image_bytes(&png(1920, 1080), "image/png", "ok.png");
        assert_eq!(fine.status, "done");
        assert!(fine.data_base64.is_some());
    }

    #[test]
    fn decoded_len_matches_the_real_decoded_size() {
        for len in [0usize, 1, 2, 3, 4, 5, 1000, 1001] {
            let raw = vec![7u8; len];
            let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
            assert_eq!(decoded_len(Some(&encoded)), len, "len {len}");
        }
        assert_eq!(decoded_len(None), 0);
    }

    #[test]
    fn a_batch_stops_accepting_images_once_the_count_is_reached() {
        let ok = PreparedAttachment {
            id: "a".into(),
            name: "a.png".into(),
            path: "a.png".into(),
            kind: "image".into(),
            status: "done".into(),
            detail: "1 KB".into(),
            content: None,
            media_type: Some("image/png".into()),
            data_base64: Some("AAAA".into()),
        };
        assert!(refuse_over_budget(&ok, MAX_IMAGES - 1, 0, 3).is_none());

        let refused = refuse_over_budget(&ok, MAX_IMAGES, 0, 3).expect("over count");
        assert_eq!(refused.status, "error");
        assert!(refused.detail.contains("too many images"));
        assert!(refused.data_base64.is_none());
    }

    #[test]
    fn a_batch_stops_accepting_images_once_the_total_is_reached() {
        // Five images at the per-file maximum is the case a per-file limit
        // alone does nothing about.
        let ok = PreparedAttachment {
            id: "a".into(),
            name: "a.png".into(),
            path: "a.png".into(),
            kind: "image".into(),
            status: "done".into(),
            detail: "8 MB".into(),
            content: None,
            media_type: Some("image/png".into()),
            data_base64: Some("AAAA".into()),
        };
        let just_under = MAX_TOTAL_IMAGE_BYTES - MAX_IMAGE_BYTES;
        assert!(refuse_over_budget(&ok, 1, just_under, MAX_IMAGE_BYTES).is_none());

        let refused =
            refuse_over_budget(&ok, 1, just_under + 1, MAX_IMAGE_BYTES).expect("over total");
        assert!(refused.detail.contains("in total"), "{}", refused.detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_lists_attachment_names() {
        let atts = vec![AttachmentInput {
            name: "a.pdf".into(),
            detail: "TextBased, 2 pages".into(),
            content: Some("hello".into()),
            status: "done".into(),
            kind: Some("pdf".into()),
            media_type: None,
            data_base64: None,
        }];
        let display = format_display_message("Please summarize", &atts);
        assert!(display.contains("Please summarize"));
        assert!(display.contains("Attached: a.pdf"));
        assert!(!display.contains("hello"));
    }

    #[test]
    fn image_block_in_user_content() {
        let atts = vec![AttachmentInput {
            name: "shot.png".into(),
            detail: "12 KB".into(),
            content: None,
            status: "done".into(),
            kind: Some("image".into()),
            media_type: Some("image/png".into()),
            data_base64: Some("AAAA".into()),
        }];
        let blocks = build_user_content("what is this?", &atts);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["data"], "AAAA");
    }
}
