// Evita una consola extra en Windows en compilaciones de release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    musicnova_lib::run()
}
