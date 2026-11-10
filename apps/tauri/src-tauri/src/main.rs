#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `terrarium serve` runs the app without its window.
    #[cfg(desktop)]
    if std::env::args().nth(1).as_deref() == Some("--headless") {
        std::process::exit(terrarium_app_lib::run_headless());
    }
    terrarium_app_lib::run()
}
