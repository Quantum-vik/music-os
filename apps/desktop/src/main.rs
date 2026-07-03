//! MusicOS desktop entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    musicos_desktop::run();
}
