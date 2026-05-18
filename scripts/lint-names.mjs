import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

const forbidden = [
  ["LAN", "-Folder-Sync"].join(""),
  ["LAN", " Folder Sync"].join(""),
  ["LAN", " Sync"].join(""),
  ["lan", "-folder-sync"].join(""),
  ["lan", "_folder_sync"].join(""),
  ["lan", "foldersync"].join(""),
  ["LAN", "FolderSync"].join(""),
  ["LAN", " folder sync"].join(""),
  ["lan", "-sync"].join(""),
  ["com", ".", "lan", "foldersync"].join(""),
];

const ignoredDirs = new Set([
  ".git",
  "node_modules",
  "dist",
  "target",
]);

const ignoredFiles = new Set([
  ".git",
]);

const root = process.cwd();
const findings = [];

async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (entry.isDirectory() && ignoredDirs.has(entry.name)) {
      continue;
    }

    const fullPath = path.join(dir, entry.name);
    const relativePath = path.relative(root, fullPath).replaceAll("\\", "/");

    if (entry.isDirectory()) {
      await walk(fullPath);
      continue;
    }

    if (!entry.isFile() || ignoredFiles.has(entry.name)) {
      continue;
    }

    for (const term of forbidden) {
      if (relativePath.includes(term)) {
        findings.push(`${relativePath}: path contains "${term}"`);
      }
    }

    let text;
    try {
      text = await readFile(fullPath, "utf8");
    } catch {
      continue;
    }

    const lines = text.split(/\r?\n/);
    lines.forEach((line, index) => {
      for (const term of forbidden) {
        if (line.includes(term)) {
          findings.push(`${relativePath}:${index + 1}: contains "${term}"`);
        }
      }
    });
  }
}

await walk(root);

if (findings.length > 0) {
  console.error("Found legacy project names. Use LanBridge naming instead:");
  for (const finding of findings) {
    console.error(`- ${finding}`);
  }
  process.exit(1);
}

console.log("LanBridge naming check passed.");
