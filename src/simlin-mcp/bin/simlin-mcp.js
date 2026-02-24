#!/usr/bin/env node
// Entry point for the @simlin/mcp npm package.
//
// Detects the current platform, resolves the platform-specific native binary
// from the matching optional dependency, and spawns it with stdio forwarding.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

// Map from (platform, arch) to npm package name and Rust target triple.
const PLATFORM_MAP = {
  "darwin-arm64": {
    package: "@simlin/mcp-darwin-arm64",
    triple: "aarch64-apple-darwin",
  },
  "darwin-x64": {
    package: "@simlin/mcp-darwin-x64",
    triple: "x86_64-apple-darwin",
  },
  "linux-x64": {
    package: "@simlin/mcp-linux-x64",
    triple: "x86_64-unknown-linux-musl",
  },
  "win32-x64": {
    package: "@simlin/mcp-win32-x64",
    triple: "x86_64-pc-windows-msvc",
  },
};

const platformKey = `${process.platform}-${process.arch}`;
const platformInfo = PLATFORM_MAP[platformKey];

if (!platformInfo) {
  console.error(
    `simlin-mcp: unsupported platform: ${process.platform} (${process.arch})`,
  );
  console.error(
    "Supported platforms: darwin-arm64, darwin-x64, linux-x64, win32-x64",
  );
  process.exit(1);
}

const binaryName =
  process.platform === "win32" ? "simlin-mcp.exe" : "simlin-mcp";

// When installed from npm, the binary lives inside the platform package.
// In development (cargo build), it lives in vendor/<triple>/.
const vendorBinaryPath = path.join(
  __dirname,
  "..",
  "vendor",
  platformInfo.triple,
  binaryName,
);

let binaryPath = null;

try {
  const pkgJsonPath = require.resolve(
    `${platformInfo.package}/package.json`,
  );
  const pkgDir = path.dirname(pkgJsonPath);
  const candidate = path.join(pkgDir, "bin", binaryName);
  if (existsSync(candidate)) {
    binaryPath = candidate;
  }
} catch {
  // Optional dependency not installed -- fall through to dev vendor path.
}

if (!binaryPath) {
  if (existsSync(vendorBinaryPath)) {
    binaryPath = vendorBinaryPath;
  } else {
    console.error(
      `simlin-mcp: could not find native binary for ${platformKey}`,
    );
    console.error(
      `Install the platform package: npm install ${platformInfo.package}`,
    );
    console.error(
      `Or for development, build with: cargo build -p simlin-mcp && ` +
        `mkdir -p vendor/${platformInfo.triple} && ` +
        `cp target/debug/simlin-mcp vendor/${platformInfo.triple}/simlin-mcp`,
    );
    process.exit(1);
  }
}

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
});

child.on("error", (err) => {
  console.error(`simlin-mcp: failed to start binary: ${err.message}`);
  process.exit(1);
});

const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    // ignore errors forwarding signals
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((sig) => {
  process.on(sig, () => forwardSignal(sig));
});

// Mirror child exit status so that callers observe the correct exit code
// or signal-based termination.
const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}
