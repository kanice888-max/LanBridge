import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const paths = {
  packageJson: path.join(root, "package.json"),
  packageLock: path.join(root, "package-lock.json"),
  tauriConfig: path.join(root, "src-tauri", "tauri.conf.json"),
  cargoManifest: path.join(root, "src-tauri", "Cargo.toml"),
  cargoLock: path.join(root, "src-tauri", "Cargo.lock"),
};

const [packageJsonText, packageLockText, tauriConfigText, cargoManifestText, cargoLockText] =
  await Promise.all(Object.values(paths).map((file) => readFile(file, "utf8")));
const packageJson = JSON.parse(packageJsonText);
const packageLock = JSON.parse(packageLockText);
const tauriConfig = JSON.parse(tauriConfigText);

function cargoPackageVersion(text, fileName) {
  const section = text.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!section) throw new Error(`${fileName}: missing package version`);
  return section[1];
}

function cargoLockPackageVersion(text) {
  const match = text.match(/\[\[package\]\]\r?\nname = "lanbridge"\r?\nversion = "([^"]+)"/);
  if (!match) throw new Error("src-tauri/Cargo.lock: missing lanbridge package version");
  return match[1];
}

const version = packageJson.version;
const versions = new Map([
  ["package-lock.json", packageLock.version],
  ["package-lock.json root package", packageLock.packages?.[""]?.version],
  ["src-tauri/tauri.conf.json", tauriConfig.package?.version],
  ["src-tauri/Cargo.toml", cargoPackageVersion(cargoManifestText, "src-tauri/Cargo.toml")],
  ["src-tauri/Cargo.lock", cargoLockPackageVersion(cargoLockText)],
]);
if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`package.json: ${version} is not stable semver`);
for (const [file, value] of versions) {
  if (value !== version) throw new Error(`${file}: expected version ${version}, found ${value ?? "missing"}`);
}
console.log(`LanBridge version ${version} is consistent.`);
