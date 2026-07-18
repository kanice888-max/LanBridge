import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cp, mkdir, readdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const target = process.argv[2];
const skipVersionBump = process.argv.includes("--no-bump");
const expectedPlatform = target === "mac" ? "darwin" : target === "win" ? "win32" : null;
if (!expectedPlatform) throw new Error(`Unsupported package target: ${target}`);
if (process.platform !== expectedPlatform) {
  throw new Error(`${target} packages must be built on ${expectedPlatform}; current=${process.platform}`);
}
if (target === "win") {
  execFileSync(process.execPath, [path.join(root, "scripts", "preflight-package.mjs"), "win"], {
    cwd: root,
    stdio: "inherit",
  });
}
if (target === "mac") {
  const mountedImages = execFileSync("hdiutil", ["info"], { encoding: "utf8" });
  if (mountedImages.includes("/Volumes/LanBridge")) {
    throw new Error("A previous LanBridge DMG is still mounted. Eject it before packaging.");
  }
}

const versionFiles = [
  "package.json",
  "package-lock.json",
  "src-tauri/tauri.conf.json",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
];
const originals = new Map();
for (const relative of versionFiles) originals.set(relative, await readFile(path.join(root, relative), "utf8"));

let packageValidated = false;
async function restoreVersionFiles() {
  await Promise.all([...originals].map(([relative, content]) => writeFile(path.join(root, relative), content)));
}
process.on("uncaughtException", async (error) => {
  if (!packageValidated) await restoreVersionFiles();
  console.error(error);
  process.exit(1);
});
process.on("unhandledRejection", async (error) => {
  if (!packageValidated) await restoreVersionFiles();
  console.error(error);
  process.exit(1);
});

let buildSucceeded = false;
try {
  if (skipVersionBump) {
    console.log("Packaging current LanBridge version without incrementing it.");
  } else {
    execFileSync(process.execPath, [path.join(root, "scripts", "bump-patch-version.mjs")], {
      cwd: root,
      stdio: "inherit",
    });
  }
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const args = ["run", "tauri", "--", "build"];
  if (target === "mac") args.push("--config", "src-tauri/tauri.macos.conf.json", "--bundles", "app");
  const result = spawnSync(npm, args, { cwd: root, stdio: "inherit", env: process.env });
  if (result.status !== 0) throw new Error(`Tauri build failed with exit code ${result.status}`);
  buildSucceeded = true;
} finally {
  if (!buildSucceeded) {
    await restoreVersionFiles();
    console.error("Build failed; version files were restored.");
  }
}

const packageJson = JSON.parse(await readFile(path.join(root, "package.json"), "utf8"));
const bundleRoot = path.join(root, "src-tauri", "target", "release", "bundle");
if (target === "mac") {
  const appPath = path.join(bundleRoot, "macos", "LanBridge.app");
  execFileSync("codesign", ["--verify", "--deep", "--strict", "--verbose=2", appPath], { stdio: "inherit" });
  const signatureResult = spawnSync("codesign", ["-dvv", appPath], { encoding: "utf8" });
  const identity = `${signatureResult.stdout || ""}${signatureResult.stderr || ""}`;
  if (!identity.includes("Signature=adhoc") || !identity.includes("Identifier=com.lanbridge.app")) {
    throw new Error(`Unexpected macOS signature:\n${identity}`);
  }
  const dmgDirectory = path.join(bundleRoot, "dmg");
  const staging = path.join(dmgDirectory, `.LanBridge-${packageJson.version}-staging`);
  const dmgPath = path.join(dmgDirectory, `LanBridge_${packageJson.version}_x64.dmg`);
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });
  await cp(appPath, path.join(staging, "LanBridge.app"), { recursive: true });
  await symlink("/Applications", path.join(staging, "Applications"));
  try {
    execFileSync("hdiutil", [
      "create", "-volname", "LanBridge", "-srcfolder", staging,
      "-ov", "-format", "UDZO", dmgPath,
    ], { stdio: "inherit" });
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

async function collectFiles(directory) {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) result.push(...await collectFiles(entryPath));
    else result.push(entryPath);
  }
  return result;
}

const installers = (await collectFiles(bundleRoot)).filter((file) =>
  file.includes(packageJson.version)
    && (target === "mac" ? file.endsWith(".dmg") : /\.(msi|exe)$/i.test(file)),
);
if (installers.length === 0) throw new Error(`No ${target} installer artifact found for ${packageJson.version}`);
for (const installer of installers) {
  const digest = createHash("sha256").update(await readFile(installer)).digest("hex");
  await writeFile(`${installer}.sha256`, `${digest}  ${path.basename(installer)}\n`);
  console.log(`Verified ${installer}\nSHA-256 ${digest}`);
}
packageValidated = true;
