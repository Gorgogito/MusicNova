// Paso previo a `tauri build` (beforeBuildCommand): Python incluido y código
// del proceso de IA limpio (sin cachés de bytecode del desarrollo).
import { execFileSync } from 'node:child_process'
import { readdirSync, rmSync, statSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const app = join(dirname(fileURLToPath(import.meta.url)), '..')
execFileSync(process.execPath, [join(app, 'scripts', 'fetch-python.mjs')], { stdio: 'inherit' })

const clean = (d) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f)
    if (!statSync(p).isDirectory()) continue
    if (f === '__pycache__') rmSync(p, { recursive: true, force: true })
    else clean(p)
  }
}
clean(join(app, '..', 'ai-worker', 'musicnova_ai'))
clean(join(app, 'src-tauri', 'resources', 'python'))
console.log('Recursos del instalador listos')
