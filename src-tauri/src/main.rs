// Hide the console window on Windows release builds. Without this a trading
// terminal launches with a stray terminal behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    atas_desktop_lib::run();
}
