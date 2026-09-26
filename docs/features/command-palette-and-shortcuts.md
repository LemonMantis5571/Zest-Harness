---
title: Command palette and keyboard shortcuts
summary: Mod+K search over chats, actions, commands, and settings; rebindable shortcuts; Escape closing whatever is on top.
paths:
  - crates/desktop/ui/src/components/CommandPalette.tsx
  - crates/desktop/ui/src/components/KeyboardShortcuts.tsx
  - crates/desktop/ui/src/components/ChatScreen.tsx
  - crates/desktop/ui/src/lib/commandPaletteSearch.ts
  - crates/desktop/ui/src/lib/keybindings.ts
  - crates/desktop/ui/src/lib/useKeybindings.ts
  - crates/desktop/ui/src/lib/escapeStack.ts
  - crates/desktop/ui/src/lib/contextMenu.ts
  - crates/desktop/ui/src/lib/useDialogFocusTrap.ts
tests:
  - crates/desktop/ui/src/lib/commandPaletteSearch.test.ts
  - crates/desktop/ui/src/lib/keybindings.test.ts
  - crates/desktop/ui/src/lib/escapeStack.test.ts
  - crates/desktop/ui/src/lib/contextMenu.test.ts
  - crates/desktop/ui/e2e/sidebar-search.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- sidebar-search
  - npm run ui:e2e -- workbench
---

# Command palette and keyboard shortcuts

## Behavior
- Default chords (`Mod` = Ctrl, or Cmd on macOS): Mod+K palette, Mod+B chat history, Mod+N new chat, Mod+. stop turn, `/` focus composer, Mod+, settings, Mod+Shift+, Customize, Mod+Shift+K shortcuts editor, Mod+Shift+P profile, Mod+Shift+U usage, Mod+Shift+M switch provider.
- The palette is a dialog named "Search" with filter tabs All / Chats / Actions / Settings. On open the input is focused and empty; the sidebar's "Search chats" button opens it on the Chats tab.
- With an empty query, All shows up to 8 recent chats plus actions; typing searches titles, projects, and (after 180 ms) chat bodies through `searchChats`, showing an excerpt and jumping to the matching message. Chats cap at 12 when searching, 24 on the Chats tab; slash commands (up to 16) appear only when searching or on Actions; Settings items appear on All only while searching.
- Palette keys: Up/Down move (wrapping), Enter runs, Escape closes, Left or Shift+Right changes the filter tab.
- Choosing a command: `btw` opens a side conversation, `model` opens the model picker, anything else puts `/<name> ` into the composer.
- Shortcuts are rebindable in Customize > Shortcuts. Enter, Tab, Space, Backspace, and Escape cannot be bound on their own, and Escape cannot be bound at all. Clashes are shown, not blocked. Only overrides are stored (`localStorage` `zest.keybindings`), so a changed default reaches users who never touched that binding. An empty string means deliberately unbound.
- Unmodified chords (like `/`) never fire while typing in an input, textarea, or contenteditable.
- Escape closes the topmost surface in this order: diff, provider switch, model picker, settings, palette, message edit, shell panel (Customize/Profile/Usage/PRs); only when none is open does it stop a running turn.
- The native right-click menu appears only in inputs, selects, textareas, and contenteditable regions (a `contenteditable="false"` ancestor turns it off).

## How it works
- Registry: `COMMANDS` in `keybindings.ts` (`CommandId`, `defaultChord`); `chordFromEvent`/`normalizeChord` produce canonical strings (`Mod`, `Alt`, `Shift`, upper-case key); `commandFor` returns the first match in registry order; `loadBindings`/`saveBindings`/`conflicts`/`chordIsBindable` back the editor.
- Dispatch: `useKeybindings` (`useKeybindings.ts`) adds one window `keydown` listener; `ChatScreen.tsx` passes handlers for every `CommandId`. `useBindings` broadcasts `zest:keybindings-changed` and listens to `storage` so other windows stay in sync.
- Editor: `KeyboardShortcuts.tsx` records with a capture-phase listener and `stopPropagation`, so the chord being assigned does not also run.
- Escape: `ChatScreen.tsx` has its own `keydown` listener that asks `escapeAction` (`escapeStack.ts`) which surface to dismiss.
- Palette: `ChatScreen` builds `paletteActions` (go back/forward, new chat, workbench, switch provider, Customize, settings, profile, usage) with shortcut labels from `formatChord`. `CommandPalette.tsx` loads `listCommands` and `listChatProjects` when opened, merges `searchChats` hits with `mergeChatHits`, and lays out sections with `buildPaletteSections` (`commandPaletteSearch.ts`).
- Context menu: `main.tsx` blocks `contextmenu` unless `shouldPreserveNativeContextMenu` (`contextMenu.ts`) allows it.
- Focus: `useDialogFocusTrap` keeps Tab inside plain positioned dialogs (Settings, for example) and restores focus to the trigger on close.

## Verify
- `npm run ui:test`: chord normalization, display, bindability, conflicts, dispatch lookup, Escape order, palette sections, and the context-menu guard.
- `npm run ui:e2e -- sidebar-search` (palette opens on Chats with focus) and `-- workbench` (runs "Open workbench" from the palette).
- Fixture: `/?fixture=1`, press Ctrl+K, type "Turn 3 prompt" to find "Fifteen turns" by body text with an excerpt.

## Pitfalls
- Escape is deliberately not in the registry. A new overlay needs a flag in `escapeAction` placed above `stop-turn`; otherwise Escape closes it and also cancels the turn (a past regression with shell panels).
- Mod+Shift+C was avoided for Customize because Chromium reserves it for the inspector.
- Left arrow in the palette input changes the filter tab rather than moving the caret.
- `useDialogFocusTrap` has no unit test; check it by hand when changing dialog markup.
