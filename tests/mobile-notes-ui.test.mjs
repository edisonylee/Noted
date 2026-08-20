import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(relativePath) {
  return readFile(new URL(relativePath, root), "utf8");
}

test("the iPhone Notes UI centralizes its typed Tauri command contract", async () => {
  const shell = await read("src/MobileShell.tsx");

  for (const command of [
    "get_mobile_notes_workspace",
    "list_mobile_notes",
    "create_mobile_note",
    "update_mobile_note",
    "file_mobile_note",
    "undo_mobile_note_filing",
    "trash_mobile_note",
    "delete_mobile_note",
    "restore_mobile_note",
    "resolve_mobile_note_conflict",
    "mobile_sync_now",
  ]) {
    assert.match(shell, new RegExp(`\\b${command}\\b`), `${command} is missing from the client seam`);
  }

  assert.match(shell, /createMobileNotesClient\(\(command, args\) => invoke\(command, args\)\)/);
  assert.match(shell, /isMissingCommand\(reason, MOBILE_NOTES_COMMANDS\.workspace\)/);
  assert.doesNotMatch(shell, /invoke\(["'](?:get|list|create|update|file|undo|trash|delete|restore|resolve)_mobile/);
});

test("paired notebooks expose Bonjour sync and strict manual-address fallback", async () => {
  const shell = await read("src/MobileShell.tsx");

  assert.match(shell, /workspace\.sync\.state !== "local" && workspace\.sync\.state !== "not_enrolled"/);
  assert.match(shell, /Sync now/);
  assert.match(shell, /Connect by address/);
  assert.match(shell, /client\.sync\(manualAddress\)/);
  assert.match(shell, /manualAddress: manualAddress \?\? null/);
  assert.match(shell, /Your saved pairing still verifies the Mac\./);
});

test("future Notes actions are capability-gated and legacy trash remains recoverable", async () => {
  const shell = await read("src/MobileShell.tsx");

  assert.match(shell, /workspace\.capabilities\.filing/);
  assert.match(shell, /workspace\.capabilities\.undoFiling/);
  assert.match(shell, /workspace\.capabilities\.restore/);
  assert.match(shell, /workspace\.capabilities\.conflictResolution/);
  assert.match(shell, /workspace\.capabilities\.trash \|\| workspace\.capabilities\.legacyTrash/);
  assert.match(shell, /hasOpenConflict && !readOnly && workspace\.capabilities\.conflictResolution/);
  assert.match(shell, /draft\.readOnly/);
  assert.match(shell, /readOnly=\{trashed \|\| readOnly \|\| hasOpenConflict\}/);
  assert.match(shell, /if \(!draft \|\| draft\.lifecycleState === "trashed" \|\| draft\.readOnly \|\| draft\.hasOpenConflict\) return/);
  assert.match(shell, /disabled=\{busy \|\| dirty\} title=\{dirty \? "Save this note before moving it to Trash"/);
  assert.match(shell, /Save or discard your changes before moving this note to Trash\./);
  assert.match(shell, /Move .* to Trash\? You can restore it later\./);
  assert.match(shell, /type ConflictResolution = "keepAsCopy" \| "useRemote"/);
  assert.match(shell, /This is a retained conflict copy\./);
  assert.match(shell, /working branch will leave the note list, but it remains retained in conflict history and evidence\./);
});

test("the mobile information architecture and accessibility hooks stay explicit", async () => {
  const shell = await read("src/MobileShell.tsx");
  const styles = await read("src/MobileShell.css");

  for (const label of ["Inbox", "Needs filing", "Spaces", "Trash", "Search", "Conflict copy", "Undo filing", "Restore"]) {
    assert.ok(shell.includes(label), `${label} is missing from the Notes surface`);
  }

  for (const identifier of [
    "mobile-notes-screen",
    "mobile-notes-library-button",
    "mobile-notes-compose-button",
    "mobile-notes-search",
    "mobile-note-editor",
    "mobile-note-title",
    "mobile-note-body",
    "mobile-note-save",
    "mobile-note-file-button",
    "mobile-note-trash",
    "mobile-note-restore",
    "mobile-note-folder-sheet",
  ]) {
    assert.ok(shell.includes(identifier), `${identifier} accessibility identifier is missing`);
  }

  assert.match(shell, /aria-current=/);
  assert.match(shell, /aria-live="polite"/);
  assert.match(shell, /aria-modal="true"/);
  assert.match(shell, /function useModalFocus/);
  assert.match(shell, /document\.getElementById\(returnFocusId\)/);
  assert.match(shell, /event\.key !== "Tab"/);
  assert.match(styles, /env\(safe-area-inset-top\)/);
  assert.match(styles, /env\(safe-area-inset-bottom\)/);
  assert.match(styles, /env\(safe-area-inset-left\)/);
  assert.match(styles, /env\(safe-area-inset-right\)/);
  assert.match(styles, /@media \(prefers-color-scheme: dark\)/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
  assert.doesNotMatch(shell, /<label[^>]*className="search-field"/);
  assert.doesNotMatch(styles, /opacity:\s*0[^.\d]/, "content must not start hidden behind animation");
});

test("note selection has one record-ID path ready for deep-link integration", async () => {
  const shell = await read("src/MobileShell.tsx");

  assert.match(shell, /function openNoteByRecordId\(recordId: string\)/);
  assert.match(shell, /view: "all"/);
  assert.match(shell, /data-record-id=\{note\.recordId\}/);
  assert.match(shell, /onClick=\{\(\) => openNoteByRecordId\(note\.recordId\)\}/);
});

test("cold-launch deep links wait for the shell and protect dirty drafts", async () => {
  const shell = await read("src/MobileShell.tsx");
  const deepLinks = await read("src/mobileDeepLinks.ts");

  assert.match(shell, /window\.addEventListener\(MOBILE_OPEN_NOTE_EVENT, openFromDeepLink\)/);
  assert.match(shell, /connectMobileDeepLinkConsumer\(\)/);
  assert.match(shell, /Discard these changes and open the linked note\?/);
  assert.match(shell, /Finish the current note action before opening that link\./);
  assert.match(deepLinks, /const pendingOpenLinks: ResolvedMobileDeepLink\[\] = \[\]/);
  assert.match(deepLinks, /const pendingErrors: DeepLinkErrorDetail\[\] = \[\]/);
  assert.match(deepLinks, /pendingOpenLinks\.splice\(0\)/);
  assert.match(deepLinks, /if \(consumerReady\) dispatchOpenLink\(detail\);\s*else pendingOpenLinks\.push\(detail\);/);
});

test("unchanged existing notes close without creating sync churn", async () => {
  const shell = await read("src/MobileShell.tsx");

  assert.match(shell, /draft\.recordId !== null && draft\.title === draft\.originalTitle && draft\.body === draft\.originalBody/);
  assert.match(shell, /setDraft\(null\);\s*return;/);
});

test("a fixture client can render the production surface without entering the iOS bundle", async () => {
  const shell = await read("src/MobileShell.tsx");

  assert.match(shell, /export function MobileShell\(\{ client = mobileNotesClient \}/);
  assert.match(shell, /export type MobileNotesClient/);
  assert.doesNotMatch(shell, /fixture|sample note|example note/i);
});
