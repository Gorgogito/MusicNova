import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './styles.css'
import App from './App'
import { invoke } from '@tauri-apps/api/core'
import { lang } from './i18n'

// Rust crea los nombres de pistas y proyectos en el idioma de la interfaz.
invoke('app_set_language', { lang: lang() }).catch(() => {})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
