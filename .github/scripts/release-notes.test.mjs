import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { releaseNotes } from "./release-notes.mjs";

async function withNotes(content) {
  const root = await mkdtemp(join(tmpdir(), "release-notes-"));
  await mkdir(join(root, "docs/releases"), { recursive: true });
  if (content !== undefined) await writeFile(join(root, "docs/releases/v1.2.3.md"), content);
  return root;
}

test("publishes the changes written for the version", async () => {
  const root = await withNotes("## 主要更新\n\n- 修复压缩\n");
  const notes = await releaseNotes({ root, version: "1.2.3" });
  assert.match(notes, /^## 主要更新\n\n- 修复压缩\n/);
  assert.match(notes, /Download the installer/);
  assert.doesNotMatch(notes, /Beta release/);
});

test("a version without written changes cannot be released", async () => {
  for (const content of [undefined, "", "## 主要更新\n\nTBD\n"]) {
    const root = await withNotes(content);
    await assert.rejects(releaseNotes({ root, version: "1.2.3" }), /docs\/releases\/v1\.2\.3\.md/);
  }
});

test("a beta says so", async () => {
  const root = await withNotes();
  await writeFile(join(root, "docs/releases/v1.2.3-beta.1.md"), "- 试用\n");
  assert.match(await releaseNotes({ root, version: "1.2.3-beta.1" }), /Beta release/);
});

test("the version cannot leave the notes directory", async () => {
  const root = await withNotes("- x\n");
  await assert.rejects(releaseNotes({ root, version: "../v1.2.3" }), /semantic version/);
});

test("the desktop version being prepared has its notes", async () => {
  const root = new URL("../..", import.meta.url).pathname;
  const { version } = JSON.parse(await readFile(join(root, "apps/desktop/package.json"), "utf8"));
  await releaseNotes({ root, version });
});
