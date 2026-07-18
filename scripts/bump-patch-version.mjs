import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dryRun = process.argv.includes("--dry-run");

const paths = {
  packageJson: path.join(root, "package.json"),
  packageLock: path.join(root, "package-lock.json"),
  tauriConfig: path.join(root, "src-tauri", "tauri.conf.json"),
  cargoManifest: path.join(root, "src-tauri", "Cargo.toml"),
  cargoLock: path.join(root, "src-tauri", "Cargo.lock"),
};

const [packageJsonText, packageLockText, tauriConfigText, cargoManifestText, cargoLockText] =
  await Promise.all([
    readFile(paths.packageJson, "utf8"),
    readFile(paths.packageLock, "utf8"),
    readFile(paths.tauriConfig, "utf8"),
    readFile(paths.cargoManifest, "utf8"),
    readFile(paths.cargoLock, "utf8"),
  ]);

const packageJson = JSON.parse(packageJsonText);
const packageLock = JSON.parse(packageLockText);
const tauriConfig = JSON.parse(tauriConfigText);

function packageSectionVersion(text, fileName) {
  const packageStart = text.indexOf("[package]");
  const packageEnd = text.indexOf("\n[", packageStart + "[package]".length);
  if (packageStart < 0 || packageEnd < 0) {
    throw new Error(`${fileName}: missing [package] section`);
  }

  const match = text.slice(packageStart, packageEnd).match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error(`${fileName}: missing package version`);
  }
  return match[1];
}

function cargoLockPackageVersion(text) {
  const match = text.match(/\[\[package\]\]\r?\nname = "lanbridge"\r?\nversion = "([^"]+)"/);
  if (!match) {
    throw new Error("src-tauri/Cargo.lock: missing lanbridge package version");
  }
  return match[1];
}

const currentVersion = packageJson.version;
const versions = new Map([
  ["package.json", currentVersion],
  ["package-lock.json", packageLock.version],
  ["package-lock.json root package", packageLock.packages?.[""]?.version],
  ["src-tauri/tauri.conf.json", tauriConfig.package?.version],
  ["src-tauri/Cargo.toml", packageSectionVersion(cargoManifestText, "src-tauri/Cargo.toml")],
  ["src-tauri/Cargo.lock", cargoLockPackageVersion(cargoLockText)],
]);

for (const [source, version] of versions) {
  if (version !== currentVersion) {
    throw new Error(`${source}: expected version ${currentVersion}, found ${version ?? "missing"}`);
  }
}

const semver = /^(\d+)\.(\d+)\.(\d+)$/;
const match = currentVersion.match(semver);
if (!match) {
  throw new Error(`package.json: ${currentVersion} is not a stable semantic version`);
}

const nextVersion = `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
console.log(`LanBridge patch version: ${currentVersion} -> ${nextVersion}${dryRun ? " (dry run)" : ""}`);

if (dryRun) {
  process.exit(0);
}

packageJson.version = nextVersion;
packageLock.version = nextVersion;
packageLock.packages[""].version = nextVersion;
tauriConfig.package.version = nextVersion;

const nextCargoManifest = cargoManifestText.replace(
  /(^\[package\][\s\S]*?^version\s*=\s*")[^"]+(".*$)/m,
  `$1${nextVersion}$2`,
);
const nextCargoLock = cargoLockText.replace(
  /(\[\[package\]\]\r?\nname = "lanbridge"\r?\nversion = ")[^"]+(")/,
  `$1${nextVersion}$2`,
);

await Promise.all([
  writeFile(paths.packageJson, `${JSON.stringify(packageJson, null, 2)}\n`),
  writeFile(paths.packageLock, `${JSON.stringify(packageLock, null, 2)}\n`),
  writeFile(paths.tauriConfig, `${JSON.stringify(tauriConfig, null, 2)}\n`),
  writeFile(paths.cargoManifest, nextCargoManifest),
  writeFile(paths.cargoLock, nextCargoLock),
]);

console.log(`LanBridge version files updated to ${nextVersion}.`);
