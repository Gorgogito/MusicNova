// Comprueba los catálogos de traducción.
//  - Interfaz: frases marcadas con tr('…') y plural(n, '…', '…') → src/i18n/en.ts
//  - Backend: mensajes en español de Rust (crates/, app/src-tauri/) y del proceso
//    de IA (ai-worker/) que llegan a la interfaz → src/i18n/backend.ts. Los huecos
//    de format!/f-strings se normalizan a {1}, {2}…
// Uso: node scripts/i18n-check.mjs [--list | --list-backend]
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const app = join(dirname(fileURLToPath(import.meta.url)), '..')
const repo = join(app, '..')
const walk = (d, re, out = []) => {
  if (!existsSync(d)) return out
  for (const f of readdirSync(d)) {
    const p = join(d, f)
    if (statSync(p).isDirectory()) {
      if (!['node_modules', 'target', 'vendor', '__pycache__', '.venv', 'tests', 'eval', 'mn-testplugin', 'examples', 'bin'].includes(f)) walk(p, re, out)
    } else if (re.test(f)) out.push(p)
  }
  return out
}

const STR = String.raw`'((?:\\.|[^'\\])*)'|"((?:\\.|[^"\\])*)"`
const unq = (a, b) => (a ?? b).replace(/\\(['"\\n])/g, (_, c) => (c === 'n' ? '\n' : c))

// ---- Interfaz ---------------------------------------------------------------------
function uiIds() {
  const ids = new Map()
  for (const f of walk(join(app, 'src'), /\.(tsx?)$/)) {
    if (f.includes(join('src', 'i18n'))) continue
    const src = readFileSync(f, 'utf8')
    for (const m of src.matchAll(new RegExp(String.raw`\btr\(\s*(?:${STR})`, 'g'))) ids.set(unq(m[1], m[2]), f)
    for (const m of src.matchAll(new RegExp(String.raw`\bplural\([^,]+,\s*(?:${STR})\s*,\s*(?:${STR})`, 'g'))) {
      ids.set(unq(m[1], m[2]), f)
      ids.set(unq(m[3], m[4]), f)
    }
  }
  return ids
}

// ---- Backend ----------------------------------------------------------------------
const SPANISH = /[áéíóúñ¿¡]|\b(el|la|los|las|de|del|un|una|para|con|sin|por|que|no|se|al|en|ya|más|esta|este|hay|pista|archivo|proyecto|modelo|falló|listo)\b/i

/** Huecos de formato → {1}, {2}… ({{ y }} son llaves literales). */
function normalize(s, py) {
  let n = 0
  const OPEN = ''
  const CLOSE = ''
  return s
    .replaceAll('{{', OPEN)
    .replaceAll('}}', CLOSE)
    .replace(/\{[^{}]*\}/g, () => `{${++n}}`)
    .replace(py ? /%[sdr]|%\.\d+f/g : /$^/g, () => `{${++n}}`)
    .replaceAll(OPEN, '{')
    .replaceAll(CLOSE, '}')
}

function backendIds() {
  const ids = new Map()
  const rust = [...walk(join(repo, 'crates'), /\.rs$/), ...walk(join(app, 'src-tauri', 'src'), /\.rs$/)]
  const py = walk(join(repo, 'ai-worker', 'musicnova_ai'), /\.py$/)
  const toml = walk(join(repo, 'ai-worker', 'musicnova_ai', 'manifests'), /\.toml$/)
  for (const f of [...rust, ...py]) {
    const isPy = f.endsWith('.py')
    let src = readFileSync(f, 'utf8')
    // Fuera de las pruebas.
    const cut = isPy ? -1 : src.search(/#\[cfg\(test\)\]\s*mod tests/)
    if (cut >= 0) src = src.slice(0, cut)
    const lines = src.split('\n')
    let inDoc = false
    for (const line of lines) {
      const st = line.trim()
      if (isPy) {
        const q = (st.match(/"""|'''/g) || []).length
        if (inDoc || st.startsWith('"""') || st.startsWith("'''")) {
          if (q % 2 === 1) inDoc = !inDoc
          continue
        }
        if (st.startsWith('#')) continue
      } else if (st.startsWith('//') || st.startsWith('#[') || /eprintln!|println!|debug_assert|assert(_eq)?!|panic!|unreachable!|\.expect\(/.test(st)) {
        if (!st.startsWith('#[error(')) continue
      }
      if (/\blog\.|logging\.|print\(/.test(st)) continue
      const re = isPy ? /(?<![\w'"])(f?)(['"])((?:\\.|(?!\2).)*)\2/g : /"((?:\\.|[^"\\])*)"/g
      for (const m of line.matchAll(re)) {
        const raw = isPy ? m[3] : m[1]
        const fmt = isPy ? m[1] === 'f' : true
        const s = raw.replace(/\\"/g, '"').replace(/\\n/g, '\n')
        if (!(SPANISH.test(s) || /\b[a-záéíóúñ]+(ando|endo)\b|^listo$|^instalado$/i.test(s)) || !/[a-záéíóúñ]{3,}/i.test(s)) continue
        // Palabras sueltas: solo gerundios y participios («transcribiendo», «listo»), no identificadores.
        if (/^[\w./:-]+$/.test(s) && !/^[a-záéíóúñ]+(ando|endo|ado|ido|isto)$/i.test(s)) continue
        ids.set(fmt ? normalize(s, isPy) : s, f)
      }
    }
  }
  // Manifiestos de modelos: nombre y descripción.
  for (const f of toml) {
    const src = readFileSync(f, 'utf8').split(/\n\[\[/)[0]
    for (const m of src.matchAll(/^(name|description|license_note|notes)\s*=\s*"((?:\\.|[^"\\])*)"/gm)) {
      // Nombres visibles (los que llevan « · ») y descripciones, aunque no parezcan español.
      if (m[1] === 'description' || m[2].includes(' · ') || SPANISH.test(m[2])) ids.set(m[2].replace(/\\"/g, '"'), f)
    }
  }
  return ids
}

function catalogKeys(file) {
  const p = join(app, 'src', 'i18n', file)
  const keys = new Set()
  if (!existsSync(p)) return keys
  for (const m of readFileSync(p, 'utf8').matchAll(new RegExp(String.raw`^\s*(?:${STR})\s*:`, 'gm'))) keys.add(unq(m[1], m[2]))
  return keys
}

const report = (label, ids, keys) => {
  const missing = [...ids.keys()].filter((k) => !keys.has(k))
  const unused = [...keys].filter((k) => !ids.has(k))
  console.log(`${label}: ${ids.size} frases · ${keys.size} en el catálogo`)
  for (const k of missing) console.log(`  falta: ${JSON.stringify(k)}   (${ids.get(k).split(/[\\/]/).pop()})`)
  for (const k of unused) console.log(`  sobra: ${JSON.stringify(k)}`)
  return missing.length
}

if (process.argv.includes('--list')) {
  const keys = catalogKeys('en.ts')
  console.log(JSON.stringify([...uiIds().keys()].filter((k) => !keys.has(k)), null, 1))
} else if (process.argv.includes('--list-backend')) {
  const keys = catalogKeys('backend.ts')
  console.log(JSON.stringify([...backendIds().keys()].filter((k) => !keys.has(k)), null, 1))
} else {
  const a = report('Interfaz', uiIds(), catalogKeys('en.ts'))
  const b = report('Backend', backendIds(), catalogKeys('backend.ts'))
  if (a + b) process.exit(1)
}
