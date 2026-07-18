import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";

const target = process.argv[2];
if (target !== "win") throw new Error(`Unsupported preflight target: ${target}`);
if (process.platform !== "win32" || process.arch !== "x64") {
  throw new Error(`Windows packaging requires win32/x64; current=${process.platform}/${process.arch}`);
}

const require = createRequire(import.meta.url);
try {
  require.resolve("@tauri-apps/cli-win32-x64-msvc");
} catch {
  throw new Error(
    "Missing @tauri-apps/cli-win32-x64-msvc@1.6.3. Run: npm ci --include=optional",
  );
}

const rust = execFileSync("rustc", ["-Vv"], { encoding: "utf8" });
if (!rust.includes("host: x86_64-pc-windows-msvc") || /release: .*-(nightly|beta)/.test(rust)) {
  throw new Error(`Stable x86_64-pc-windows-msvc Rust is required.\n${rust}`);
}

execFileSync("powershell", [
  "-NoProfile",
  "-Command",
  "$keys=@('HKLM:\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F1E7E24E-1E5C-42D1-BA4D-7A7BB9E7A97F}','HKLM:\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F1E7E24E-1E5C-42D1-BA4D-7A7BB9E7A97F}'); if(-not ($keys | Where-Object { Test-Path $_ })){ throw 'WebView2 Runtime not found' }",
], { stdio: "inherit" });

console.log("Windows package preflight passed.");
