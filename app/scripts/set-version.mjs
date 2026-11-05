// Cambia la versión de MusicNova en todos los sitios a la vez.
// Uso: node scripts/set-version.mjs 0.2.0
import { readFileSync, writeFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const v = process.argv[2]
if (!/^\d+\.\d+\.\d+$/.test(v ?? '')) {
  console.error('uso: node scripts/set-version.mjs X.Y.Z')
  process.exit(2)
}
const app = join(dirname(fileURLToPath(import.meta.url)), '..')
const repo = join(app, '..')
const edit = (file, fn) => {
  const p = join(repo, file)
  const before = readFileSync(p, 'utf8')
  const after = fn(before)
  if (after === before) throw new Error(`${file}: no se encontró la versión`)
  writeFileSync(p, after)
  console.log(`${file} → ${v}`)
}
edit('app/src-tauri/tauri.conf.json', (s) => s.replace(/("version":\s*")[^"]+(")/, `$1${v}$2`))
edit('app/package.json', (s) => s.replace(/("version":\s*")[^"]+(")/, `$1${v}$2`))
// [workspace.package] del Cargo.toml raíz (todas las crates la heredan).
edit('Cargo.toml', (s) => s.replace(/(\[workspace\.package\][^[]*?\nversion\s*=\s*")[^"]+(")/, `$1${v}$2`))
edit('ai-worker/pyproject.toml', (s) => s.replace(/(\nversion\s*=\s*")[^"]+(")/, `$1${v}$2`))
edit('ai-worker/musicnova_ai/__init__.py', (s) => s.replace(/(__version__\s*=\s*")[^"]+(")/, `$1${v}$2`))
