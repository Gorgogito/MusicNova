// Evita una consola extra en Windows en compilaciones de release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `--scan-plugin <ruta>`: escaneo aislado de un plugin (lo lanza la propia app).
    if musicnova_lib::scan_subprocess_main() {
        return;
    }
    musicnova_lib::run()
}
