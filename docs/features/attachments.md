---
title: Attachments
summary: Attach files, PDFs, and images to a message from the file picker or by pasting, within per-image and per-message limits.
paths:
  - crates/desktop/src/attachments.rs
  - crates/desktop/ui/src/lib/attachmentLimits.ts
  - crates/desktop/ui/src/components/ui/attachment.tsx
tests:
  - crates/desktop/ui/src/lib/attachmentLimits.test.ts
  - crates/desktop/ui/src/lib/composerAttachments.test.ts
verify:
  - npm run ui:test
  - cargo test -p zest-desktop --lib -- attachments::
---

# Attachments

## Behavior
- The composer's "+" menu opens a native picker (Documents / Images / PDF / All files filters, starting in the workspace). Pasting image data into the composer attaches it.
- Supported: images (`png`, `jpg`/`jpeg`, `gif`, `webp`), PDFs (text extraction only, no OCR), and any other UTF-8 text file. Binary files and invalid UTF-8 become error chips with the reason.
- Limits: 8 MiB and 8,000 px per side per image; at most 8 images and 16 MiB of images per message, counted across every pick and paste. Refused images are toasted and not added. Text and PDF bodies are cut to 100,000 characters.
- Failed files stay as error chips (and are toasted) so the user sees why; they are not sent as content, but the model is told "Could not extract: <name> — <reason>".
- Image chips preview and open in the same lightbox as chat images. Chips can be removed before sending.
- A message can be only attachments if at least one is usable (extracted text or image data).
- The transcript shows the typed text plus `Attached: <name> (<detail>)` lines. Filename chips on the user bubble appear for the live send only; a reloaded chat shows the `Attached:` lines.
- Messages sent while a turn is running keep their attachments in the durable queue.
- Question/plan answers never take the composer's attachments. User messages with attachments have no Edit button.

## How it works
- Pick: `Composer.tsx` -> `App.tsx` `onAttachFiles` -> `backend.pickFiles()` -> Tauri `pick_files` (`crates/desktop/src/lib.rs`, `rfd` dialog) -> `attachments::prepare_paths` -> `prepare_one` (`prepare_pdf` via `pdf_inspector`, `prepare_image_path`, or `read_text_file`). `prepare_paths` applies the batch limits (`refuse_over_budget`).
- Paste: `Composer` `onPaste` -> `App.tsx` `onPasteImages` (rejects files over `MAX_IMAGE_BYTES` before reading) -> `backend.preparePastedImage` -> Tauri `prepare_pasted_image` -> `prepare_image_bytes` (size, header dimensions via `image_dimensions`, media type normalisation, base64).
- Merge: `App.tsx` `mergeAttachments` -> `admitAttachments` (`attachmentLimits.ts`, mirrors the Rust constants) enforces the cross-batch image count and total; duplicates by path+name+data prefix are dropped.
- Send: `onSend` / `submitTurn` pass `AttachmentInput`s to `send_message`. `turn.rs` uses `has_usable_attachment`, `format_display_message` (transcript text), and `build_user_content` (text block with `---\nAttached files:` sections, failures list, then base64 image blocks). With images, `turn.rs` calls `Agent::send_blocks_cancellable_with_inbox_and_side_context`; text-only turns keep the single-text-block path.
- Queue: `thread_input_attachment` (`lib.rs`) stores attachments on `ThreadInput`; `resume_queued` / `run_loop` convert them back to `AttachmentInput`.
- UI primitives: `components/ui/attachment.tsx` (`Attachment`, `AttachmentMedia`, `AttachmentTitle`, `AttachmentTrigger`, `AttachmentGroup`, ...) used by `Composer.tsx` and `ChatScreen.tsx`.

## Verify
- `cargo test -p zest-desktop --lib -- attachments::` runs `attachments::limit_tests` (header dimensions, oversize refusal, batch count/total, bounded reads) and `attachments::tests` (display text, image blocks).
- `attachmentLimits.test.ts` covers cross-batch admission; `composerAttachments.test.ts` pins the lightbox wiring and that send clears the draft before submitting.
- In `?fixture=1` the fixture backend returns canned picks (including `sample.pdf`) for checking chips without Tauri.

## Pitfalls
- The image limits are defined twice (Rust `attachments.rs` and `attachmentLimits.ts`); change both together.
- Rust can only see one batch, so the cross-batch ceiling must live in the UI.
- Queued image attachments are stored base64 inside the thread JSON (`pending_inputs`) until claimed.
- Image dimensions come from the file header; an unreadable header is not a rejection.
