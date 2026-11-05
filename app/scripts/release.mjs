// Prepara (y con --publish, publica) una versión en GitHub Releases.
//
//   1. node scripts/set-version.mjs X.Y.Z
//   2. compilar con la clave de firma:
//        set TAURI_SIGNING_PRIVATE_KEY=%USERPROFILE%\.tauri\musicnova.key
//        set TAURI_SIGNING_PRIVATE_KEY_PASSWORD=
//        npx tauri build
//   3. node scripts/release.mjs --notes notas.md            (genera latest.json y lo comprueba)
//      node scripts/release.mjs --notes notas.md --publish  (crea la release vX.Y.Z con gh)
//
// latest.json es lo que consulta la app (…/releases/latest/download/latest.json):
// versión, notas, fecha, y para Windows la URL del instalador y su firma.
// --base-url URL genera un latest.json que apunta a otro sitio (pruebas locales).
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const REPO = 'Gorgogito/MusicNova'
const app = join(dirname(fileURLToPath(import.meta.url)), '..')
const arg = (name) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : undefined
}
const conf = JSON.parse(readFileSync(join(app, 'src-tauri', 'tauri.conf.json'), 'utf8'))
const version = conf.version
const tag = `v${version}`
const dir = join(app, '..', 'target', 'release', 'bundle', 'nsis')
const installer = `MusicNova_${version}_x64-setup.exe`
const exe = join(dir, installer)
const sig = `${exe}.sig`
for (const f of [exe, sig]) {
  if (!existsSync(f)) {
    console.error(`falta ${f}: compila con la clave de firma (TAURI_SIGNING_PRIVATE_KEY) y la versión ${version}`)
    process.exit(1)
  }
}
const notesFile = arg('--notes')
const notes = notesFile ? readFileSync(notesFile, 'utf8').trim() : `MusicNova ${version}`
const base = arg('--base-url') ?? `https://github.com/${REPO}/releases/download/${tag}/`
const latest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    'windows-x86_64': {
      signature: readFileSync(sig, 'utf8').trim(),
      url: base.replace(/\/?$/, '/') + installer,
    },
  },
}
const latestPath = join(dir, 'latest.json')
writeFileSync(latestPath, JSON.stringify(latest, null, 2) + '\n')
console.log(`latest.json → ${latest.platforms['windows-x86_64'].url}`)

if (process.argv.includes('--publish')) {
  const args = ['release', 'create', tag, exe, sig, latestPath, '--repo', REPO, '--title', `MusicNova ${version}`]
  args.push(...(notesFile ? ['--notes-file', notesFile] : ['--notes', notes]))
  execFileSync('gh', args, { stdio: 'inherit' })
  console.log(`Publicada ${tag}: https://github.com/${REPO}/releases/tag/${tag}`)
} else {
  console.log('Sin --publish: no se ha subido nada.')
}
