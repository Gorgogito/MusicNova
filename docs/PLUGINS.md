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
| Surge XT 1.3.4 (sintetizador) | GPL-3.0 | ✔ | ✔ | Como instrumento en pistas de instrumento: un acorde de Do da picos en 261/330/392 Hz y un La3 en 220 Hz; el cambio desde la interfaz regenera la pista en la nota correcta. |
| Airwindows Consolidated (2026-10-31) | MIT | ✔ | ✔ | Todas. Al cambiar de efecto desde su interfaz cambia la lista de parámetros; se refleja y se guarda. |
| Dragonfly Hall / Plate / Room Reverb 3.2.10 | GPL-3.0 | ✔ | ✔ | Todas, incluido el arrastre en la interfaz. |
| Dragonfly Early Reflections 3.2.10 | GPL-3.0 | ✔ | ✔ | Con los plugins en el proceso de la app, su interfaz bloqueaba el hilo principal (OpenGL esperaba un sincronismo vertical que no llegaba; fallo conocido, [dragonfly-reverb#169](https://github.com/michaelwillis/dragonfly-reverb/issues/169), que también congela REAPER, Audacity, Ableton y Reason). En su propio proceso la interfaz funciona (CLAP y VST3): responde cada ~16 ms y el audio sigue (ejemplo `isolation`). |

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

- `request_restart` de CLAP y `restartComponent` de VST3 no reinician el plugin. Ninguno de los
  probados lo necesitó.
- No hay automatización de parámetros de plugins.
- Las pistas de instrumento generan su audio sin conexión (al editar notas o cambiar de
  instrumento), no en tiempo real: tocar un teclado MIDI en vivo no está soportado.

## Cada plugin en su propio proceso

Cada efecto o instrumento con plugin corre en un proceso anfitrión propio: la aplicación relanzada
con `--plugin-host` (en las pruebas, el binario `mn-plugin-host`). El anfitrión aloja el plugin con
el mismo código de siempre. Su hilo principal ejecuta un bucle de mensajes de Windows: órdenes de
la app y ventana del plugin. Un hilo de audio de prioridad crítica atiende los bloques.

- **Órdenes** (cargar, parámetros, estado, abrir o cerrar la interfaz): JSON por líneas en
  stdin/stdout. El anfitrión desvía su stdout original a stderr para que lo que impriman los
  plugins no ensucie el protocolo.
- **Audio:** memoria compartida con el bloque (≤ 2048 frames estéreo, cambios de parámetros y
  notas) y dos eventos de Windows («pedido» y «hecho»). La espera incluye el proceso: si muere,
  se nota al instante. Ida y vuelta medida: unos 36 µs por bloque (256 frames a 48 kHz duran 5,3 ms).
- **Ventana del plugin:** es del anfitrión, sin ventana dueña en la app (una dueña de otro proceso
  uniría las colas de entrada y un plugin colgado congelaría la app). Tiene botón en la barra de
  tareas y la app le cede el primer plano con `AllowSetForegroundWindow`.
- **Si se cae** (cualquier código de salida): el efecto pasa el audio sin procesar, la app avisa
  («se cerró inesperadamente (código 0x…)») y el panel del efecto ofrece «Reiniciar plugin». Al
  reiniciar se carga con el último estado guardado en el proyecto.
- **Si se cuelga el audio:** en tiempo real, un bloque espera como máximo 250 ms. Los siguientes
  pasan sin procesar sin esperar mientras siga colgado; tras 5 s se termina su proceso. Al exportar
  o generar, el límite es de 30 s.
- **Si se cuelga su hilo principal** (como la interfaz de Early Reflections): la orden que espera
  se rinde a los 8 s, se termina el proceso y la app sigue respondiendo. Cargar tiene 60 s.
- **Si la app desaparece:** el anfitrión ve el fin de stdin y se cierra solo.
- `MUSICNOVA_PLUGINS_INPROCESS=1` vuelve a cargarlos en el proceso de la app (para depurar).

Ejemplo con un plugin real: `cargo build -p mn-plugin --bins --examples` y
`target\debug\examples\isolation <plugin> [id]` (carga, procesa, abre la interfaz y mide que
las órdenes y el audio siguen respondiendo).

Verificado con el plugin de prueba `test-crash` (se cae al procesar, cuelga el audio o cuelga el
hilo principal a voluntad; pruebas en `crates/mn-plugin/tests/remote.rs`) y en la app real. Con
Surge XT Effects en el máster, matar su proceso durante la reproducción no detuvo el audio; llegó el
aviso y «Reiniciar plugin» lanzó un anfitrión nuevo con la interfaz operativa. Surge XT (VST3)
generó la pista de instrumento en un anfitrión temporal.

## Instrumentos, latencia y sidechain

- **Instrumentos:** el escaneo marca como instrumento los CLAP con la característica
  `instrument` y los VST3 de subcategoría `Instrument` sin `Fx`. Las notas llegan como
  `clap_event_note` (mezcladas por tiempo con los cambios de parámetro) o por una `IEventList`
  preasignada en VST3, con el bus de eventos activado y sin buses de entrada de audio.
  `mn_plugin::render_instrument` genera el audio en bloques con una instancia propia.
- **Latencia:** se lee con `CLAP_EXT_LATENCY` o `getLatencySamples` y se compensa con una
  línea de retardo por pista en el motor (las pistas de menos latencia esperan a la mayor),
  en el metrónomo, en la alineación de las grabaciones y en la exportación.
- **Sidechain:** solo el compresor integrado; la señal clave es el audio crudo de otra pista
  (antes de efectos y fader, incluso silenciada).
