# IA generativa: limpieza de ruido y acompañamientos

Fase 7. Todo se mide con audio real; los datos se descargan con
`ai-worker/eval/fetch.py` (ver `docs/TRANSCRIPCION.md`) y los scripts de
`ai-worker/eval/` reproducen cada tabla.

## Limpieza de voz (DeepFilterNet3)

DeepFilterNet3 (MIT o Apache-2.0) separa la voz de cualquier ruido de fondo a
48 kHz. MusicNova usa los tres grafos ONNX publicados con onnxruntime (8 MB, sin
PyTorch) y un port propio en numpy del procesado de señal de libDF: STFT con
ventana Vorbis, 32 bandas ERB con media exponencial, espectro complejo
normalizado, máscara ERB y filtrado profundo de orden 5 en los 96 bins graves.
Los archivos largos se procesan en trozos de 20 s solapados 2 s.

Ocho voces reales de Vocadito con ruido blanco a 5 dB de SNR
(`eval/denoise_voice_vocadito.py`):

| Señal | SI-SDR medio |
|---|---|
| Ruidosa | 5,0 dB |
| **DeepFilterNet3 (MusicNova)** | **18,1 dB** |
| Control: solo la máscara ERB | 8,7 dB |
| Control: filtrado profundo desalineado | −25,2 dB |

Los controles validan el port: sin el filtrado profundo se pierden 9 dB y con
la anticipación mal puesta el resultado se destruye. Procesar por trozos no
cuesta calidad (18,4 dB frente a 18,3 dB de una pieza en 45 s de voz) y es
rápido: 3,2 s en CPU para 45 s en estéreo.

## Ruido constante en cualquier instrumento

Sustracción espectral en Rust (`mn_core::denoise`): ganancia de Wiener con SNR a
priori «dirigida por decisión» (Ephraim y Malah), suelo de reducción y
suavizado en frecuencia contra el ruido musical. El perfil de ruido sale del
bucle A-B si se marca un tramo con solo ruido o, si no, de las tramas más
silenciosas de la pista (un zumbido y una nota sostenida son igual de estables:
sin silencios ni tramo de referencia no se pueden distinguir).

Instrumentos reales de IRMAS con siseo blanco y 1 s de solo ruido delante
(`eval/denoise_steady_irmas.py`, ajustes por defecto: 18 dB, sensibilidad 0,3):

| SNR de entrada | Chelo | Clarinete | Flauta | Guitarra acústica | Órgano | Mejora media |
|---|---|---|---|---|---|---|
| 10 dB | 15,5 | 19,0 | 18,7 | 15,0 | 18,6 | **+7,4 dB** |
| 20 dB | 23,7 | 27,7 | 24,4 | 22,4 | 24,8 | **+4,6 dB** |

La sensibilidad por defecto (0,3) es la que dio la mejor SNR; 0,5 (el ruido
estimado tal cual) quitaba más ruido y también más señal (+7,0 / +3,6 dB).

## Acompañamientos (ACE-Step 1.5)

ACE-Step 1.5 base (código y pesos MIT, uso comercial de lo generado permitido)
genera una pista concreta (batería, bajo, guitarra, teclado, cuerdas,
sintetizador, percusión, metales, vientos, coros, voz o efectos) en el contexto
de la canción (tarea «lego»), con el tempo, el compás y la tonalidad del
análisis. El contexto puede ser lo que suena (sin las pistas silenciadas, para
sustituir una parte) o la mezcla original.

ACE-Step pide torch 2.7.1 y el proceso de IA está fijado en torch 2.11, así que
vive en un entorno de Python aparte que la aplicación instala (PyTorch 2.7.1
para CUDA 12.8 y las versiones exactas del `uv.lock` de ACE-Step; unos 3–4 GB) y
se lanza como subproceso. Los pesos (DiT base, VAE y codificador de texto
Qwen3-Embedding-0.6B, Apache-2.0; 6,3 GB) se descargan desde «Modelos» con
comprobación SHA-256.

Cuatro canciones de MDB Drums: contexto = mezcla − batería original; se genera
una batería nueva (32 pasos) y se compara con la batería real
(`eval/generation_mdb.py`). Encaje rítmico = F de los golpes generados
(detectados con la transcripción de batería) frente a los golpes reales, a ±50
ms; azar = los mismos golpes desplazados 0,1–0,4 s.

| Canción | Duración | Tiempo (RTX 3050 8 GB) | Encaje | Azar | PANNs reconoce |
|---|---|---|---|---|---|
| Beatles | 36 s | 45 s | 0,58 | 0,25 | Batería, percusión |
| Britpop | 37 s | 41 s | 0,81 | 0,40 | Batería, percusión |
| Country1 | 35 s | 42 s | 0,58 | 0,15 | Batería, percusión |
| FreeJazz | 104 s | 61 s | 0,58 | 0,50 | Batería, percusión |
| **Media** | | **≈ 1× la duración** | **0,64** | **0,33** | |

La batería generada sigue el pulso de la canción (casi el doble que el azar; en
free jazz, sin pulso regular, la diferencia es menor) y es solo batería: su
correlación con el contexto es de 0,2–0,26 (≈ 6 % de la energía; en FreeJazz,
0). La calidad musical (estilo, gusto) no se puede medir así: hay que
escucharla; por eso cada generación es una pista nueva y se pueden probar
varias semillas.

Límites: en 8 GB de GPU ACE-Step descarga modelos a la CPU entre pasos; más
pasos (calidad «alta») tardan proporcionalmente más. Sin el entorno o el
modelo, el panel guía la instalación paso a paso.
