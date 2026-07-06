# Threat Model & Security Review

This document provides a comprehensive security review and threat model of the Avatar Desktop Ecosystem, outlining potential attack vectors, mitigation strategies, and architectural details of our cryptographic layers.

---

## 1. System Threat Model

We assume a zero-trust architecture. The host operating system may reside on a compromised network (hostile LAN, public Wi-Fi, or behind CGNAT) and could be subject to passive sniffing, active packet injection, or unauthorized access attempts.

### Threat Matrix

| Threat ID | Threat Description | Attack Vector | Risk | Mitigation |
| :--- | :--- | :--- | :--- | :--- |
| **T-1** | **Man-in-the-Middle (MITM) Screen Snipping** | Attacker intercepts signaling payload or WebRTC stream to capture desktop frame buffers. | **Critical** | **mTLS 1.3 Signaling + WebRTC DTLS-SRTP**. Fingerprints of self-signed server and client certificates are verified and pinned during the physical QR pairing phase. The relay server cannot read the stream as it lacks keying material. |
| **T-2** | **Replay Action Injection** | Attacker captures a valid control WebSocket frame and replays it to click elements or run shell actions. | **High** | **Challenge-Response tokens**. In addition to WebSockets running over TLS, WebRTC Data Channels use unique message index sequences. Sessions expire automatically, preventing outdated command replays. |
| **T-3** | **Unauthorized Desktop Control** | Attacker guesses pairing PIN or websocket port and injects commands. | **Critical** | **Argon2id Pairing Authentication**. Connections are dropped instantly if the session token is missing or invalid in the encrypted SQLite database. Dangerous operations (restart, shell executables) are blocked by the Intent Dispatcher and require manual confirmation. |
| **T-4** | **Local Database Vault Extraction** | Attacker copies SQLite database file from local storage to decrypt messages, tokens, and preferences. | **High** | **Argon2id + AES-256-GCM Row Encryption**. Sensitive data fields (chat history, short-term cache, facts) are encrypted at the application level before database insertion. The 256-bit encryption key is derived dynamically from user PIN and kept in RAM, never touching the disk. |
| **T-5** | **Audit Log Tampering** | Attacker modifies or deletes audit logs in SQLite to cover their tracks after malicious execution. | **Medium** | **HMAC-SHA256 Cryptographic Log Chaining**. Every log entry signature depends on the content of the current log combined with the signature of the previous log entry. The system validates the entire chain on startup; any deletion/modification alerts the user. |

---

## 2. Cryptographic Details

### Key Derivation Function (KDF)
We use **Argon2id** (via the `argon2` crate) to derive a 256-bit key from the user passcode/PIN.
- **Salt**: 128-bit random salt, stored in local configuration files.
- **Argon2 Parameters**: Default memory/time constraints configured for consumer devices to prevent brute-force attacks while loading database connections under 500ms.

### Authenticated Encryption (AEAD)
All columns containing sensitive data are encrypted using **AES-256-GCM** (via the `aes-gcm` crate).
- **Key Size**: 256 bits (derived via KDF).
- **Nonce**: 96-bit (12-byte) randomly generated cryptographically strong nonce per column row. Nonce is prepended to the ciphertext before base64 encoding.

### Audit Log Signatures
Chained logs are signed via **HMAC-SHA256** using the `ring::hmac` library:
- **Key**: Derived dynamically on startup from user PIN.
- **Data Block**: `timestamp + event_type + description + previous_signature`.

---

## 3. Anti-Tampering & Execution Protections

### Windows Anti-Debugger
To detect memory inspection and active tampering during process execution, we implement check routines using Win32 API functions:
```rust
use windows::Win32::System::Diagnostics::Debug::IsDebuggerPresent;

pub fn detect_debugging() -> bool {
    unsafe { IsDebuggerPresent().as_bool() }
}
```
If debugging is detected, the system will write an alert to the secure audit log with severity `WARN` and limit sensitive memory structures from being exposed.
