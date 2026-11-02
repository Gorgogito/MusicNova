# Transcripción: medidas con audio real

Medidas de la fase 6 con conjuntos de datos públicos anotados a mano. Los datos
no se incluyen en el repositorio; `ai-worker/eval/fetch.py` los descarga y los
scripts de `ai-worker/eval/` reproducen cada tabla (ver el final).

## Acordes (análisis)

GuitarSet: 47 acompañamientos de guitarra real (6 guitarristas, 5 estilos) frente
a la progresión de la partitura anotada, por tramas de 10 ms
(`eval/chords_guitarset.py`).

| Motor | Raíz | Mayor/menor | Tipo exacto (mayor, menor, 7, m7♭5) |
|---|---|---|---|
| Plantillas de croma + HMM (sin modelo) | 74,0 % | 61,5 % | 40,5 % |
| **BTC, vocabulario amplio (el predeterminado si está instalado)** | **82,0 %** | **72,4 %** | **61,9 %** |

BTC (Park et al., ISMIR 2019; MIT) reconoce 14 tipos sobre las 12 raíces. Sus
probabilidades por trama (93 ms) se promedian en cada pulso y se suavizan con
Viterbi, así que los cambios de acorde caen en los pulsos. Los pesos se cargan
sin ejecutar código del archivo (`weights_only` con solo escalares de numpy
permitidos).

## Tono de la voz (afinación vocal)

Vocadito: 40 voces solistas reales (817 s) con f0 anotada. RPA = porcentaje de
fotogramas con voz cuya f0 estimada está a menos de 50 cents de la anotada.

| Detector | RPA | Error mediano | Error p90 | Voz detectada | Falsas alarmas de voz | Tiempo (GPU) |
|---|---|---|---|---|---|---|
| pYIN | 97,2 % | 5,2 cents | 22,2 cents | 99,5 % | 40,8 % | 47 s |
| CREPE rápido (tiny) | 98,9 % | 4,1 cents | 14,8 cents | 88,4 %* | 14,7 %* | 23 s |
| **CREPE (full), el predeterminado** | **99,1 %** | **3,4 cents** | **12,6 cents** | **97,6 %** | **15,5 %** | 69 s |

\* Medido antes de sustituir el umbral de silencio de torchcrepe (ver abajo).

Dos correcciones sobre torchcrepe que salieron de estas medidas:

- Sus decodificadores devuelven el centro de cada intervalo de 20 cents **más un
  ruido triangular de ±20 cents** («dither»). MusicNova usa Viterbi para elegir
  el intervalo y una media ponderada de ±4 intervalos para la f0 fina, como el
  CREPE original: el error mediano bajó de 8,7 a 3,4 cents.
- Su umbral de silencio promedia la sonoridad de todas las bandas y da por mudo
  un tono limpio aunque suene fuerte. Se sustituye por el RMS de cada ventana
  (−60 dBFS): la voz detectada sube del 89,6 % al 97,6 %.

El umbral de voz de CREPE (periodicidad ≥ 0,6) se eligió con esta tabla: pYIN
detecta casi toda la voz pero también da por cantadas el 41 % de las
respiraciones y silencios, que luego se intentarían afinar.

## Audio a MIDI con voz real

Vocadito, modo monofónico, frente a las notas del anotador 1 (acierto = inicio a
±50 ms y altura a ±50 cents).

| Umbrales de Basic Pitch (inicio / nota) | Precisión | Recall | F1 |
|---|---|---|---|
| 0,5 / 0,3 (los de Basic Pitch) | 57,7 % | 40,0 % | 47,2 % |
| **0,8 / 0,25 (los de MusicNova en monofónico)** | **69,9 %** | **44,3 %** | **54,2 %** |

En monofónico, las notas iguales separadas por menos de 30 ms se unen: Basic
Pitch vuelve a disparar el inicio en cada ciclo de un vibrato amplio (±30 cents
a 5 Hz partía una nota de 2 s en seis).

### Curvas de tono dentro de cada nota (pitch bends)

Tono de «nota + curva» frente a la f0 anotada, en los fotogramas cubiertos por
alguna nota (el 84,5 % de la voz), con los umbrales de MusicNova:

| Curvas | Error mediano | Error p90 | A menos de 50 cents |
|---|---|---|---|
| Sin curvas (altura fija) | 26,9 cents | 67,7 cents | 79,1 % |
| Contorno de Basic Pitch (polifónico) | 6,6 cents | 26,0 cents | 96,2 % |
| **CREPE (monofónico, el predeterminado)** | **4,5 cents** | **18,0 cents** | **97,8 %** |

El contorno de Basic Pitch tenía un desvío constante de +33,6 cents (un
intervalo del contorno) medido con tonos puros de Do2 a La6; se corrige en
`midi_pitch_to_contour_bin` y se interpola entre intervalos.

En el MIDI, las curvas van como pitch bend con rango de ±2 semitonos (fijado con
RPN 0, el estándar). Las notas con curva que se solapan se reparten entre
canales libres, porque el pitch bend afecta a todo un canal.

## Batería a MIDI

MDB Drums: batería de 23 canciones reales de MedleyDB con cada golpe anotado.
Acierto = golpe a ±50 ms. Los parámetros se ajustaron con 12 canciones; la
tabla es de las otras 11.

| Entrada | Bombo | Caja | Charles | Media |
|---|---|---|---|---|
| Pista de batería original | 94,8 % | 63,3 % | 64,6 % | **74,3 %** |
| **Stem de batería separado con HTDemucs (como en la aplicación)** | 90,2 % | 63,4 % | 60,4 % | **71,3 %** |
| Mezcla completa sin separar | 49,4 % | 41,1 % | 58,3 % | 49,6 % |

(F por instrumento.) El método es NMF semiadaptativa (Dittmar y Gärtner, 2014)
sin modelos que descargar. Los platos tienen plantilla para que no se confundan
con el charles, pero no se transcriben: su F no llegaba al 15 %.

## Detección de instrumentos

IRMAS (partición de prueba): 157 fragmentos de 5–20 s de música real con los
instrumentos predominantes anotados, hasta 30 por instrumento. Recall = de los
instrumentos anotados, cuántos aparecen en el informe. IRMAS solo anota los
predominantes, así que un acompañamiento real que se detecte cuenta como fallo:
la precisión es una cota inferior.

| Entrada (PANNs Cnn14, umbral 0,2) | Recall | Precisión | F1 |
|---|---|---|---|
| Mezcla | 49,6 % | 55,1 % | 52,2 % |
| **Stems separados con HTDemucs (si el proyecto está separado)** | **67,7 %** | **53,7 %** | **59,9 %** |

Aciertos por instrumento (mezcla → stems): voz 41 → 94 %, piano 11 → 36 %,
guitarra eléctrica 35 → 62 %, chelo 45 → 79 %, violín 70 → 97 %, órgano 30 →
37 %, saxofón 86 → 93 %, trompeta 74 → 76 %, guitarra acústica 86 → 86 %,
clarinete 43 → 47 %, flauta 46 → 57 %. Con Cnn10 en la mezcla: F1 47,5 %.

Con stems, cada grupo se lee de su stem (voz, batería, bajo; piano y guitarras
del suyo si existe; el resto de «otros»). La «voz hablada» sale de la mezcla:
leída del stem de voz aparecía en 68 de 157 fragmentos sin habla; desde la
mezcla, en 14.

Se probó también un umbral distinto por grupo, ajustado con la mitad de las
canciones: en la otra mitad el F1 bajaba (54,6 % → 49,7 %), así que se mantiene
el umbral único.

Límites: el piano dentro de mezclas densas (jazz con metales, la mayoría de los
fragmentos de piano de IRMAS) sigue siendo difícil para PANNs, que le da una
probabilidad media de 0,04 en la mezcla.

## Conservación del timbre al afinar

Diez voces reales de Vocadito transpuestas con el motor de afinación
(`apply_shifts`). Para cada voz se compara la envolvente espectral media (LPC)
con la original y se busca el factor α que mejor las alinea: α = 1 si los
formantes no se mueven, α = 2^(semitonos/12) si se mueven con la nota.

| Transposición | Formantes libres | **Formantes conservados** | Si se movieran con la nota |
|---|---|---|---|
| −5 semitonos | α 0,750 | **α 0,980** | 0,749 |
| −3 semitonos | α 0,842 | **α 0,975** | 0,841 |
| +3 semitonos | α 1,188 | **α 1,046** | 1,189 |
| +5 semitonos | α 1,331 | **α 1,035** | 1,335 |

Al conservar formantes queda un desplazamiento residual del 2–5 % y la
envolvente cambia unos 1,1–1,5 dB (frente a 2,7–4,0 dB sin conservarlos). Darle
a Signalsmith la f0 conocida como guía para separar la envolvente no mejoraba
el resultado (α 0,947–1,061), así que se deja que la estime él.

## Reproducir las medidas

```
cd ai-worker
set MN_EVAL_DATA=D:\datos-eval          (opcional; por defecto ai-worker/eval-data)
.venv\Scripts\python eval\fetch.py      (vocadito, mdb, irmas)
.venv\Scripts\python eval\pitch_vocadito.py
.venv\Scripts\python eval\notes_vocadito.py contour crepe
.venv\Scripts\python eval\separate_mdb.py && .venv\Scripts\python eval\drums_mdb.py sep prueba
.venv\Scripts\python eval\drums_mdb.py drum_only prueba
.venv\Scripts\python eval\instruments_irmas.py cnn14
.venv\Scripts\python eval\separate_irmas.py cnn14 && .venv\Scripts\python eval\instruments_stems_irmas.py cnn14
cargo build --release -p mn-core --example shift_file
.venv\Scripts\python eval\formants_vocadito.py
```

Los modelos (HTDemucs, PANNs) deben estar instalados en «Modelos».
