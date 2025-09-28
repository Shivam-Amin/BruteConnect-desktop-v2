import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// const SERVICE = "_bruteconnect._tcp.local.";
const SERVICE = "_mdnsconnect._udp.local.";

export default function App() {
  const [devices, setDevices] = useState([]);
  const [discovering, setDiscovering] = useState(false);
  const [advertising, setAdvertising] = useState(false);
  const [socketServerStatus, setSocketServerStatus] = useState({ running: false, port: null });

  useEffect(() => {
    const unsubs = [];

    const on = async (event, cb) => {
      const un = await listen(event, (e) => cb(e.payload));
      unsubs.push(() => un());
    };

    const key = (d) => `${d.hostname}:${d.port}`;

    const upsert = (list, item) => {
      const i = list.findIndex((d) => key(d) === key(item));
      if (i === -1) return [item, ...list];
      const copy = [...list];
      copy[i] = item;
      return copy;
    };

    on("mdns:found", (d) => setDevices((prev) => upsert(prev, d)));
    on("mdns:update", (d) => setDevices((prev) => upsert(prev, d)));
    on("mdns:lost", (d) =>
      setDevices((prev) => prev.filter((x) => key(x) !== key(d)))
    );

    // Check socket server status on startup
    const checkSocketStatus = async () => {
      try {
        const status = await invoke("get_socket_server_status");
        setSocketServerStatus(status);
      } catch (error) {
        console.error("Failed to get socket server status:", error);
      }
    };

    checkSocketStatus();
    
    // Poll socket server status every 2 seconds
    const statusInterval = setInterval(checkSocketStatus, 2000);

    // Cleanup function - this will run when component unmounts
    return () => {
      console.log("App component unmounting - cleaning up...");
      
      // Clear the status interval
      clearInterval(statusInterval);
      
      // Unsubscribe from events first
      unsubs.forEach((u) => u());
      
      // Force cleanup of mDNS services
      invoke("force_cleanup").catch(console.error);
      
      console.log("React cleanup completed");
    };
  }, []);

  // Note: Removed beforeunload event listener as cleanup is now handled 
  // properly in the Rust backend during window close events

  const startDiscovery = async () => {
    try {
      await invoke("start_discovery", { serviceType: SERVICE });
      setDiscovering(true);
      console.log("Discovery started");
    } catch (error) {
      console.error("Failed to start discovery:", error);
    }
  };

  const stopDiscovery = async () => {
    try {
      await invoke("stop_discovery");
      setDiscovering(false);
      setDevices([]); // Clear devices when stopping discovery
      console.log("Discovery stopped");
    } catch (error) {
      console.error("Failed to stop discovery:", error);
    }
  };

  const advertise = async () => {
    try {
      // Check if socket server is running
      if (!socketServerStatus.running) {
        alert("Socket server must be running before advertising. Please wait for it to start or restart the app.");
        return;
      }

      await invoke("register_service", {
        serviceType: SERVICE,
        instanceName: "BruteConnect-Desktop",
        port: 9001,
        txt: ["role=desktop"],
      });
      setAdvertising(true);
      console.log("Service registered");
    } catch (error) {
      console.error("Failed to register service:", error);
      alert(`Failed to register service: ${error}`);
    }
  };

  const unadvertise = async () => {
    try {
      await invoke("unregister_service");
      setAdvertising(false);
      console.log("Service unregistered");
    } catch (error) {
      console.error("Failed to unregister service:", error);
    }
  };

  const sendGoodbye = async () => {
    try {
      await invoke("send_goodbye_message");
      console.log("Goodbye message sent");
    } catch (error) {
      console.error("Failed to send goodbye message:", error);
    }
  };

  const startSocketServer = async () => {
    try {
      const port = await invoke("start_socket_server");
      console.log("Socket server started on port:", port);
      setSocketServerStatus({ running: true, port });
    } catch (error) {
      console.error("Failed to start socket server:", error);
      alert(`Failed to start socket server: ${error}`);
    }
  };

  const stopSocketServer = async () => {
    try {
      await invoke("stop_socket_server");
      console.log("Socket server stopped");
      setSocketServerStatus({ running: false, port: null });
    } catch (error) {
      console.error("Failed to stop socket server:", error);
    }
  };

  return (
    <div style={{ padding: "2rem", fontFamily: "sans-serif" }}>
      <h1>BruteConnect (mDNS demo)</h1>

      <div style={{ marginBottom: "1rem" }}>
        {discovering ? (
          <button onClick={stopDiscovery} style={{ backgroundColor: "#dc3545", color: "white", padding: "10px 15px", border: "none", borderRadius: "4px", marginRight: "10px" }}>
            Stop Discovery
          </button>
        ) : (
          <button onClick={startDiscovery} style={{ backgroundColor: "#007bff", color: "white", padding: "10px 15px", border: "none", borderRadius: "4px", marginRight: "10px" }}>
            Discover Devices
          </button>
        )}
        
        {advertising ? (
          <button onClick={unadvertise} style={{ backgroundColor: "#ffc107", color: "black", padding: "10px 15px", border: "none", borderRadius: "4px", marginRight: "10px" }}>
            Unregister Device
          </button>
        ) : (
          <button onClick={advertise} style={{ backgroundColor: "#28a745", color: "white", padding: "10px 15px", border: "none", borderRadius: "4px", marginRight: "10px" }}>
            Advertise This Device
          </button>
        )}
        
        <button onClick={sendGoodbye} style={{ backgroundColor: "#6c757d", color: "white", padding: "10px 15px", border: "none", borderRadius: "4px" }}>
          Send Goodbye
        </button>
      </div>

      <div style={{ marginBottom: "1rem", padding: "10px", backgroundColor: "#f8f9fa", borderRadius: "4px" }}>
        <strong>Status:</strong> 
        <span style={{ color: discovering ? "#28a745" : "#6c757d" }}>
          {discovering ? " 🔍 Discovering" : " ⏸️ Stopped"}
        </span>
        {" | "}
        <span style={{ color: advertising ? "#28a745" : "#6c757d" }}>
          {advertising ? " 📡 Advertising" : " 📴 Not Advertising"}
        </span>
        {" | "}
        <span style={{ color: socketServerStatus.running ? "#28a745" : "#dc3545" }}>
          {socketServerStatus.running ? ` 🔌 Socket Server: ${socketServerStatus.port}` : " 🔌 Socket Server: Stopped"}
        </span>
      </div>

      {/* Socket Server Controls */}
      <div style={{ marginBottom: "1rem", padding: "10px", backgroundColor: "#e9ecef", borderRadius: "4px" }}>
        <strong>Socket Server:</strong>
        {socketServerStatus.running ? (
          <button onClick={stopSocketServer} style={{ backgroundColor: "#dc3545", color: "white", padding: "5px 10px", border: "none", borderRadius: "4px", marginLeft: "10px" }}>
            Stop Server
          </button>
        ) : (
          <button onClick={startSocketServer} style={{ backgroundColor: "#28a745", color: "white", padding: "5px 10px", border: "none", borderRadius: "4px", marginLeft: "10px" }}>
            Start Server
          </button>
        )}
        <span style={{ marginLeft: "10px", fontSize: "12px", color: "#6c757d" }}>
          {socketServerStatus.running 
            ? `Running on port ${socketServerStatus.port}` 
            : "Required for device advertising"}
        </span>
      </div>

      <h2>Discovered Devices: ({devices.length})</h2>
      {devices.length === 0 ? (
        <div style={{ padding: "20px", textAlign: "center", color: "#6c757d" }}>
          {discovering ? "Searching for devices..." : "No devices found. Start discovery to search for devices."}
        </div>
      ) : (
        <ul style={{ listStyle: "none", padding: 0 }}>
          {devices.map((d) => (
            <li key={`${d.hostname}:${d.port}`} style={{ marginBottom: "10px", padding: "15px", backgroundColor: "#f8f9fa", borderRadius: "8px", border: "1px solid #dee2e6" }}>
              <div style={{ fontSize: "16px", fontWeight: "bold", color: "#333" }}>
                {d.name || d.hostname}
              </div>
              <div style={{ color: "#666", margin: "5px 0" }}>
                <strong>Address:</strong> {d.addr}:{d.port}
              </div>
              {d.txt.length > 0 && (
                <div style={{ marginTop: "8px" }}>
                  <strong>TXT Records:</strong>
                  <div style={{ marginTop: "4px" }}>
                    {d.txt.map((txt, i) => (
                      <span key={i} style={{ 
                        display: "inline-block", 
                        backgroundColor: "#e3f2fd", 
                        color: "#1976d2", 
                        fontSize: "12px", 
                        padding: "2px 6px", 
                        borderRadius: "4px", 
                        marginRight: "5px" 
                      }}>
                        {txt}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}



// import { invoke } from '@tauri-apps/api';
// import { listen } from '@tauri-apps/api';
// import { useEffect, useState } from 'react';

// export default function App() {
//   const [isDiscovering, setIsDiscovering] = useState(false);
//   const [devices, setDevices] = useState([]);
//   const [serviceRegistered, setServiceRegistered] = useState(false);
//   const [statusMessage, setStatusMessage] = useState('Initializing...');

//   useEffect(() => {
//     console.log('Component mounted, starting initialization...');
    
//     // Set up event listeners first
//     const setupEventListeners = async () => {
//       try {
//         // Listen for device discoveries
//         const unlistenDevices = await listen('device-discovered', (event) => {
//           console.log('Device discovered:', event.payload);
//           setDevices(prevDevices => {
//             const newDevice = event.payload;
//             // Check if device already exists to avoid duplicates
//             const exists = prevDevices.some(d => 
//               d.name === newDevice.name && d.ip === newDevice.ip
//             );
//             if (!exists) {
//               return [...prevDevices, newDevice];
//             }
//             return prevDevices;
//           });
//         });

//         // Listen for discovery stop events
//         const unlistenStop = await listen('discovery-stopped', () => {
//           console.log('Discovery stopped');
//           setIsDiscovering(false);
//           setStatusMessage('Discovery completed');
//         });

//         return { unlistenDevices, unlistenStop };
//       } catch (error) {
//         console.error('Error setting up event listeners:', error);
//         setStatusMessage(`Event listener error: ${error}`);
//         return null;
//       }
//     };

//     // Initialize the app
//     const initialize = async () => {
//       try {
//         setStatusMessage('Setting up event listeners...');
//         const listeners = await setupEventListeners();
        
//         if (!listeners) {
//           setStatusMessage('Failed to setup event listeners');
//           return null;
//         }

//         setStatusMessage('Registering service...');
//         console.log('Registering service...');
        
//         // Register service first
//         const result = await invoke('register_service', { port: 8080 });
//         setServiceRegistered(true);
//         setStatusMessage(result);
//         console.log('Service registered:', result);

//         // Start initial discovery
//         setStatusMessage('Starting initial discovery...');
//         console.log('Starting initial discovery...');
//         await startDiscovery();
        
//         return listeners;
//       } catch (error) {
//         console.error('Initialization error:', error);
//         setStatusMessage(`Error: ${error}`);
//         return null;
//       }
//     };

//     let cleanup = null;
//     initialize().then(listeners => {
//       cleanup = listeners;
//     });

//     // Cleanup on unmount
//     return () => {
//       console.log('Component unmounting, cleaning up...');
//       if (cleanup) {
//         cleanup.unlistenDevices();
//         cleanup.unlistenStop();
//       }
//       invoke('unregister_service').catch(console.error);
//       invoke('stop_discovery').catch(console.error);
//     };
//   }, []);

//   const startDiscovery = async () => {
//     try {
//       console.log('Starting discovery...');
//       setIsDiscovering(true);
//       setDevices([]); // Clear previous devices
//       setStatusMessage('Starting discovery...');
      
//       await invoke('start_discovery');
//       setStatusMessage('Discovery started - searching for 10 seconds...');
//       console.log('Discovery command sent');
//     } catch (error) {
//       console.error('Discovery error:', error);
//       setStatusMessage(`Discovery error: ${error}`);
//       setIsDiscovering(false);
//     }
//   };

//   const stopDiscovery = async () => {
//     try {
//       console.log('Stopping discovery...');
//       await invoke('stop_discovery');
//       setIsDiscovering(false);
//       setStatusMessage('Discovery stopped manually');
//     } catch (error) {
//       console.error('Stop discovery error:', error);
//     }
//   };

//   const refreshDevices = async () => {
//     try {
//       console.log('Refreshing devices...');
//       const discoveredDevices = await invoke('get_discovered_devices');
//       setDevices(discoveredDevices);
//       setStatusMessage(`Refreshed - ${discoveredDevices.length} devices found`);
//     } catch (error) {
//       console.error('Refresh devices error:', error);
//     }
//   };

//   // const containerStyle = {
//   //   padding: '20px',
//   //   maxWidth: '800px',
//   //   margin: '0 auto',
//   //   fontFamily: 'Arial, sans-serif'
//   // };

//   // const statusBoxStyle = {
//   //   marginBottom: '20px',
//   //   padding: '15px',
//   //   backgroundColor: '#f5f5f5',
//   //   borderRadius: '8px',
//   //   border: '1px solid #ddd'
//   // };

//   // const buttonStyle = {
//   //   padding: '10px 15px',
//   //   marginRight: '10px',
//   //   marginBottom: '10px',
//   //   border: 'none',
//   //   borderRadius: '4px',
//   //   cursor: 'pointer',
//   //   fontSize: '14px',
//   //   transition: 'background-color 0.2s'
//   // };

//   // const primaryButtonStyle = {
//   //   ...buttonStyle,
//   //   backgroundColor: isDiscovering ? '#ccc' : '#007bff',
//   //   color: 'white'
//   // };

//   // const dangerButtonStyle = {
//   //   ...buttonStyle,
//   //   backgroundColor: !isDiscovering ? '#ccc' : '#dc3545',
//   //   color: 'white'
//   // };

//   // const successButtonStyle = {
//   //   ...buttonStyle,
//   //   backgroundColor: '#28a745',
//   //   color: 'white'
//   // };

//   // const deviceBoxStyle = {
//   //   backgroundColor: 'white',
//   //   borderRadius: '8px',
//   //   boxShadow: '0 2px 4px rgba(0,0,0,0.1)',
//   //   border: '1px solid #ddd'
//   // };

//   // const deviceHeaderStyle = {
//   //   fontSize: '20px',
//   //   fontWeight: 'bold',
//   //   padding: '15px',
//   //   borderBottom: '1px solid #ddd',
//   //   margin: 0
//   // };

//   // const deviceItemStyle = {
//   //   padding: '15px',
//   //   borderBottom: '1px solid #eee'
//   // };

//   // const deviceNameStyle = {
//   //   fontSize: '16px',
//   //   fontWeight: 'bold',
//   //   color: '#333',
//   //   marginBottom: '5px'
//   // };

//   // const deviceInfoStyle = {
//   //   color: '#666',
//   //   marginBottom: '10px'
//   // };

//   // const txtRecordStyle = {
//   //   display: 'inline-block',
//   //   backgroundColor: '#e3f2fd',
//   //   color: '#1976d2',
//   //   fontSize: '12px',
//   //   padding: '4px 8px',
//   //   borderRadius: '4px',
//   //   marginRight: '5px',
//   //   marginBottom: '5px'
//   // };

//   // const loadingStyle = {
//   //   display: 'flex',
//   //   alignItems: 'center',
//   //   justifyContent: 'center',
//   //   padding: '40px',
//   //   color: '#666'
//   // };

//   // const spinnerStyle = {
//   //   width: '20px',
//   //   height: '20px',
//   //   border: '2px solid #f3f3f3',
//   //   borderTop: '2px solid #007bff',
//   //   borderRadius: '50%',
//   //   animation: 'spin 1s linear infinite',
//   //   marginRight: '10px'
//   // };

//   return (
//     <div style={containerStyle}>
//       <style>
//         {`
//           @keyframes spin {
//             0% { transform: rotate(0deg); }
//             100% { transform: rotate(360deg); }
//           }
//         `}
//       </style>
      
//       <h1 style={{ fontSize: '28px', fontWeight: 'bold', marginBottom: '20px', color: '#333' }}>
//         BruteConnect - Device Discovery
//       </h1>
      
//       {/* Status Section */}
//       <div style={statusBoxStyle}>
//         <h2 style={{ fontSize: '18px', fontWeight: 'bold', marginBottom: '10px' }}>Status</h2>
//         <p style={{ fontSize: '14px', color: '#666', margin: '5px 0' }}>
//           Service: {serviceRegistered ? '✅ Registered' : '❌ Not Registered'}
//         </p>
//         <p style={{ fontSize: '14px', color: '#666', margin: '5px 0' }}>
//           Discovery: {isDiscovering ? '🔍 Active' : '⏸️ Inactive'}
//         </p>
//         {statusMessage && (
//           <p style={{ fontSize: '14px', color: '#007bff', marginTop: '10px' }}>
//             {statusMessage}
//           </p>
//         )}
//       </div>

//       {/* Controls */}
//       <div style={{ marginBottom: '20px' }}>
//         <button
//           style={primaryButtonStyle}
//           onClick={startDiscovery}
//           disabled={isDiscovering}
//         >
//           {isDiscovering ? "Discovering..." : "Start Discovery (10s)"}
//         </button>
        
//         <button
//           style={dangerButtonStyle}
//           onClick={stopDiscovery}
//           disabled={!isDiscovering}
//         >
//           Stop Discovery
//         </button>
        
//         <button
//           style={successButtonStyle}
//           onClick={refreshDevices}
//         >
//           Refresh List
//         </button>
//       </div>

//       {/* Devices List */}
//       <div style={deviceBoxStyle}>
//         <h2 style={deviceHeaderStyle}>
//           Discovered Devices ({devices.length})
//         </h2>
        
//         {devices.length === 0 ? (
//           <div style={loadingStyle}>
//             {isDiscovering ? (
//               <>
//                 <div style={spinnerStyle}></div>
//                 Searching for devices...
//               </>
//             ) : (
//               "No devices found. Click 'Start Discovery' to search for devices."
//             )}
//           </div>
//         ) : (
//           <div>
//             {devices.map((device, index) => (
//               <div key={index} style={deviceItemStyle}>
//                 <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'start' }}>
//                   <div style={{ flex: 1 }}>
//                     <h3 style={deviceNameStyle}>
//                       {device.name}
//                     </h3>
//                     <p style={deviceInfoStyle}>
//                       <strong>Address:</strong> {device.ip}:{device.port}
//                     </p>
//                     {device.txt && device.txt.length > 0 && (
//                       <div>
//                         <span style={{ fontWeight: 'bold', color: '#333' }}>TXT Records:</span>
//                         <div style={{ marginTop: '5px' }}>
//                           {device.txt.map((txt, txtIndex) => (
//                             <span key={txtIndex} style={txtRecordStyle}>
//                               {txt}
//                             </span>
//                           ))}
//                         </div>
//                       </div>
//                     )}
//                   </div>
//                   <div style={{ marginLeft: '15px', textAlign: 'right' }}>
//                     <span style={{
//                       display: 'inline-block',
//                       width: '12px',
//                       height: '12px',
//                       backgroundColor: '#28a745',
//                       borderRadius: '50%'
//                     }}></span>
//                     <p style={{ fontSize: '12px', color: '#666', marginTop: '5px' }}>Online</p>
//                   </div>
//                 </div>
//               </div>
//             ))}
//           </div>
//         )}
//       </div>
//     </div>
//   );
// }