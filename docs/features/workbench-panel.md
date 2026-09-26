---
title: Workbench panel
summary: A side panel for a project chat with Activity, Outline, Delegation, and Files tabs, including a quick workspace check, checkpoint rewind, and a read-only file browser.
paths:
  - crates/desktop/ui/src/components/WorkbenchPanel.tsx
  - crates/desktop/src/workspace_files.rs
tests:
  - crates/desktop/ui/e2e/workbench.spec.ts
verify:
  - cargo test -p zest-desktop --lib -- workspace_files
  - cargo test -p zest-desktop --lib -- workspace_review
  - npm run ui:e2e -- workbench
---

# Workbench panel

## Behavior
- Opened and closed from the command palette ("Open workbench" / "Close
  workbench"); there is no toolbar button. Not offered for a free chat (no
  workspace). Esc or "Close Workbench" closes it; focus moves into the panel on
  open.
- Tabs, in order: Activity (default), Outline, Delegation, Files. Arrow keys move
  between tabs.
- Activity shows the provider, model, and Ready / Working / Compacting state,
  subagents and tool tasks from the transcript (click jumps to the message), the
  Workspace check (Verify / Run again, disabled while a turn runs), and
  checkpoints with a Rewind button each.
- The workspace check (`verify_workspace`) only runs `git status` and
  `git diff --check`: it reports changed files (first 24) and whether the patch
  has whitespace errors. It never runs project scripts or changes files.
- Outline lists the conversation's messages; clicking one scrolls to it.
- Delegation is the agent board for delegation jobs (create, approve, cancel,
  retry, apply); its behavior belongs to the delegation feature.
- Files is a read-only, shallow browser rooted at the project: directories first,
  then files, max 400 entries per folder; text preview up to 200000 bytes
  (marked truncated); binary files have no preview.
- The file browser hides `.git`, `.zest`, `target`, `node_modules`, `.venv`,
  `dist`, symlinks, and likely-secret files; asking for a sensitive file directly
  errors with "sensitive files are hidden from the workspace preview".
- No path can leave the project: `..`, absolute, and prefix components are
  refused before canonicalizing, and the canonical path must stay under the root.

## How it works
1. ChatScreen.tsx holds `workbenchOpen` and the palette action
   `toggle-workbench`; it lazily renders `WorkbenchPanel` with the session,
   messages, `workspaceReview`, and delegation callbacks.
2. `WorkbenchBody` (WorkbenchPanel.tsx) owns the tab state; `FileBrowser` calls
   backend `listWorkspaceFiles(relativePath)` and `readWorkspaceFile(path)`,
   discarding stale responses with request counters.
3. Tauri `list_workspace_files` / `read_workspace_file` (crates/desktop/src/lib.rs)
   resolve the workspace root and call `workspace_files::list` / `read` on a
   blocking thread (`resolve_path`, `contains_noise_directory`,
   `is_sensitive_path`, `decode_preview`).
4. The check button calls App.tsx `onVerifyWorkspace` -> Tauri
   `verify_workspace` -> `review_workspace_at` and shows a toast.
5. Rewind calls App.tsx `onRewindThread` -> Tauri `rewind_thread(checkpoint_id)`.

## Verify
The e2e spec opens the Workbench from the palette in `?fixture=1`, visits every
tab, closes it, and fails on any console error. The fixture backend provides
`listWorkspaceFiles` / `readWorkspaceFile` data.

## Pitfalls
- The file browser's hidden-directory list (adds `.venv`, `dist`) differs from
  the agent tools' walk skip list (`.git`, `.zest`, `target`, `node_modules`).
- A UTF-8 character cut by the 200000-byte preview limit is trimmed rather than
  reported as binary; invalid UTF-8 elsewhere means "binary".
- The panel is only mounted while open (`WorkbenchPanel` returns null), so tab
  state resets on every open.
