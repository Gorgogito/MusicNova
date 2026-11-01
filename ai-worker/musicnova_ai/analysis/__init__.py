"""Análisis musical: pulsos, compases, tempo, tonalidad, acordes y secciones.

Los módulos trabajan sobre arrays de NumPy a `SR` Hz en mono y no dependen del
protocolo RPC, para poder probarlos por separado.
"""

SR = 22_050
HOP = 512

NOTE_NAMES = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"]
