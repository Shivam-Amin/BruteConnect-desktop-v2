// //src-tauri/src/main.rs
// // Prevents additional console window on Windows in release, DO NOT REMOVE!!
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// fn main() {
//     bruteconnect_desktop_lib::run()
// }
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{net::IpAddr, sync::Mutex};
use tauri::{Emitter, Listener};

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
}

// Collect non-loopback IPs so we can advertise the service.
fn local_ips() -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            // Skip loopback
            if iface.is_loopback() { continue; }
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
    service_type: String,   // e.g. "_bruteconnect._tcp.local."
    instance_name: String,  // e.g. "BruteConnect-1234"
    port: u16,              // e.g. 9000
    txt: Vec<String>,       // e.g. ["role=desktop"]
) -> Result<(), String> {
    println!("Registering service: {} as {} on port {}", service_type, instance_name, port);
    
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
    
    println!("Service registration completed successfully");
    Ok(())
}

#[tauri::command]
fn unregister_service(state: State<MdnsState>) -> Result<(), String> {
    if let Some(handle) = state.broadcaster.lock().unwrap().take() {
        handle
            .shutdown()
            .map_err(|e| format!("broadcast shutdown failed: {e}"))?;
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
            }
            // Fixed: Remove unreachable pattern since all enum variants are covered above
        });

    *state.discovery.lock().unwrap() = Some(discovery);
    Ok(())
}

#[tauri::command]
fn stop_discovery(state: State<MdnsState>) -> Result<(), String> {
    if let Some(handle) = state.discovery.lock().unwrap().take() {
        handle
            .shutdown()
            .map_err(|e| format!("discovery shutdown failed: {e}"))?;
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
    
    if let Ok(mut broadcaster_guard) = state.broadcaster.lock() {
        if let Some(h) = broadcaster_guard.take() {
            println!("Shutting down broadcaster...");
            if let Err(e) = h.shutdown() {
                eprintln!("Error shutting down broadcaster: {}", e);
            } else {
                println!("Broadcaster shut down successfully");
            }
        }
    }
    
    if let Ok(mut discovery_guard) = state.discovery.lock() {
        if let Some(h) = discovery_guard.take() {
            println!("Shutting down discovery...");
            if let Err(e) = h.shutdown() {
                eprintln!("Error shutting down discovery: {}", e);
            } else {
                println!("Discovery shut down successfully");
            }
        }
    }
    
    println!("mDNS cleanup completed");
}

fn main() {
    tauri::Builder::default()
        .manage(MdnsState::default())
        .setup(|app| {
            let app_handle = app.handle().clone();
            
            // Listen for window close events
            let app_handle_clone = app_handle.clone();
            app.listen("tauri://close-requested", move |_event| {
                println!("App close requested - cleaning up mDNS services");
                let state: State<MdnsState> = app_handle_clone.state();
                cleanup(&state);
            });

            // Listen for app exit events  
            let app_handle_clone2 = app_handle.clone();
            app.listen("tauri://exit", move |_event| {
                println!("App exit - cleaning up mDNS services");
                let state: State<MdnsState> = app_handle_clone2.state();
                cleanup(&state);
            });

            Ok(())
        })
        .on_window_event(|_window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { .. } => {
                    println!("Window close requested");
                    // Additional cleanup can be done here if needed
                }
                tauri::WindowEvent::Destroyed => {
                    println!("Window destroyed");
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            register_service,
            unregister_service,
            start_discovery,
            stop_discovery
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}