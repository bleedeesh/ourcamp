//! OurCamp Server window entry point.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ourcamp_server_lib::run();
}
