import { readFile } from "node:fs/promises";

const packageJsonUrl = new URL("../package.json", import.meta.url);
const packageJson = JSON.parse(await readFile(packageJsonUrl, "utf8"));
const expectedTag = `v${packageJson.version}`;
const releaseTag = process.env.RELEASE_TAG;

if (releaseTag !== expectedTag) {
  throw new Error(`Release tag ${releaseTag ?? "missing"} does not match application version ${expectedTag}`);
}

console.log(`Release tag ${releaseTag} matches LanBridge ${packageJson.version}.`);
