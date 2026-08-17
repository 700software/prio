#!/usr/bin/env node
import { execFileSync } from 'child_process'
import { existsSync } from 'fs'
import { fileURLToPath } from 'url'
import path from 'path'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const root = path.join(__dirname, '..')

const map = {
  'win32-x64': 'prio-x86_64-pc-windows-msvc.exe',
  'darwin-x64': 'prio-x86_64-apple-darwin',
  'darwin-arm64': 'prio-aarch64-apple-darwin',
  'linux-x64': 'prio-x86_64-unknown-linux-gnu',
}

const key = `${process.platform}-${process.arch}`
const name = map[key]
if (!name) {
  console.error(`Unsupported platform: ${key}`)
  process.exit(1)
}

const binPath = path.join(root, 'dist-bin', name)
if (!existsSync(binPath)) {
  console.error(
    `prio binary not found at dist-bin/${name}.\n` +
      'From this repo, build it with:\n' +
      '  npm run build:binary\n' +
      '  npm run prepublishOnly\n',
  )
  process.exit(1)
}

try {
  execFileSync(binPath, process.argv.slice(2), { stdio: 'inherit' })
} catch (e) {
  process.exit(e.status || 1)
}
