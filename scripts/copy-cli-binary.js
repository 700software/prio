import fs from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const root = path.join(__dirname, '..')

const map = {
  'win32-x64': 'prio-x86_64-pc-windows-msvc.exe',
  'darwin-x64': 'prio-x86_64-apple-darwin',
  'darwin-arm64': 'prio-aarch64-apple-darwin',
  'linux-x64': 'prio-x86_64-unknown-linux-gnu',
}

const key = `${process.platform}-${process.arch}`
const destName = map[key]
if (!destName) {
  console.error(`Unsupported platform for CLI copy: ${key}`)
  process.exit(1)
}

const releaseDir = path.join(root, 'src-tauri', 'target', 'release')
const candidates = [path.join(releaseDir, 'prio.exe'), path.join(releaseDir, 'prio')]

const src = candidates.find(p => fs.existsSync(p))
if (!src) {
  console.error('Release binary not found. Run: cd src-tauri && cargo build --release')
  process.exit(1)
}

const destDir = path.join(root, 'dist-bin')
fs.mkdirSync(destDir, { recursive: true })
const dest = path.join(destDir, destName)
fs.copyFileSync(src, dest)
if (process.platform !== 'win32') {
  fs.chmodSync(dest, 0o755)
}
console.log(`Copied ${src} -> ${dest}`)
console.log(`Standalone exe: ${src}`)
console.log(`npx prio uses: ${dest}`)
