import fs from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const root = path.join(__dirname, '..')
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'))
const version = pkg.version
const cargoPath = path.join(root, 'src-tauri', 'Cargo.toml')
let cargo = fs.readFileSync(cargoPath, 'utf8')
cargo = cargo.replace(/^version = ".*"$/m, `version = "${version}"`)
fs.writeFileSync(cargoPath, cargo)
console.log(`Synced version ${version} to src-tauri/Cargo.toml`)
