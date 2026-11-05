// Descarga el Python que va dentro del instalador (python-build-standalone de
// astral-sh: CPython completo con pip y venv, que el «embeddable» oficial no
// trae), verifica su SHA-256 y lo deja en src-tauri/resources/python.
// Uso: node scripts/fetch-python.mjs   (no hace nada si ya está esa versión)
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, renameSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const RELEASE = '20261021'
const VERSION = '3.12.15'
const ASSET = `cpython-${VERSION}+${RELEASE}-x86_64-pc-windows-msvc-install_only_stripped.tar.gz`
const BASE = `https://github.com/astral-sh/python-build-standalone/releases/download/${RELEASE}`

const app = join(dirname(fileURLToPath(import.meta.url)), '..')
const dest = join(app, 'src-tauri', 'resources', 'python')
const marker = join(dest, 'MUSICNOVA_PYTHON_VERSION')
const want = `${VERSION}+${RELEASE}`

if (existsSync(marker) && readFileSync(marker, 'utf8').trim() === want) {
  console.log(`Python ${want} ya está en ${dest}`)
  process.exit(0)
}

async function get(url) {
  const r = await fetch(url, { redirect: 'follow', headers: { 'User-Agent': 'MusicNova-build' } })
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`)
  return Buffer.from(await r.arrayBuffer())
}

console.log(`Descargando ${ASSET}…`)
const sums = (await get(`${BASE}/SHA256SUMS`)).toString('utf8')
const line = sums.split('\n').find((l) => l.trim().endsWith(ASSET))
if (!line) throw new Error(`${ASSET} no figura en SHA256SUMS`)
const expected = line.trim().split(/\s+/)[0].toLowerCase()
const data = await get(`${BASE}/${encodeURIComponent(ASSET)}`)
const actual = createHash('sha256').update(data).digest('hex')
if (actual !== expected) throw new Error(`SHA-256 no coincide: esperado ${expected}, obtenido ${actual}`)
console.log(`SHA-256 correcto (${actual.slice(0, 16)}…), ${(data.length / 1e6).toFixed(1)} MB`)

// En el mismo disco que el destino (para poder moverlo).
const work = join(app, 'src-tauri', 'resources', `.python-tmp-${process.pid}`)
rmSync(work, { recursive: true, force: true })
mkdirSync(work, { recursive: true })
const archive = join(work, ASSET)
writeFileSync(archive, data)
// El bsdtar de Windows 10+ (el tar de Git toma «C:» por un servidor remoto).
const tar = process.platform === 'win32' ? join(process.env.SystemRoot || 'C:/Windows', 'System32', 'tar.exe') : 'tar'
execFileSync(tar, ['-xzf', archive, '-C', work], { stdio: 'inherit' })

// Lo que no necesita el proceso de IA (pruebas, Tk, IDLE): menos tamaño en el instalador.
const root = join(work, 'python')
for (const p of ['Lib/test', 'Lib/idlelib', 'Lib/tkinter', 'Lib/turtledemo', 'tcl', 'DLLs/_tkinter.pyd', 'DLLs/tcl86t.dll', 'DLLs/tk86t.dll']) {
  rmSync(join(root, p), { recursive: true, force: true })
}
writeFileSync(join(root, 'MUSICNOVA_PYTHON_VERSION'), `${want}\n`)

rmSync(dest, { recursive: true, force: true })
mkdirSync(dirname(dest), { recursive: true })
renameSync(root, dest)
rmSync(work, { recursive: true, force: true })
console.log(`Python ${want} listo en ${dest}`)
