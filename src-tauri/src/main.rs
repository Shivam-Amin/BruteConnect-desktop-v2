// //src-tauri/src/main.rs
// // Prevents additional console window on Windows in release, DO NOT REMOVE!!
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// fn main() {
//     bruteconnect_desktop_lib::run()
// }
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::{net::IpAddr, sync::Mutex};
use tauri::Emitter;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

use if_addrs::get_if_addrs;
use searchlight::{
    broadcast::{BroadcasterBuilder, BroadcasterHandle, ServiceBuilder},
    discovery::{DiscoveryBuilder, DiscoveryEvent, DiscoveryHandle, Responder},
    net::IpVersion,
};
use serde::Serialize;
use tauri::{Manager, State};

// ---- State ----
#[derive(Default)]
struct MdnsState {
    discovery: Mutex<Option<DiscoveryHandle>>,
    broadcaster: Mutex<Option<BroadcasterHandle>>,
    last_service_info: Mutex<Option<ServiceInfo>>,
    socket_server_port: Mutex<Option<u16>>,
    socket_server_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Clone)]
struct ServiceInfo {
    service_type: String,
    instance_name: String,
    port: u16,
    txt: Vec<String>,
}

impl Drop for MdnsState {
    fn drop(&mut self) {
        println!("MdnsState being dropped - performing final cleanup");

        // Cleanup broadcaster
        if let Ok(mut broadcaster_guard) = self.broadcaster.lock() {
            if let Some(handle) = broadcaster_guard.take() {
                println!("Dropping broadcaster handle...");
                if let Err(e) = handle.shutdown() {
                    eprintln!("Error during broadcaster drop cleanup: {}", e);
                }
            }
        }

        // Cleanup socket server
        if let Ok(mut socket_handle_guard) = self.socket_server_handle.lock() {
            if let Some(handle) = socket_handle_guard.take() {
                println!("Dropping socket server handle...");
                handle.abort();
            }
        }

        // Clear service info
        if let Ok(mut service_info_guard) = self.last_service_info.lock() {
            *service_info_guard = None;
        }

        // Cleanup discovery
        if let Ok(mut discovery_guard) = self.discovery.lock() {
            if let Some(handle) = discovery_guard.take() {
                println!("Dropping discovery handle...");
                if let Err(e) = handle.shutdown() {
                    eprintln!("Error during discovery drop cleanup: {}", e);
                }
            }
        }

        println!("MdnsState drop cleanup completed");
    }
}

// Collect non-loopback IPs so we can advertise the service.
fn local_ips() -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            // Skip loopback
            if iface.is_loopback() {
                continue;
            }
            out.push(iface.ip());
        }
    }
    out
}

#[derive(Serialize, Clone)]
struct FoundDevice {
    name: String,
    hostname: String,
    addr: String,
    port: u16,
    txt: Vec<String>,
}

#[tauri::command]
fn register_service(
    state: State<MdnsState>,
    service_type: String,  // e.g. "_bruteconnect._tcp.local."
    instance_name: String, // e.g. "BruteConnect-1234"
    port: u16,             // e.g. 9000
    txt: Vec<String>,      // e.g. ["role=desktop"]
) -> Result<(), String> {
    // Check if socket server is running
    let socket_port = state.socket_server_port.lock().unwrap();
    if socket_port.is_none() {
        return Err("Socket server must be started before registering mDNS service. Please start the socket server first.".into());
    }
    let socket_port = socket_port.unwrap();
    println!(
        "Registering service: {} as {} on port {}",
        service_type, instance_name, port
    );

    let ips = local_ips();
    if ips.is_empty() {
        return Err("No non-loopback IPs found for advertisement".into());
    }

    // Build the service to broadcast
    let mut svc = ServiceBuilder::new(&service_type, &instance_name, port)
        .map_err(|e| format!("invalid service params: {e}"))?;

    for ip in ips {
        svc = svc.add_ip_address(ip);
        println!("Added IP address: {}", ip);
    }
    // Add socket port to TXT records
    let mut enhanced_txt = txt.clone();
    enhanced_txt.push(format!("socketPort={}", socket_port));

    // Store service info for potential goodbye messages before consuming txt
    let service_info = ServiceInfo {
        service_type: service_type.clone(),
        instance_name: instance_name.clone(),
        port,
        txt: enhanced_txt.clone(),
    };

    for rec in enhanced_txt {
        svc = svc.add_txt_truncated(rec);
    }

    let svc = svc
        .build()
        .map_err(|e| format!("service build failed: {e}"))?;

    // Start broadcasting in the background and keep its handle
    let broadcaster = BroadcasterBuilder::new()
        .add_service(svc)
        .build(IpVersion::Both)
        .map_err(|e| format!("broadcaster build failed: {e}"))?
        .run_in_background();

    let mut guard = state.broadcaster.lock().unwrap();
    if let Some(prev) = guard.take() {
        println!("Shutting down previous broadcaster...");
        let _ = prev.shutdown();
    }
    *guard = Some(broadcaster);

    // Store the service info
    *state.last_service_info.lock().unwrap() = Some(service_info);

    println!("Service registration completed successfully");
    Ok(())
}

#[tauri::command]
fn unregister_service(state: State<MdnsState>) -> Result<(), String> {
    println!("Unregistering service...");

    match state.broadcaster.lock() {
        Ok(mut broadcaster_guard) => {
            if let Some(handle) = broadcaster_guard.take() {
                println!("Shutting down broadcaster service...");

                // Shutdown the broadcaster - this should send goodbye messages
                handle
                    .shutdown()
                    .map_err(|e| format!("broadcast shutdown failed: {e}"))?;

                println!("Service unregistered successfully");

                // Send explicit goodbye message to ensure immediate cache invalidation
                drop(broadcaster_guard); // Release the lock before calling send_goodbye_message
                if let Err(e) = send_goodbye_message(state.clone()) {
                    eprintln!("Warning: Failed to send goodbye message: {}", e);
                }

                // Clear the service info
                *state.last_service_info.lock().unwrap() = None;
            } else {
                println!("No service was registered");
            }
        }
        Err(e) => {
            return Err(format!("Failed to acquire broadcaster lock: {e}"));
        }
    }

    Ok(())
}

#[tauri::command]
fn start_discovery(
    app: tauri::AppHandle,
    state: State<MdnsState>,
    service_type: String, // e.g. "_bruteconnect._tcp.local."
) -> Result<(), String> {
    if state.discovery.lock().unwrap().is_some() {
        return Ok(()); // already running
    }

    let app_for_cb = app.clone();
    let discovery = DiscoveryBuilder::new()
        .service(&service_type)
        .map_err(|e| format!("invalid service type: {e}"))?
        .build(IpVersion::Both)
        .map_err(|e| format!("discovery build failed: {e}"))?
        .run_in_background(move |event| match event {
            DiscoveryEvent::ResponderFound(responder) => {
                let _ = emit_responder(&app_for_cb, "mdns:found", &responder);
            }
            DiscoveryEvent::ResponderLost(responder) => {
                let _ = emit_responder(&app_for_cb, "mdns:lost", &responder);
            }
            DiscoveryEvent::ResponseUpdate { new, .. } => {
                let _ = emit_responder(&app_for_cb, "mdns:update", &new);
            } // Fixed: Remove unreachable pattern since all enum variants are covered above
        });

    *state.discovery.lock().unwrap() = Some(discovery);
    Ok(())
}

#[tauri::command]
fn stop_discovery(state: State<MdnsState>) -> Result<(), String> {
    println!("Stopping discovery...");

    match state.discovery.lock() {
        Ok(mut discovery_guard) => {
            if let Some(handle) = discovery_guard.take() {
                println!("Shutting down discovery service...");
                handle
                    .shutdown()
                    .map_err(|e| format!("discovery shutdown failed: {e}"))?;
                println!("Discovery stopped successfully");
            } else {
                println!("No discovery was running");
            }
        }
        Err(e) => {
            return Err(format!("Failed to acquire discovery lock: {e}"));
        }
    }

    Ok(())
}

#[tauri::command]
fn get_service_status(state: State<MdnsState>) -> Result<serde_json::Value, String> {
    let broadcaster_active = state
        .broadcaster
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false);

    let discovery_active = state
        .discovery
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false);

    Ok(serde_json::json!({
        "broadcaster_active": broadcaster_active,
        "discovery_active": discovery_active
    }))
}

#[tauri::command]
fn force_cleanup(state: State<MdnsState>) -> Result<(), String> {
    println!("Force cleanup requested");
    cleanup(&state);
    Ok(())
}

#[tauri::command]
fn send_goodbye_message(state: State<MdnsState>) -> Result<(), String> {
    println!("Sending goodbye message...");

    // Get the last service info
    let service_info = {
        let guard = state.last_service_info.lock().unwrap();
        guard.clone()
    };

    if let Some(info) = service_info {
        println!(
            "Sending goodbye for service: {} ({})",
            info.instance_name, info.service_type
        );

        // Create a temporary broadcaster just to send goodbye messages
        // We'll create the service and immediately shut it down, which should send goodbye messages
        let ips = local_ips();
        if ips.is_empty() {
            return Err("No non-loopback IPs found for goodbye message".into());
        }

        let mut svc = ServiceBuilder::new(&info.service_type, &info.instance_name, info.port)
            .map_err(|e| format!("invalid service params for goodbye: {e}"))?;

        for ip in ips {
            svc = svc.add_ip_address(ip);
        }
        for rec in &info.txt {
            svc = svc.add_txt_truncated(rec.clone());
        }

        let svc = svc
            .build()
            .map_err(|e| format!("service build failed for goodbye: {e}"))?;

        // Create broadcaster and immediately shut it down to send goodbye
        let goodbye_broadcaster = BroadcasterBuilder::new()
            .add_service(svc)
            .build(IpVersion::Both)
            .map_err(|e| format!("goodbye broadcaster build failed: {e}"))?
            .run_in_background();

        // Give it a moment to start, then shut down to send goodbye
        std::thread::sleep(std::time::Duration::from_millis(100));

        goodbye_broadcaster
            .shutdown()
            .map_err(|e| format!("goodbye broadcast shutdown failed: {e}"))?;

        println!("Goodbye message sent successfully");

        // Send multiple goodbye messages to ensure they reach all devices
        println!("Sending additional goodbye messages...");
        for i in 1..=3 {
            std::thread::sleep(std::time::Duration::from_millis(200));

            // Create another temporary broadcaster for additional goodbye
            let mut svc2 = ServiceBuilder::new(&info.service_type, &info.instance_name, info.port)
                .map_err(|e| format!("invalid service params for goodbye {}: {e}", i))?;

            for ip in local_ips() {
                svc2 = svc2.add_ip_address(ip);
            }
            for rec in &info.txt {
                svc2 = svc2.add_txt_truncated(rec.clone());
            }

            let svc2 = svc2
                .build()
                .map_err(|e| format!("service build failed for goodbye {}: {e}", i))?;

            let goodbye_broadcaster2 = BroadcasterBuilder::new()
                .add_service(svc2)
                .build(IpVersion::Both)
                .map_err(|e| format!("goodbye broadcaster {} build failed: {e}", i))?
                .run_in_background();

            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = goodbye_broadcaster2.shutdown();
            println!("Additional goodbye message {} sent", i);
        }

        // Extra delay for goodbye propagation
        std::thread::sleep(std::time::Duration::from_millis(300));
        println!("All goodbye messages propagation completed");
    } else {
        println!("No service info available for goodbye message");
    }

    Ok(())
}

// Cursor control functions
fn handle_cursor_command(action: &str, json_data: &serde_json::Value) {
    println!("Handling cursor command: {}", action);

    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(enigo) => enigo,
        Err(e) => {
            eprintln!("Failed to create Enigo instance for cursor: {}", e);
            return;
        }
    };

    match action {
        "left_click" => {
            println!("Simulating left mouse click");
            if let Err(e) = enigo.button(Button::Left, Direction::Click) {
                eprintln!("Failed to simulate left click: {}", e);
            }
        }
        "right_click" => {
            println!("Simulating right mouse click");
            if let Err(e) = enigo.button(Button::Right, Direction::Click) {
                eprintln!("Failed to simulate right click: {}", e);
            }
        }
        "move" => {
            if let (Some(delta_x), Some(delta_y)) = (
                json_data.get("deltaX").and_then(|v| v.as_i64()),
                json_data.get("deltaY").and_then(|v| v.as_i64()),
            ) {
                println!("Moving cursor by deltaX: {}, deltaY: {}", delta_x, delta_y);
                if let Err(e) = enigo.move_mouse(delta_x as i32, delta_y as i32, Coordinate::Rel) {
                    eprintln!("Failed to move cursor: {}", e);
                }
            } else {
                println!("Invalid cursor move command - missing deltaX or deltaY");
            }
        }
        "scroll" => {
            if let (Some(direction), Some(delta)) = (
                json_data.get("direction").and_then(|v| v.as_str()),
                json_data.get("delta").and_then(|v| v.as_i64()),
            ) {
                let scroll_amount = if direction == "up" {
                    delta as i32
                } else {
                    -(delta as i32)
                };
                println!("Scrolling {} by delta: {}", direction, scroll_amount);
                if let Err(e) = enigo.scroll(scroll_amount, Axis::Vertical) {
                    eprintln!("Failed to scroll: {}", e);
                }
            } else {
                println!("Invalid scroll command - missing direction or delta");
            }
        }
        _ => {
            println!("Unknown cursor action: {}", action);
        }
    }
}

// Presentation control functions
fn handle_presentation_command(action: &str) {
    println!("Handling presentation command: {}", action);

    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(enigo) => enigo,
        Err(e) => {
            eprintln!("Failed to create Enigo instance: {}", e);
            return;
        }
    };

    match action {
        "left" => {
            println!("Simulating Left Arrow key press");
            if let Err(e) = enigo.key(Key::LeftArrow, enigo::Direction::Click) {
                eprintln!("Failed to simulate Left Arrow key: {}", e);
            }
        }
        "right" => {
            println!("Simulating Right Arrow key press");
            if let Err(e) = enigo.key(Key::RightArrow, enigo::Direction::Click) {
                eprintln!("Failed to simulate Right Arrow key: {}", e);
            }
        }
        _ => {
            println!("Unknown presentation action: {}", action);
        }
    }
}

// Socket server implementation
async fn handle_socket_connection(mut stream: TcpStream, addr: std::net::SocketAddr) {
    println!("New socket connection from: {}", addr);

    let mut buffer = [0; 1024];

    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => {
                println!("Connection closed by client: {}", addr);
                break;
            }
            Ok(n) => {
                let message = String::from_utf8_lossy(&buffer[..n]);
                println!("Received from {}: {}", addr, message.trim());

                // Try to parse as JSON and handle presentation commands
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(message.trim()) {
                    // Check if it's a direct presentation command
                    if let (Some(msg_type), Some(action)) = (
                        json_value.get("type").and_then(|v| v.as_str()),
                        json_value.get("action").and_then(|v| v.as_str()),
                    ) {
                        match msg_type {
                            "presentation" => handle_presentation_command(action),
                            "cursor" => handle_cursor_command(action, &json_value),
                            _ => println!("Unknown message type: {}", msg_type),
                        }
                    }
                    // Check if it's nested in a "data" field (mobile app format)
                    else if let Some(data_str) = json_value.get("data").and_then(|v| v.as_str()) {
                        if let Ok(inner_json) = serde_json::from_str::<serde_json::Value>(data_str)
                        {
                            if let (Some(msg_type), Some(action)) = (
                                inner_json.get("type").and_then(|v| v.as_str()),
                                inner_json.get("action").and_then(|v| v.as_str()),
                            ) {
                                match msg_type {
                                    "presentation" => handle_presentation_command(action),
                                    "cursor" => handle_cursor_command(action, &inner_json),
                                    _ => println!("Unknown inner message type: {}", msg_type),
                                }
                            } else {
                                println!("Invalid inner JSON format - missing type or action");
                            }
                        } else {
                            println!("Failed to parse inner JSON data");
                        }
                    } else {
                        println!("Invalid JSON format - missing type/action or data field");
                    }
                } else {
                    println!(
                        "Failed to parse JSON, treating as plain text: {}",
                        message.trim()
                    );
                }
            }
            Err(e) => {
                eprintln!("Failed to read from socket: {}", e);
                break;
            }
        }
    }
}

async fn run_socket_server(port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    println!("Socket server listening on: {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tokio::spawn(handle_socket_connection(stream, addr));
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
            }
        }
    }
}

#[tauri::command]
async fn start_socket_server(state: State<'_, MdnsState>) -> Result<u16, String> {
    println!("Starting socket server...");

    // Check if server is already running
    if state.socket_server_port.lock().unwrap().is_some() {
        let port = state.socket_server_port.lock().unwrap().unwrap();
        println!("Socket server already running on port: {}", port);
        return Ok(port);
    }

    // Get a random free port
    let port = portpicker::pick_unused_port().ok_or("Failed to find an unused port")?;

    println!("Selected port: {}", port);

    // Start the server in a background task
    let server_handle = tokio::spawn(async move {
        if let Err(e) = run_socket_server(port).await {
            eprintln!("Socket server error: {}", e);
        }
    });

    // Store the port and handle
    *state.socket_server_port.lock().unwrap() = Some(port);
    *state.socket_server_handle.lock().unwrap() = Some(server_handle);

    println!("Socket server started successfully on port: {}", port);
    Ok(port)
}

#[tauri::command]
fn stop_socket_server(state: State<MdnsState>) -> Result<(), String> {
    println!("Stopping socket server...");

    // Stop the server task
    if let Some(handle) = state.socket_server_handle.lock().unwrap().take() {
        handle.abort();
        println!("Socket server task stopped");
    }

    // Clear the port
    *state.socket_server_port.lock().unwrap() = None;

    println!("Socket server stopped successfully");
    Ok(())
}

#[tauri::command]
fn get_socket_server_status(state: State<MdnsState>) -> Result<serde_json::Value, String> {
    let port = *state.socket_server_port.lock().unwrap();
    let is_running = port.is_some();

    Ok(serde_json::json!({
        "running": is_running,
        "port": port
    }))
}

fn emit_responder(
    app: &tauri::AppHandle,
    topic: &str,
    r: &std::sync::Arc<Responder>,
) -> Result<(), tauri::Error> {
    use searchlight::dns::{op::DnsResponse, rr::RData};

    let packet: &DnsResponse = &r.last_response; // last response we got

    let mut name = String::new();
    let mut port: u16 = 0;
    let mut hostname = String::new();
    let mut txt: Vec<String> = Vec::new();

    // Walk additionals to pull SRV/TXT
    for rec in packet.additionals() {
        match rec.data() {
            Some(RData::SRV(srv)) => {
                hostname = srv.target().to_utf8().trim_end_matches('.').to_string();
                port = srv.port();
                name = rec.name().to_utf8().trim_end_matches('.').to_string();
            }
            Some(RData::TXT(t)) => {
                for d in t.txt_data() {
                    if let Ok(s) = std::str::from_utf8(d) {
                        txt.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    let payload = FoundDevice {
        name,
        hostname,
        addr: r.addr.ip().to_string(),
        port,
        txt,
    };

    app.emit(topic, payload)
}

fn cleanup(state: &MdnsState) {
    println!("Cleaning up mDNS services...");

    // Use a timeout to ensure cleanup doesn't hang
    let cleanup_timeout = std::time::Duration::from_secs(3);
    let start_time = std::time::Instant::now();

    let mut services_cleaned = 0;

    // Shutdown socket server
    if let Ok(mut socket_handle_guard) = state.socket_server_handle.lock() {
        if let Some(handle) = socket_handle_guard.take() {
            println!("Shutting down socket server...");
            handle.abort();
            println!("Socket server shut down successfully");
            services_cleaned += 1;
        } else {
            println!("No socket server to shut down");
        }
    }

    // Clear socket port
    *state.socket_server_port.lock().unwrap() = None;

    // Shutdown broadcaster
    if let Ok(mut broadcaster_guard) = state.broadcaster.lock() {
        if let Some(h) = broadcaster_guard.take() {
            println!("Shutting down broadcaster...");
            match h.shutdown() {
                Ok(_) => {
                    println!("Broadcaster shut down successfully");
                    services_cleaned += 1;
                }
                Err(e) => eprintln!("Error shutting down broadcaster: {}", e),
            }
        } else {
            println!("No broadcaster to shut down");
        }
    } else {
        eprintln!("Failed to acquire broadcaster lock for cleanup");
    }

    // Shutdown discovery
    if let Ok(mut discovery_guard) = state.discovery.lock() {
        if let Some(h) = discovery_guard.take() {
            println!("Shutting down discovery...");
            match h.shutdown() {
                Ok(_) => {
                    println!("Discovery shut down successfully");
                    services_cleaned += 1;
                }
                Err(e) => eprintln!("Error shutting down discovery: {}", e),
            }
        } else {
            println!("No discovery to shut down");
        }
    } else {
        eprintln!("Failed to acquire discovery lock for cleanup");
    }

    let elapsed = start_time.elapsed();
    println!(
        "mDNS cleanup completed in {:?} ({} services cleaned)",
        elapsed, services_cleaned
    );

    if elapsed > cleanup_timeout {
        eprintln!("Warning: Cleanup took longer than expected ({:?})", elapsed);
    }

    // Give extra time for goodbye messages to propagate across the network
    if services_cleaned > 0 {
        println!("Waiting for goodbye messages to propagate across network...");
        std::thread::sleep(std::time::Duration::from_millis(750));
        println!("Network cleanup delay completed");
    }
}

fn main() {
    let app = tauri::Builder::default()
        .manage(MdnsState::default())
        .setup(|app| {
            // Start socket server automatically when app starts
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Give the app a moment to fully initialize
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                let state: State<MdnsState> = app_handle.state();
                match start_socket_server(state).await {
                    Ok(port) => println!("Socket server auto-started on port: {}", port),
                    Err(e) => eprintln!("Failed to auto-start socket server: {}", e),
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { .. } => {
                println!("Window close requested - cleaning up mDNS services");
                let app_handle = window.app_handle();
                let state: State<MdnsState> = app_handle.state();
                cleanup(&state);
            }
            tauri::WindowEvent::Destroyed => {
                println!("Window destroyed - final cleanup");
                let app_handle = window.app_handle();
                let state: State<MdnsState> = app_handle.state();
                cleanup(&state);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            register_service,
            unregister_service,
            start_discovery,
            stop_discovery,
            get_service_status,
            force_cleanup,
            send_goodbye_message,
            start_socket_server,
            stop_socket_server,
            get_socket_server_status
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Set up cleanup on app exit
    let app_handle = app.handle().clone();
    std::panic::set_hook(Box::new(move |_| {
        println!("Panic detected - cleaning up mDNS services");
        let state: State<MdnsState> = app_handle.state();
        cleanup(&state);
    }));

    // Register signal handlers for graceful shutdown
    #[cfg(unix)]
    {
        use std::sync::Arc;
        let app_handle = app.handle().clone();
        let app_handle_arc = Arc::new(app_handle);

        let app_handle_sigint = app_handle_arc.clone();
        ctrlc::set_handler(move || {
            println!("Received SIGINT - cleaning up mDNS services");
            let state: State<MdnsState> = app_handle_sigint.state();
            cleanup(&state);
            std::process::exit(0);
        })
        .expect("Error setting Ctrl-C handler");
    }

    app.run(|_app_handle, event| match event {
        tauri::RunEvent::ExitRequested { .. } => {
            println!("Exit requested - cleaning up mDNS services");
            let state: State<MdnsState> = _app_handle.state();
            cleanup(&state);
        }
        _ => {}
    });
}
