// Traducción de la interfaz, al estilo gettext: el texto español del código es
// la clave y cada idioma tiene un catálogo que lo traduce. Si una frase no está
// en el catálogo se muestra en español. `npm run i18n:check` lista las que faltan.
//
// El idioma se decide al arrancar (preferencia guardada o idioma del sistema) y
// cambiarlo recarga la ventana: así también se traducen los textos calculados
// al cargar los módulos.
import { backendEn } from './backend'
import { en } from './en'

export type Lang = 'es' | 'en'
export const LANGS: { id: Lang; label: string }[] = [
  { id: 'es', label: 'Español' },
  { id: 'en', label: 'English' },
]

const KEY = 'musicnova.lang'
const CATALOGS: Record<Lang, Record<string, string> | null> = { es: null, en }

function detect(): Lang {
  try {
    const saved = localStorage.getItem(KEY)
    if (saved === 'es' || saved === 'en') return saved
  } catch {
    /* sin almacenamiento local */
  }
  const nav = typeof navigator !== 'undefined' ? navigator.language || '' : ''
  return nav.toLowerCase().startsWith('es') ? 'es' : 'en'
}

let current: Lang = detect()
if (typeof document !== 'undefined') document.documentElement.lang = current

export const lang = (): Lang => current

/** Etiqueta BCP 47 para formatear fechas y números en el idioma actual. */
export const locale = (): string => (current === 'es' ? 'es-ES' : 'en-US')

/** Guarda el idioma y recarga la interfaz. */
export function setLang(l: Lang) {
  try {
    localStorage.setItem(KEY, l)
  } catch {
    /* sin almacenamiento local: dura hasta cerrar la app */
  }
  location.reload()
}

const missing = new Set<string>()

/** Traduce `msg` (texto español) e inserta `{nombre}` con `params`. */
export function tr(msg: string, params?: Record<string, string | number>): string {
  const cat = CATALOGS[current]
  let out = msg
  if (cat) {
    const hit = cat[msg]
    if (hit !== undefined) out = hit
    else if (import.meta.env?.DEV && !missing.has(msg)) {
      missing.add(msg)
      console.warn(`[i18n] sin traducción (${current}): ${msg}`)
    }
  }
  if (params) out = out.replace(/\{(\w+)\}/g, (m, k: string) => (k in params ? String(params[k]) : m))
  return out
}

// ---- Textos del backend ------------------------------------------------------------
// Errores de Rust, progreso y errores del proceso de IA, descripciones de modelos…
// llegan en español. El catálogo `backend.ts` los traduce: las claves con huecos
// {1}, {2}… se reconocen como patrones y lo capturado se traduce a su vez (así
// funcionan los errores encadenados, «no se pudo cargar X: <otro error>»).

type Pattern = { re: RegExp; to: string }
let patterns: Pattern[] | null = null

function buildPatterns(): Pattern[] {
  return Object.entries(backendEn)
    .filter(([k]) => /\{\d+\}/.test(k))
    .map(([k, to]) => {
      const literal = k.replace(/\{\d+\}/g, '').length
      const src = k
        .split(/(\{\d+\})/)
        .map((part) => (/^\{\d+\}$/.test(part) ? '([\\s\\S]+?)' : part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
        .join('')
      return { re: new RegExp(`^${src}$`), to, literal }
    })
    .sort((a, b) => b.literal - a.literal)
    .map(({ re, to }) => ({ re, to }))
}

const patternsFor = (): Pattern[] => (patterns ??= buildPatterns())

/** Traduce un texto que viene del backend (sin cambios si no está en el catálogo). */
export function trBackend(msg: string | null | undefined, depth = 0): string {
  if (!msg) return msg ?? ''
  if (current === 'es' || depth > 4) return msg
  const exact = backendEn[msg]
  if (exact !== undefined) return exact
  const trimmed = msg.trim()
  if (trimmed !== msg && backendEn[trimmed] !== undefined) return msg.replace(trimmed, backendEn[trimmed])
  if (msg.includes('\n')) {
    // «encabezado:\n<detalle>» con el encabezado en el catálogo (salida de pip, etc.).
    const nl = msg.indexOf('\n')
    const head = msg.slice(0, nl + 1)
    if (backendEn[head] !== undefined) return backendEn[head] + msg.slice(nl + 1)
    for (const p of patternsFor()) {
      const m = p.re.exec(head)
      if (m) return p.to.replace(/\{(\d+)\}/g, (_, i: string) => trBackend(m[Number(i)] ?? '', depth + 1)) + msg.slice(nl + 1)
    }
    return msg.split('\n').map((l) => trBackend(l, depth + 1)).join('\n')
  }
  for (const p of patternsFor()) {
    const m = p.re.exec(msg)
    if (m) return p.to.replace(/\{(\d+)\}/g, (_, i: string) => trBackend(m[Number(i)] ?? '', depth + 1))
  }
  // «causa: detalle» con la causa traducible.
  const i = msg.indexOf(': ')
  if (i > 0) {
    const raw = msg.slice(0, i)
    const rest = msg.slice(i + 2)
    if (backendEn[raw + ': '] !== undefined) return backendEn[raw + ': '] + rest
    // Si la causa no se traduce (un nombre, una ruta), el detalle sí.
    return `${trBackend(raw, depth + 1)}: ${trBackend(rest, depth + 1)}`
  }
  return msg
}

/** Elige la forma singular o plural (en ambos idiomas basta con 1 / no 1). */
export const plural = (n: number, one: string, many: string, params?: Record<string, string | number>) =>
  tr(n === 1 ? one : many, { n, ...params })
