// //src-tauri/src/main.rs
// // Prevents additional console window on Windows in release, DO NOT REMOVE!!
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// fn main() {
//     bruteconnect_desktop_lib::run()
// }
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{net::IpAddr, sync::Mutex};
use tauri::Emitter;

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
    // Store service info for potential goodbye messages before consuming txt
    let service_info = ServiceInfo {
        service_type: service_type.clone(),
        instance_name: instance_name.clone(),
        port,
        txt: txt.clone(),
    };

    for rec in txt {
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
        println!("Sending goodbye for service: {} ({})", info.instance_name, info.service_type);
        
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
        .setup(|_app| Ok(()))
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
            send_goodbye_message
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
