import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";
import "./App.css";

interface ProcessInfo {
  pid: number;
  name: string;
  memory_bytes: number;
  cpu_usage_pct: number;
}

interface GpuInfo {
  name: string;
  vram_dedicated_bytes: number;
}

interface SystemMetrics {
  cpu_usage_pct: number;
  ram_total_bytes: number;
  ram_used_bytes: number;
  disk_total_bytes: number;
  disk_used_bytes: number;
  battery_pct: number;
  is_charging: boolean;
  gpus: GpuInfo[];
  running_processes: ProcessInfo[];
}

interface PairingPayload {
  server_ip: string;
  port: number;
  fingerprint: string;
  token: string;
}

interface ChatMessage {
  sender: "user" | "assistant" | "system";
  text: string;
}

export default function App() {
  const [isLocked, setIsLocked] = useState(true);
  const [passcode, setPasscode] = useState("");
  const [lockError, setLockError] = useState("");
  const [chatInput, setChatInput] = useState("");
  const [chatHistory, setChatHistory] = useState<ChatMessage[]>([
    { sender: "system", text: "SYSTEM STATUS: SECURE CORE LOADED. STANDBY..." }
  ]);
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null);
  const [activeConnections, setActiveConnections] = useState<string[]>([]);
  const [isPairing, setIsPairing] = useState(false);
  const [pairingData, setPairingData] = useState<PairingPayload | null>(null);
  const [qrCodeUrl, setQrCodeUrl] = useState<string>("");
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [copiedToken, setCopiedToken] = useState(false);
  const [copiedIp, setCopiedIp] = useState(false);

  const handleCopyToken = () => {
    if (pairingData?.token) {
      navigator.clipboard.writeText(pairingData.token);
      setCopiedToken(true);
      setTimeout(() => setCopiedToken(false), 2000);
    }
  };

  const handleCopyIp = () => {
    if (pairingData?.server_ip) {
      navigator.clipboard.writeText(pairingData.server_ip);
      setCopiedIp(true);
      setTimeout(() => setCopiedIp(false), 2000);
    }
  };

  const consoleEndRef = useRef<HTMLDivElement | null>(null);

  // Auto-scroll console histories
  useEffect(() => {
    consoleEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chatHistory]);

  // Telemetry loop after unlock
  useEffect(() => {
    if (isLocked) return;

    // Fetch initial metrics
    fetchMetrics();
    
    // Poll metrics every 2.5 seconds
    const interval = setInterval(fetchMetrics, 2500);
    return () => clearInterval(interval);
  }, [isLocked]);

  // Keyboard typing listener for passcode on lock screen
  useEffect(() => {
    if (!isLocked) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (document.activeElement?.tagName === "INPUT" || document.activeElement?.tagName === "TEXTAREA") {
        return;
      }

      if (e.key >= "0" && e.key <= "9") {
        setLockError("");
        setPasscode(prev => {
          if (prev.length < 4) {
            return prev + e.key;
          }
          return prev;
        });
      } else if (e.key === "Backspace") {
        setLockError("");
        setPasscode(prev => prev.slice(0, -1));
      } else if (e.key === "Enter") {
        handleUnlock();
      } else if (e.key === "Escape" || e.key === "Delete") {
        setPasscode("");
        setLockError("");
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isLocked, passcode]);

  const fetchMetrics = async () => {
    try {
      const data: SystemMetrics = await invoke("get_telemetry");
      setMetrics(data);
      const conns: string[] = await invoke("get_active_connections");
      setActiveConnections(conns);
    } catch (e) {
      console.error("Failed to query telemetry:", e);
    }
  };

  const handleKeyPress = (num: string) => {
    setLockError("");
    if (passcode.length < 4) {
      setPasscode(prev => prev + num);
    }
  };

  const handleClear = () => {
    setPasscode("");
    setLockError("");
  };

  const handleUnlock = async () => {
    console.log("handleUnlock called, passcode length =", passcode.length);
    if (passcode.length === 0) return;
    try {
      console.log("Invoking unlock_database command...");
      const result: string = await invoke("unlock_database", { passcode });
      console.log("unlock_database resolved with result =", result);
      if (result === "unlocked" || result === "already_unlocked") {
        setIsLocked(false);
        setChatHistory(prev => [
          ...prev,
          { sender: "system", text: "DECRYPTION KEY ACCEPTED. DATA VAULT UNLOCKED." }
        ]);
        
        // Auto start streaming server on port 8086
        await invoke("start_streaming", { port: 8086 });
      } else {
        console.warn("Unexpected result from unlock_database:", result);
      }
    } catch (e) {
      console.error("unlock_database threw an error:", e);
      setLockError(String(e));
      setPasscode("");
    }
  };

  const triggerPairing = async () => {
    try {
      const data: PairingPayload = await invoke("get_pairing_payload");
      setPairingData(data);
      
      const qrPayload = JSON.stringify({
        ip: data.server_ip,
        port: data.port,
        token: data.token,
        fingerprint: data.fingerprint,
      });
      const qrUrl = await QRCode.toDataURL(qrPayload, { margin: 2, width: 256 });
      setQrCodeUrl(qrUrl);
      
      setIsPairing(true);
      setChatHistory(prev => [
        ...prev,
        { sender: "system", text: `PAIRING SIGNAL GENERATED ON PORT ${data.port}. READY FOR SECURE HANDSHAKE.` }
      ]);
    } catch (e) {
      setChatHistory(prev => [
        ...prev,
        { sender: "system", text: `PAIRING FAILED: ${e}` }
      ]);
    }
  };

  const triggerKillSwitch = async () => {
    if (confirm("CRITICAL WARNING: This will immediately drop all paired devices and secure the system. Proceed?")) {
      try {
        const msg: string = await invoke("trigger_emergency_kill");
        alert(msg);
        setIsLocked(true);
        setPasscode("");
        setMetrics(null);
        setIsPairing(false);
        setChatHistory([{ sender: "system", text: "SYSTEM LOCKDOWN ENFORCED. ALL SECTOR CONNECTIONS SEVERED." }]);
      } catch (e) {
        console.error("Kill switch failed:", e);
      }
    }
  };

  const sendTextCommand = async () => {
    if (!chatInput.trim()) return;
    const cmd = chatInput;
    setChatInput("");
    
    // Add user line to console
    setChatHistory(prev => [...prev, { sender: "user", text: `> ${cmd}` }]);
    
    try {
      setIsSpeaking(true);
      const reply: string = await invoke("send_command", { text: cmd, sessionId: "desktop_direct_session" });
      setChatHistory(prev => [...prev, { sender: "assistant", text: reply }]);
    } catch (e) {
      setChatHistory(prev => [...prev, { sender: "system", text: `CORE EXCEPTION: ${e}` }]);
    } finally {
      setIsSpeaking(false);
    }
  };

  // Convert bytes to GB/MB
  const formatBytes = (bytes: number) => {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  };

  if (isLocked) {
    return (
      <div className="lock-screen">
        <div className="glass-panel lock-box">
          <div className="lock-logo">AVATAR</div>
          <div className="pin-display">
            {"*".repeat(passcode.length) || "ENTER ACCESS PIN"}
          </div>
          
          <div className="pin-keypad">
            {["1", "2", "3", "4", "5", "6", "7", "8", "9"].map(n => (
              <button key={n} className="keypad-btn" onClick={() => handleKeyPress(n)}>{n}</button>
            ))}
            <button className="keypad-btn" style={{ color: "var(--accent-red)" }} onClick={handleClear}>CLR</button>
            <button className="keypad-btn" onClick={() => handleKeyPress("0")}>0</button>
            <button className="keypad-btn" style={{ color: "var(--accent-green)" }} onClick={handleUnlock}>ENTER</button>
          </div>
          <div className="error-text">{lockError}</div>
        </div>
      </div>
    );
  }

  // Calculate dashes for circular loaders
  const getStrokeDash = (pct: number) => {
    const radius = 16;
    const circumference = 2 * Math.PI * radius;
    const dashValue = (pct / 100) * circumference;
    return `${dashValue} ${circumference}`;
  };

  return (
    <div className="app-container">
      {/* Header status bar */}
      <header className="glass-panel app-header">
        <div className="logo-section">
          <div className="avatar-sphere"></div>
          <div className="logo-text">AVATAR SYSTEM</div>
        </div>
        <div className="status-badge">
          <span className="status-dot"></span>
          <span>SECURE SECURE LAYER ONLINE</span>
        </div>
      </header>

      {/* Main dashboard grid */}
      <div className="dashboard-grid">
        
        {/* Left column: system metrics */}
        <section className="glass-panel sidebar-panel metrics-section">
          <h2 className="section-title">Telemetry</h2>
          
          {/* CPU loader */}
          <div className="metric-row">
            <div className="metric-meta">
              <span className="metric-label">CPU Usage</span>
              <span className="metric-value">{metrics ? metrics.cpu_usage_pct.toFixed(1) : "0.0"} %</span>
            </div>
            <div className="radial-progress-container">
              <svg viewBox="0 0 36 36" className="circular-chart">
                <path className="circle-bg" d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
                <path className="circle cyan" strokeDasharray={getStrokeDash(metrics ? metrics.cpu_usage_pct : 0)} d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
              </svg>
            </div>
          </div>

          {/* RAM loader */}
          <div className="metric-row">
            <div className="metric-meta">
              <span className="metric-label">Memory Used</span>
              <span className="metric-value">
                {metrics ? `${formatBytes(metrics.ram_used_bytes)} / ${formatBytes(metrics.ram_total_bytes)}` : "0.0 MB"}
              </span>
            </div>
            <div className="radial-progress-container">
              <svg viewBox="0 0 36 36" className="circular-chart">
                <path className="circle-bg" d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
                <path className="circle purple" strokeDasharray={getStrokeDash(metrics ? (metrics.ram_used_bytes / metrics.ram_total_bytes) * 100 : 0)} d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
              </svg>
            </div>
          </div>

          {/* Disk storage loader */}
          <div className="metric-row">
            <div className="metric-meta">
              <span className="metric-label">Storage Capacity</span>
              <span className="metric-value">
                {metrics ? `${formatBytes(metrics.disk_used_bytes)} / ${formatBytes(metrics.disk_total_bytes)}` : "0 GB"}
              </span>
            </div>
            <div className="radial-progress-container">
              <svg viewBox="0 0 36 36" className="circular-chart">
                <path className="circle-bg" d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
                <path className="circle green" strokeDasharray={getStrokeDash(metrics ? (metrics.disk_used_bytes / metrics.disk_total_bytes) * 100 : 0)} d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831" />
              </svg>
            </div>
          </div>

          {/* Battery */}
          <div className="metric-row">
            <div className="metric-meta">
              <span className="metric-label">Battery Level</span>
              <span className="metric-value">
                {metrics ? `${metrics.battery_pct.toFixed(0)}%` : "100%"} {metrics?.is_charging && "(CHARGING)"}
              </span>
            </div>
          </div>

          {/* GPU Hardware Adapter info */}
          {metrics && metrics.gpus.length > 0 && (
            <div className="sec-widget" style={{ marginTop: "15px" }}>
              <div className="sec-widget-title">GPU Adapters</div>
              <div style={{ fontSize: "0.85rem", color: "var(--accent-cyan)" }}>
                {metrics.gpus[0].name}
              </div>
              <div style={{ fontSize: "0.8rem", color: "var(--text-secondary)", marginTop: "3px" }}>
                Dedicated VRAM: {formatBytes(metrics.gpus[0].vram_dedicated_bytes)}
              </div>
            </div>
          )}
        </section>

        {/* Center column: AI chat console & waveform */}
        <div className="center-panel">
          <section className="glass-panel hologram-visualizer">
            <div className="hologram-avatar"></div>
            {isSpeaking ? (
              <div className="audio-waveform">
                <div className="waveform-bar"></div>
                <div className="waveform-bar"></div>
                <div className="waveform-bar"></div>
                <div className="waveform-bar"></div>
                <div className="waveform-bar"></div>
                <div className="waveform-bar"></div>
              </div>
            ) : (
              <span style={{ fontSize: "0.85rem", color: "var(--text-secondary)" }}>AI ONLINE - STANDBY</span>
            )}
          </section>

          <section className="glass-panel chat-console">
            <div className="console-history">
              {chatHistory.map((msg, idx) => (
                <div key={idx} className={`history-line ${msg.sender}`}>
                  {msg.text}
                </div>
              ))}
              <div ref={consoleEndRef} />
            </div>

            <div className="console-input-row">
              <input 
                type="text" 
                className="console-input" 
                placeholder="INPUT COMMAND OR QUESTION..."
                value={chatInput}
                onChange={e => setChatInput(e.target.value)}
                onKeyDown={e => e.key === "Enter" && sendTextCommand()}
              />
              <button className="btn-send" onClick={sendTextCommand}>SUBMIT</button>
            </div>
          </section>
        </div>

        {/* Right column: security configurations */}
        <section className="glass-panel security-panel">
          <h2 className="section-title">Security & Devices</h2>
          
          <div className="sec-widget">
            <div className="sec-widget-title">Mobile Pairing</div>
            <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)", marginBottom: "12px" }}>
              Sync and pair your Android/iOS mobile application over a secure mutual TLS bridge.
            </p>
            <button className="btn-action" onClick={triggerPairing}>PAIR NEW DEVICE</button>
          </div>

          <div className="sec-widget">
            <div className="sec-widget-title">Active Connections</div>
            {activeConnections.length > 0 ? (
              <ul style={{ margin: 0, paddingLeft: "15px", fontSize: "0.85rem", color: "var(--accent-cyan)", listStyleType: "square" }}>
                {activeConnections.map((conn, idx) => (
                  <li key={idx}>{conn}</li>
                ))}
              </ul>
            ) : (
              <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)" }}>
                No active streaming connections.
              </p>
            )}
          </div>

          <button className="btn-kill" onClick={triggerKillSwitch}>EMERGENCY KILL SWITCH</button>
        </section>

      </div>

      {/* Pairing QR dialog modal */}
      {isPairing && pairingData && (
        <div className="modal-backdrop" onClick={() => { setIsPairing(false); setQrCodeUrl(""); }}>
          <div className="glass-panel modal-content" onClick={e => e.stopPropagation()}>
            <h3 className="section-title" style={{ marginBottom: "15px" }}>Secure Handshake</h3>
            
            <div className="qr-box" style={{ 
              border: "2px solid var(--accent-cyan)", 
              boxShadow: "0 0 20px rgba(0,243,255,0.4)",
              background: "white",
              padding: "10px",
              borderRadius: "8px",
              display: "flex",
              justifyContent: "center",
              alignItems: "center",
              margin: "15px auto",
              width: "220px",
              height: "220px"
            }}>
              {qrCodeUrl ? (
                <img src={qrCodeUrl} alt="Pairing QR Code" style={{ width: "200px", height: "200px", display: "block" }} />
              ) : (
                <span style={{ color: "#333", fontSize: "0.85rem" }}>Generating QR Code...</span>
              )}
            </div>

            <div className="pair-meta">
              <div style={{ 
                fontSize: "0.85rem",
                display: "flex",
                alignItems: "center",
                gap: "8px",
                justifyContent: "center",
                marginBottom: "4px"
              }}>
                <span>IP: {pairingData.server_ip}</span>
                <button 
                  onClick={handleCopyIp}
                  style={{
                    background: copiedIp ? "rgba(0, 243, 255, 0.2)" : "rgba(255, 255, 255, 0.1)",
                    border: copiedIp ? "1px solid var(--accent-cyan)" : "1px solid rgba(255, 255, 255, 0.2)",
                    borderRadius: "4px",
                    color: "var(--accent-cyan)",
                    cursor: "pointer",
                    padding: "2px 6px",
                    fontSize: "0.65rem",
                    transition: "all 0.2s ease",
                    flexShrink: 0
                  }}
                >
                  {copiedIp ? "COPIED" : "COPY"}
                </button>
              </div>
              <div style={{ fontSize: "0.85rem", marginBottom: "4px" }}>PORT: {pairingData.port}</div>
              <div style={{ 
                fontSize: "0.75rem", 
                color: "var(--text-secondary)", 
                marginTop: "5px",
                display: "flex",
                alignItems: "center",
                gap: "8px",
                justifyContent: "center"
              }}>
                <span style={{ wordBreak: "break-all" }}>TOKEN: {pairingData.token}</span>
                <button 
                  onClick={handleCopyToken}
                  style={{
                    background: copiedToken ? "rgba(0, 243, 255, 0.2)" : "rgba(255, 255, 255, 0.1)",
                    border: copiedToken ? "1px solid var(--accent-cyan)" : "1px solid rgba(255, 255, 255, 0.2)",
                    borderRadius: "4px",
                    color: "var(--accent-cyan)",
                    cursor: "pointer",
                    padding: "2px 6px",
                    fontSize: "0.65rem",
                    transition: "all 0.2s ease",
                    flexShrink: 0
                  }}
                >
                  {copiedToken ? "COPIED" : "COPY"}
                </button>
              </div>
            </div>
            
            <button className="btn-action" style={{ marginTop: "15px" }} onClick={() => { setIsPairing(false); setQrCodeUrl(""); }}>CLOSE</button>
          </div>
        </div>
      )}

    </div>
  );
}
