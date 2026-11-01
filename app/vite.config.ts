import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Configuración para Tauri: puerto fijo y sin vigilar el código Rust.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'chrome120',
  },
})
