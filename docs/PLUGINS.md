# Compatibilidad con plugins

MusicNova aloja efectos CLAP y VST3 (crate `mn-plugin`). Este documento recoge con qué plugins
reales se ha probado, qué fallos destaparon y cómo repetir las pruebas.

## Sonda de compatibilidad

```
cargo run -p mn-plugin --example probe -- <carpeta o plugin>... [--no-gui]
```

Reproduce lo que hace la aplicación. El hilo principal del proceso bombea los mensajes de Windows y
atiende las tareas de los plugins. El audio corre en otro hilo mientras se abre la interfaz. Para cada
efecto comprueba:

- el escaneo;
- la carga y los parámetros;
- el audio en bloques de 512, 5000 (se trocean) y 7 frames, sin muestras no finitas ni picos absurdos;
- un parámetro fijado por el anfitrión, que el plugin debe reflejar;
- la interfaz: abrirla con el audio sonando, mover un parámetro con ella abierta, cerrarla y reabrirla;
- el estado guardado y restaurado en otra instancia a 44,1 kHz, con parámetros idénticos;
- cuatro instancias procesando a la vez en hilos distintos;
- la descarga y recarga de la biblioteca.

Variables de entorno opcionales:

| Variable | Efecto |
|---|---|
| `MN_PROBE_SHOTS=<carpeta>` | Guarda una captura BMP de cada interfaz y cuenta sus colores (una interfaz que no se dibuja sale casi lisa). |
| `MN_PROBE_DRAG=x,y,dy` | Arrastra el ratón en ese punto de la interfaz, como un usuario moviendo un control. Comprueba qué parámetro cambió y que el cambio llegó al procesador: restaura en otra instancia solo el estado del procesador (en VST3, el del componente sin el del controlador). Mueve el cursor real. |
| `MN_PROBE_VERBOSE=1` | Muestra cada paso mientras ocurre. |
| `MN_PROBE_ALL_THREADS=1` | Si hay un bloqueo, vuelca la pila de todos los hilos. |

Si el hilo principal se queda bloqueado más de 10 s, la sonda dice qué plugin y qué mensaje de
Windows estaba procesando, vuelca su pila con `dbghelp` y termina con código 3.

## Plugins probados (noviembre de 2026, Windows 11, RTX 3050)

| Plugin | Licencia | CLAP | VST3 | Notas |
|---|---|---|---|---|
| Surge XT Effects 1.3.4 | GPL-3.0 | ✔ | ✔ | Todas las pruebas, incluido el arrastre en la interfaz. |
| Surge XT 1.3.4 (sintetizador) | GPL-3.0 | — | — | Es un instrumento: el escaneo lo descarta correctamente. |
| Airwindows Consolidated (2026-10-31) | MIT | ✔ | ✔ | Todas. Al cambiar de efecto desde su interfaz cambia la lista de parámetros; se refleja y se guarda. |
| Dragonfly Hall / Plate / Room Reverb 3.2.10 | GPL-3.0 | ✔ | ✔ | Todas, incluido el arrastre en la interfaz. |
| Dragonfly Early Reflections 3.2.10 | GPL-3.0 | ✔* | ✔* | *El audio, los parámetros y el estado funcionan, pero su interfaz bloquea el hilo principal (el driver OpenGL espera un sincronismo vertical que no llega). Es un fallo conocido del plugin ([dragonfly-reverb#169](https://github.com/michaelwillis/dragonfly-reverb/issues/169)): también congela REAPER, Audacity, Ableton y Reason. No abras su interfaz; usa los parámetros genéricos. |

## Fallos del anfitrión encontrados y corregidos

- **VST3, cambios de salida:** el anfitrión no ofrecía la lista `outputParameterChanges`, y los
  plugins DPF (Dragonfly) lo denunciaban en cada bloque. Ahora se ofrece una lista preasignada. Lo que
  el procesador informa se refleja en el controlador desde el hilo principal.
- **VST3, orden de liberación:** el componente se soltaba antes que el procesador, el controlador y
  los puntos de conexión, y DPF lo denunciaba. Ahora se liberan en el orden correcto: vista,
  conexiones, procesador, controlador, componente y, al final, la biblioteca.
- **CLAP, `stop_processing`:** se llamaba desde el hilo principal al apagar, pero el estándar exige
  el de audio (Airwindows lo comprueba con `thread-check`). Ahora se llama desde un hilo breve marcado
  como de audio. Además, `reset` ya no se llama si el procesado no está en marcha.
- **Escaneo:** la salida del subproceso se leía solo al terminar, así que un plugin que escribiera
  mucho podía bloquearse con la tubería llena hasta agotar el límite de 30 s. Además, su texto rompía
  el análisis del resultado. Ahora se lee en paralelo y el resultado va en una línea marcada.
- **Interfaz de MusicNova:** el panel genérico de parámetros no se actualizaba con lo que se movía
  en la ventana del plugin. Ahora se relee cada segundo mientras está abierto (sin pisar el deslizador
  que se esté arrastrando).
- Los textos de los parámetros se recortan: algunos plugins los rellenan con espacios.

El plugin de prueba (`mn-testplugin`) detecta ahora esos incumplimientos: `stop_processing` o
`reset` fuera del hilo de audio, o la falta de `outputParameterChanges`. Los cuenta en
`mn_test_violations` y la prueba `host_follows_threading_and_output_rules` falla si hay alguno.

## Limitaciones conocidas

- Los plugins corren dentro del proceso de la aplicación. Si la interfaz de un plugin bloquea el hilo
  principal, como Early Reflections, la ventana de MusicNova se congela. El audio sigue sonando y el
  proyecto ya está guardado, salvo el estado del plugin desde el último guardado. Aislar los plugins
  en otro proceso queda pendiente.
- `request_restart` de CLAP y `restartComponent` de VST3 no reinician el plugin. Ninguno de los
  probados lo necesitó.
- No hay compensación de latencia, automatización de parámetros de plugins, instrumentos ni sidechain.
