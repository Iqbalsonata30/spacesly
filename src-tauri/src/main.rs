// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    spacesly_lib::initialize_performance();
    if std::env::args().any(|argument| argument == "--spacesly-task-tools") {
        if let Err(error) = spacesly_lib::run_task_tools() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|argument| argument == "--spacesly-mcp-proxy") {
        if let Err(error) = spacesly_lib::run_mcp_proxy() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|argument| argument == "--spacesly-ocp-connector") {
        if let Err(error) = spacesly_lib::run_ocp_connector() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    spacesly_lib::run()
}
