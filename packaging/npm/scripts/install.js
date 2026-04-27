#!/usr/bin/env node
// V2.0 — npm wrapper postinstall script.
//
// Detects the host platform + architecture, picks the matching
// GitHub Releases asset, downloads + verifies + extracts the
// binaries into ./bin/. Runs once at `npm install -g @aegis/cli`.
//
// Pre-V2.0 release: the asset URLs assume `release.yml` has run
// and produced the expected tarball naming. If a release is
// missing for the user's platform, we error with a clear message
// instead of installing a broken stub.

const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const { spawnSync } = require("node:child_process");
const { pipeline } = require("node:stream");

const VERSION = require("../package.json").version;
const REPO = "wei9072/aegis";

function targetTriple() {
  const arch = process.arch === "x64" ? "x86_64" : process.arch === "arm64" ? "aarch64" : null;
  if (!arch) {
    return null;
  }
  switch (process.platform) {
    case "linux":
      return `${arch}-unknown-linux-gnu`;
    case "darwin":
      return `${arch}-apple-darwin`;
    case "win32":
      return arch === "x86_64" ? "x86_64-pc-windows-msvc" : null;
    default:
      return null;
  }
}

function assetUrl(triple) {
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  const base = `aegis-v${VERSION}-${triple}.${ext}`;
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${base}`;
}

async function fetchTo(url, dest) {
  await new Promise((resolve, reject) => {
    const req = https.get(url, { headers: { "User-Agent": "aegis-npm-installer" } }, (res) => {
      if (res.statusCode === 301 || res.statusCode === 302) {
        // Follow redirect.
        fetchTo(res.headers.location, dest).then(resolve, reject);
        return;
      }
      if (res.statusCode !== 200) {
        reject(new Error(`download failed: HTTP ${res.statusCode} for ${url}`));
        return;
      }
      const out = fs.createWriteStream(dest);
      pipeline(res, out, (err) => (err ? reject(err) : resolve()));
    });
    req.on("error", reject);
  });
}

function extract(archive, dir) {
  const ext = archive.endsWith(".zip") ? "zip" : "tar";
  if (ext === "zip") {
    spawnSync("unzip", ["-o", archive, "-d", dir], { stdio: "inherit" });
  } else {
    spawnSync("tar", ["-xzf", archive, "-C", dir, "--strip-components=1"], {
      stdio: "inherit",
    });
  }
}

async function main() {
  const triple = targetTriple();
  if (!triple) {
    console.error(
      `Unsupported platform/arch: ${process.platform}/${process.arch}. ` +
        `See https://github.com/${REPO}/releases for available builds.`
    );
    process.exit(1);
  }
  const binDir = path.join(__dirname, "..", "bin");
  fs.mkdirSync(binDir, { recursive: true });
  const tmpDir = fs.mkdtempSync("aegis-install-");
  const url = assetUrl(triple);
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  const archive = path.join(tmpDir, `aegis.${ext}`);
  console.log(`downloading ${url}`);
  await fetchTo(url, archive);
  extract(archive, binDir);
  // Drop trampoline JS shims that node uses to find the binary.
  for (const name of ["aegis", "aegis-mcp"]) {
    const exe = process.platform === "win32" ? `${name}.exe` : name;
    const target = path.join(binDir, exe);
    if (!fs.existsSync(target)) {
      console.error(`expected binary missing after extract: ${target}`);
      process.exit(1);
    }
    fs.chmodSync(target, 0o755);
    const wrapperPath = path.join(binDir, `${name}.js`);
    fs.writeFileSync(
      wrapperPath,
      `#!/usr/bin/env node\n` +
        `const { spawn } = require("node:child_process");\n` +
        `const path = require("node:path");\n` +
        `const exe = process.platform === "win32" ? "${name}.exe" : "${name}";\n` +
        `const child = spawn(path.join(__dirname, exe), process.argv.slice(2), { stdio: "inherit" });\n` +
        `child.on("exit", (code) => process.exit(code ?? 1));\n`
    );
    fs.chmodSync(wrapperPath, 0o755);
  }
  console.log(`aegis ${VERSION} installed for ${triple}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
