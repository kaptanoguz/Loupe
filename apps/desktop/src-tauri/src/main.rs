// Sürüm derlemesinde Windows'ta konsol penceresi açılmasın (Linux'ta etkisiz).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    match loupe_desktop::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loupe: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
