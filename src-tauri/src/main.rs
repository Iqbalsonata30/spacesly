// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|argument| argument == "--spacesly-mcp-proxy") {
        if let Err(error) = spacesly_lib::run_mcp_proxy() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    spacesly_lib::run()
}
