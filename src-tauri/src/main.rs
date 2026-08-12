// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--mcp") {
        let client_id = args
            .windows(2)
            .find(|pair| pair[0] == "--client")
            .map(|pair| pair[1].clone());
        let result = client_id
            .ok_or_else(|| anyhow::anyhow!("--client is required in MCP mode"))
            .and_then(tauri_app_lib::mcp::run_stdio);
        if let Err(error) = result {
            eprintln!("noted MCP: {error}");
            std::process::exit(1);
        }
        return;
    }
    tauri_app_lib::run()
}
