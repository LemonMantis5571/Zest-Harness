---
title: Attachments
summary: Attach files, PDFs, and images to a message from the file picker or by pasting, within per-image and per-message limits.
paths:
  - crates/desktop/src/attachments.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/composer.rs
  - crates/desktop/src/native.rs
tests:
  - crates/desktop/src/workspace_test.rs
  - crates/desktop/src/selftest.rs
verify:
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- attachments::
  - npm run gpui:test
---

# Attachments

## Behavior
- The composer's "+" button ("Attach files") opens the system file picker (multiple files, no type filters). Pasting image data into the composer attaches it as a thumbnail; pasted file paths are ignored.
- Supported: images (`png`, `jpg`/`jpeg`, `gif`, `webp`), PDFs (text extraction only, no OCR), and any other UTF-8 text file. Text and PDF bodies are cut to 100,000 characters.
- Limits: 8 MiB and 8,000 px per side per image; at most 8 images and 16 MiB of images per message. The picker applies the limits to each pick; the send checks the count and total across picked and pasted images and refuses the message with "A message can include up to 8 images totaling 16 MiB", keeping the draft.
- A file that cannot be read (binary, invalid UTF-8, over a limit) is not added; the notice card above the composer names it and the reason. The desktop app does not keep error chips or tell the model about failed files.
- Picked files show as chips ("name ×") in the composer toolbar; clicking a chip removes it. The desktop app does not preview attachments in a lightbox.
- A message can be only attachments; the send button is enabled when the draft has text, a pasted image or a picked file.
- The transcript shows the typed text plus `Attached: <name> (<detail>)` lines once the turn's rows are rebuilt from the saved message. While the turn runs, the user bubble shows the typed text and an "N image attachment(s)" line.
- Messages sent while a turn is running keep their attachments in the durable queue; a queued row shows "N images" and "N files".
- Question answers never take the composer's attachments, and `/btw` side questions leave them in the composer.

## How it works
- Pick: `native_view.rs` `pick_files` -> `cx.prompt_for_paths` -> `attachments::prepare_paths` on the background executor -> `prepare_one` (`prepare_pdf` via `pdf_inspector`, `prepare_image_path`, or `read_text_file`). `prepare_paths` applies the batch limits (`refuse_over_budget`). Ready files become `ThreadInputAttachment`s in `Root::files` (`native::prepared`).
- Paste: `composer.rs` `paste` keeps clipboard images on the composer; `ComposerEvent::Send` carries them. `native_view.rs` `accept_submission` turns each into an attachment with `attachments::prepare_image_bytes` (size, header dimensions via `image_dimensions`, media type normalisation, base64), then checks `MAX_IMAGES` and `MAX_TOTAL_IMAGE_BYTES` over every image in the message.
- Queue and send: `native::Chat::enqueue` stores the attachments on the `ThreadInput`. `Chat::run` claims the input, converts the attachments back with `native::to_attachment`, and uses `format_display_message` (transcript text) and `build_user_content` (text block with `---\nAttached files:` sections, then base64 image blocks), sent with `Agent::send_blocks_cancellable`.

## Verify
- `cargo test ... -- attachments::` runs `attachments::limit_tests` (header dimensions, oversize refusal, batch count/total, bounded reads) and `attachments::tests` (display text, image blocks).
- `npm run gpui:test` attaches a project file through `prepare_paths`, sends it, and checks that the saved wire message contains its contents.
- `npm run gpui -- --selftest` includes `paste_image`, which pastes a PNG into the composer and checks it becomes one image attachment.

## Pitfalls
- `prepare_paths` sees one pick at a time; the cross-pick image ceiling is enforced in `accept_submission`, so change both together.
- Queued image attachments are stored base64 inside the thread JSON (`pending_inputs`) until claimed.
- Image dimensions come from the file header; an unreadable header is not a rejection.
