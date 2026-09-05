import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function source(path) {
  return readFile(new URL(`../${path}`, import.meta.url), "utf8");
}

test("fresh installs do not expose a developer identity or weather city", async () => {
  const [ask, mobile, weather] = await Promise.all([
    source("src/AskView.tsx"),
    source("src/DesktopMatchedMobile.tsx"),
    source("src/Weather.tsx"),
  ]);

  for (const content of [ask, mobile, weather]) {
    assert.doesNotMatch(content, /Hi Edison|Atlanta|San Francisco/);
  }
  assert.match(weather, /Set weather city/);
  assert.match(weather, /Choose your city for local weather/);
});

test("folder initialization creates neutral roots instead of personal projects", async () => {
  const database = await source("src-tauri/src/db.rs");
  const start = database.indexOf("fn seed_note_folders");
  const end = database.indexOf("fn seed_note_folder_structure_v2", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const initialization = database.slice(start, end);

  assert.match(initialization, /'Work'/);
  assert.match(initialization, /'Personal'/);
  assert.doesNotMatch(
    initialization,
    /Baro|Symphony|Side Projects|Career|Health|Finances|Personal Learning/,
  );
});

test("backup destination and repository hooks are user-selected and portable", async () => {
  const [desktop, app, settings, hook, qa] = await Promise.all([
    source("src-tauri/src/desktop.rs"),
    source("src/App.tsx"),
    source("src/Settings.tsx"),
    source(".claude/settings.json"),
    source("design-qa.md"),
  ]);
  const exportStart = desktop.indexOf("async fn export_db");
  const exportEnd = desktop.indexOf("/// LAN URL", exportStart);
  const exportCommand = desktop.slice(exportStart, exportEnd);

  assert.match(app, /await save\(/);
  assert.match(exportCommand, /destination: String/);
  assert.doesNotMatch(exportCommand, /HOME|Desktop/);
  for (const content of [settings, hook, qa]) {
    assert.doesNotMatch(content, /\/Users\/(?:edison|edisonlee)/);
  }
});
