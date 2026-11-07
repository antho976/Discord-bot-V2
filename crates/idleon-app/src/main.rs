// No console window behind the app on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> iced::Result {
    idleon_app::run()
}
