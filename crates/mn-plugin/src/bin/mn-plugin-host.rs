//! Anfitrión de plugins independiente (lo usan las pruebas; la aplicación se
//! relanza a sí misma con `--plugin-host`).

fn main() {
    if !mn_plugin::host_main() {
        eprintln!("uso: mn-plugin-host --plugin-host");
        std::process::exit(2);
    }
}
