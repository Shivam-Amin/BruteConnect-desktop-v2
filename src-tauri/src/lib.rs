// // src-tauri/src/lib.rs
// use serde::Serialize;
// use std::{
//     any::Any,
//     sync::{Arc, Mutex},
//     time::Duration,
//     thread,
// };
// use tauri::{AppHandle, Emitter, Manager};
// use zeroconf::prelude::*;
// use zeroconf::{MdnsBrowser, MdnsService, ServiceDiscovery, ServiceRegistration, ServiceType, TxtRecord};

// #[derive(Debug, Serialize, Clone)]
// struct DeviceInfo {
//     name: String,
//     ip: String,
//     port: u16,
//     txt: Vec<String>,
// }

// // Global state
// static DISCOVERED_DEVICES: Mutex<Vec<DeviceInfo>> = Mutex::new(Vec::new());
// static SERVICE_EVENT_LOOP: Mutex<Option<zeroconf::EventLoop>> = Mutex::new(None);
// static BROWSER_EVENT_LOOP: Mutex<Option<zeroconf::EventLoop>> = Mutex::new(None);
// static DISCOVERY_ACTIVE: Mutex<bool> = Mutex::new(false);
// static APP_HANDLE: Mutex<Option<AppHandle>> = Mutex::new(None);

// #[tauri::command]
// fn register_service(port: u16) -> Result<String, String> {
//     let hostname = format!("bruteconnect-{}", whoami::hostname());
    
//     // Create service type
//     let service_type = ServiceType::new("_mdnsconnect", "_udp").map_err(|e| e.to_string())?;
    
//     // Create the service
//     let mut service = MdnsService::new(service_type, port);
//     service.set_name(&hostname);
    
//     // Set TXT records
//     let mut txt_record = TxtRecord::new();
//     txt_record.insert("name", "bruteconnect").map_err(|e| e.to_string())?;
//     txt_record.insert("platform", "desktop").map_err(|e| e.to_string())?;
//     service.set_txt_record(txt_record);
    
//     // Set callback
//     service.set_registered_callback(Box::new(on_service_registered));
    
//     // Register the service
//     let event_loop = service.register().map_err(|e| e.to_string())?;
    
//     // Store event loop to keep service alive
//     {
//         let mut service_loop = SERVICE_EVENT_LOOP.lock().unwrap();
//         *service_loop = Some(event_loop);
//     }
    
//     // Start polling in background thread
//     thread::spawn(|| {
//         loop {
//             if let Some(event_loop) = SERVICE_EVENT_LOOP.lock().unwrap().as_ref() {
//                 if event_loop.poll(Duration::from_millis(100)).is_err() {
//                     break;
//                 }
//             } else {
//                 break;
//             }
//         }
//     });
    
//     println!("✅ Service registration initiated for: {} on port {}", hostname, port);
//     Ok(format!("Service registration initiated: {} on port {}", hostname, port))
// }

// #[tauri::command]
// async fn start_discovery(app_handle: AppHandle) -> Result<(), String> {
//     // Store app handle for callbacks
//     {
//         let mut handle = APP_HANDLE.lock().unwrap();
//         *handle = Some(app_handle);
//     }
    
//     // Check if already active
//     {
//         let mut active = DISCOVERY_ACTIVE.lock().unwrap();
//         if *active {
//             return Ok(());
//         }
//         *active = true;
//     }
    
//     // Clear previous devices
//     {
//         let mut devices = DISCOVERED_DEVICES.lock().unwrap();
//         devices.clear();
//     }
    
//     // Create service type for browsing
//     let service_type = ServiceType::new("bruteconnect", "tcp").map_err(|e| e.to_string())?;
    
//     // Create browser
//     let mut browser = MdnsBrowser::new(service_type);
//     browser.set_service_discovered_callback(Box::new(on_service_discovered));
    
//     // Start browsing
//     let event_loop = browser.browse_services().map_err(|e| e.to_string())?;
    
//     // Store event loop
//     {
//         let mut browser_loop = BROWSER_EVENT_LOOP.lock().unwrap();
//         *browser_loop = Some(event_loop);
//     }
    
//     // Start discovery polling in background thread with 10-second timeout
//     tokio::spawn(async move {
//         let start_time = std::time::Instant::now();
        
//         loop {
//             // Check timeout (10 seconds)
//             if start_time.elapsed() >= Duration::from_secs(10) {
//                 println!("🛑 Discovery timeout reached (10s)");
//                 break;
//             }
            
//             // Check if discovery should stop
//             {
//                 let active = DISCOVERY_ACTIVE.lock().unwrap();
//                 if !*active {
//                     println!("🛑 Discovery stopped manually");
//                     break;
//                 }
//             }
            
//             // Poll the browser
//             if let Some(event_loop) = BROWSER_EVENT_LOOP.lock().unwrap().as_ref() {
//                 if event_loop.poll(Duration::from_millis(100)).is_err() {
//                     println!("❌ Browser polling error");
//                     break;
//                 }
//             } else {
//                 break;
//             }
//         }
        
//         // Cleanup
//         {
//             let mut active = DISCOVERY_ACTIVE.lock().unwrap();
//             *active = false;
//         }
//         {
//             let mut browser_loop = BROWSER_EVENT_LOOP.lock().unwrap();
//             *browser_loop = None;
//         }
        
//         // Notify frontend that discovery stopped
//         if let Some(app_handle) = APP_HANDLE.lock().unwrap().as_ref() {
//             let _ = app_handle.emit("discovery-stopped", ());
//         }
        
//         println!("🛑 Discovery thread finished");
//     });
    
//     println!("🔍 Discovery started");
//     Ok(())
// }

// #[tauri::command]
// fn stop_discovery() -> Result<(), String> {
//     let mut active = DISCOVERY_ACTIVE.lock().unwrap();
//     *active = false;
//     println!("🛑 Discovery stop requested");
//     Ok(())
// }

// #[tauri::command]
// fn unregister_service() -> Result<(), String> {
//     // Stop service
//     {
//         let mut service_loop = SERVICE_EVENT_LOOP.lock().unwrap();
//         *service_loop = None;
//     }
//     println!("🧹 Service unregistered");
//     Ok(())
// }

// #[tauri::command]
// fn get_discovered_devices() -> Vec<DeviceInfo> {
//     let devices = DISCOVERED_DEVICES.lock().unwrap();
//     devices.clone()
// }

// #[tauri::command]
// fn is_discovery_active() -> bool {
//     *DISCOVERY_ACTIVE.lock().unwrap()
// }

// // Callback for service registration
// fn on_service_registered(
//     result: zeroconf::Result<ServiceRegistration>,
//     _context: Option<Arc<dyn Any>>,
// ) {
//     match result {
//         Ok(service) => {
//             println!("✅ Service registered successfully: {}", service.name());
//         }
//         Err(e) => {
//             println!("❌ Service registration failed: {:?}", e);
//         }
//     }
// }

// // Callback for service discovery
// fn on_service_discovered(
//     result: zeroconf::Result<ServiceDiscovery>,
//     _context: Option<Arc<dyn Any>>,
// ) {
//     match result {
//         Ok(service) => {
//             println!("🟢 Service discovered: {} at {}:{}", 
//                 service.name(), service.address(), service.port());
            
//             // Extract TXT records
//             let txt_records: Vec<String> = service.txt().clone()
//                 .map(|txt| {
//                     txt.iter()
//                         .map(|(key, value)| format!("{}={}", key, value))
//                         .collect()
//                 })
//                 .unwrap_or_default();
            
//             let device = DeviceInfo {
//                 name: service.name().to_string(),
//                 ip: service.address().to_string(),
//                 port: *service.port(),
//                 txt: txt_records,
//             };
            
//             // Add to discovered devices list
//             {
//                 let mut devices = DISCOVERED_DEVICES.lock().unwrap();
//                 // Check for duplicates
//                 if !devices.iter().any(|d| d.name == device.name && d.ip == device.ip) {
//                     devices.push(device.clone());
//                     println!("🟢 Added device: {:?}", device);
                    
//                     // Notify frontend
//                     if let Some(app_handle) = APP_HANDLE.lock().unwrap().as_ref() {
//                         let _ = app_handle.emit("device-discovered", &device);
//                     }
//                 }
//             }
//         }
//         Err(e) => {
//             println!("❌ Service discovery error: {:?}", e);
//         }
//     }
// }

// #[cfg_attr(mobile, tauri::mobile_entry_point)]
// pub fn run() {
//     tauri::Builder::default()
//         .plugin(tauri_plugin_opener::init())
//         .invoke_handler(tauri::generate_handler![
//             register_service,
//             unregister_service,
//             start_discovery,
//             stop_discovery,
//             get_discovered_devices,
//             is_discovery_active
//         ])
//         .setup(|app| {
//             // Listen for window close events
//             if let Some(window) = app.get_webview_window("main") {
//                 window.on_window_event(move |event| {
//                     if let tauri::WindowEvent::CloseRequested { .. } = event {
//                         println!("🧹 Application closing, cleaning up...");
//                         let _ = unregister_service();
//                         let _ = stop_discovery();
//                     }
//                 });
//             }
            
//             Ok(())
//         })
//         .run(tauri::generate_context!())
//         .expect("error while running tauri application");
// }