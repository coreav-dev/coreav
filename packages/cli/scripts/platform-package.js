"use strict";
// Writes the package.json and README of a native package from the main
// package's metadata: node scripts/platform-package.js PLATFORM DIR.

const fs = require("node:fs");
const path = require("node:path");

const [platform, dir] = process.argv.slice(2);
const main = require("../package.json");
const [os, cpu] = platform.split("-");
const exe = os === "win32" ? "coreav.exe" : "coreav";
const pkg = {
  name: `@coreav/cli-${platform}`,
  version: main.version,
  description: `The coreav command for ${platform}, the native binary of the coreav package`,
  license: main.license,
  repository: main.repository,
  homepage: main.homepage,
  os: [os],
  cpu: [cpu],
  files: [`bin/${exe}`, "README.md", "LICENSE-MIT", "LICENSE-APACHE"],
  preferUnplugged: true,
};
fs.writeFileSync(path.join(dir, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
fs.writeFileSync(
  path.join(dir, "README.md"),
  `# @coreav/cli-${platform}\n\nThe \`coreav\` command for ${platform}. Install \`coreav\` instead: it depends on this package where it applies (\`npx coreav ...\`).\n`,
);
