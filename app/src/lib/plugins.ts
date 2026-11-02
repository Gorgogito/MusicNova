// Lista de plugins escaneados, compartida por toda la interfaz.
import { useSyncExternalStore } from 'react'
import { api, events, type PluginCache, type PluginDescriptor, type EffectSlot } from './api'

type Progress = { done: number; total: number; current: string }
let cache: PluginCache | null = null
let scanning: Progress | null = null
const listeners = new Set<() => void>()
const emit = () => listeners.forEach((l) => l())

let snapshot: { cache: PluginCache | null; scanning: Progress | null } = { cache, scanning }
function update() {
  snapshot = { cache, scanning }
  emit()
}

export function refreshPlugins(): Promise<void> {
  return api.pluginsList().then((c) => {
    cache = c
    update()
  })
}

export async function scanPlugins(folders: string[]): Promise<PluginCache> {
  scanning = { done: 0, total: 0, current: '' }
  update()
  const un = await events.pluginScan((p) => {
    scanning = p
    update()
  })
  try {
    cache = await api.pluginsScan(folders)
    return cache
  } finally {
    un()
    scanning = null
    update()
  }
}

export function usePlugins() {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      if (!cache) refreshPlugins().catch(() => {})
      return () => listeners.delete(cb)
    },
    () => snapshot,
  )
}

export function pluginSlot(d: PluginDescriptor): EffectSlot {
  return {
    id: crypto.randomUUID(),
    enabled: true,
    params: { type: 'plugin' },
    plugin: { format: d.format, path: d.path, id: d.id, name: d.name, vendor: d.vendor, state: null },
  }
}

export const FORMAT_LABEL: Record<PluginDescriptor['format'], string> = { clap: 'CLAP', vst3: 'VST3' }
