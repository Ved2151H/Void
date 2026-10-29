import { copyFileSync, existsSync, readdirSync, statSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const bundleDir = join(root, "src-tauri", "target", "release", "bundle", "nsis");

if (!existsSync(bundleDir)) {
	console.error("No NSIS bundle yet — run `bun run tauri build` first.");
	process.exit(1);
}

const setup = readdirSync(bundleDir)
	.filter((f) => f.endsWith("_x64-setup.exe"))
	.map((f) => ({ f, mtime: statSync(join(bundleDir, f)).mtimeMs }))
	.sort((a, b) => b.mtime - a.mtime)[0];

if (!setup) {
	console.error("No *_x64-setup.exe found in", bundleDir);
	process.exit(1);
}

for (const f of readdirSync(root).filter((f) => /_x64-setup\.exe$/.test(f))) {
	if (f !== setup.f) rmSync(join(root, f));
}

const dest = join(root, setup.f);
copyFileSync(join(bundleDir, setup.f), dest);
console.log(`Copied ${setup.f} -> ${dest}`);
