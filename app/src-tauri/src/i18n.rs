//! Idioma de los datos que crea la aplicación (nombres de pistas y proyectos).
//!
//! Los mensajes (errores, avisos, progreso) se traducen en la interfaz con su
//! catálogo; aquí solo se eligen los nombres que se guardan en el proyecto, para
//! que un proyecto nuevo en inglés tenga «Drums» y no «Batería».

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Nombres en español → inglés.
const NAMES: &[(&str, &str)] = &[
    ("Mezcla original", "Original mix"),
    ("Proyecto nuevo", "New project"),
    ("Máster", "Master"),
    ("Batería", "Drums"),
    ("Bajo", "Bass"),
    ("Guitarra", "Guitar"),
    ("Piano", "Piano"),
    ("Teclado", "Keys"),
    ("Cuerdas", "Strings"),
    ("Sintetizador", "Synth"),
    ("Percusión", "Percussion"),
    ("Metales", "Brass"),
    ("Vientos de madera", "Woodwinds"),
    ("Coros", "Backing vocals"),
    ("Voz", "Vocals"),
    ("Efectos", "FX"),
    ("Otros", "Other"),
    ("Instrumental", "Instrumental"),
    ("Instrumento", "Instrument"),
    // Patrones ({} = lo que se inserta).
    ("{} · sin ruido", "{} · denoised"),
    ("{} · afinada", "{} · tuned"),
    ("{} · generado", "{} · generated"),
    ("Máster · {}", "Master · {}"),
    ("Toma {}", "Take {}"),
    ("Canal {}", "Channel {}"),
];

pub fn set_language(lang: &str) {
    ENGLISH.store(lang.starts_with("en"), Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Nombre en el idioma de la interfaz (si no está en la tabla, tal cual).
pub fn name(es: &str) -> String {
    if english() {
        if let Some((_, en)) = NAMES.iter().find(|(k, _)| *k == es) {
            return en.to_string();
        }
    }
    es.to_string()
}

/// Patrón con `{}` (de la tabla) relleno con `arg`.
pub fn name_with(pattern: &str, arg: &str) -> String {
    name(pattern).replacen("{}", arg, 1)
}

/// Nombre de un stem separado según su tipo.
pub fn stem_name(stem: &str, fallback: &str) -> String {
    let es = match stem {
        "vocals" => "Voz",
        "drums" => "Batería",
        "bass" => "Bajo",
        "guitar" => "Guitarra",
        "piano" => "Piano",
        "other" => "Otros",
        "instrumental" => "Instrumental",
        _ => return fallback.to_string(),
    };
    name(es)
}

/// Lo elige la interfaz al arrancar y al cambiar de idioma.
#[tauri::command]
pub fn app_set_language(lang: String) {
    set_language(&lang);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_language() {
        set_language("es");
        assert_eq!(name_with("Toma {}", "3"), "Toma 3");
        assert_eq!(stem_name("drums", "x"), "Batería");
        set_language("en");
        assert_eq!(name("Mezcla original"), "Original mix");
        assert_eq!(name_with("{} · sin ruido", "Vocals"), "Vocals · denoised");
        assert_eq!(stem_name("vocals", "x"), "Vocals");
        assert_eq!(stem_name("raro", "Raro"), "Raro");
        assert_eq!(name("Mi pista"), "Mi pista", "lo que no está en la tabla queda igual");
        set_language("es");
    }
}
