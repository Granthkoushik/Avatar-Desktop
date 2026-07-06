# Deployment & Configuration Guide

Follow these steps to deploy, configure, and compile the Avatar Desktop Ecosystem components.

---

## 1. Local AI Setup (Ollama)

Avatar relies on a local installation of **Ollama** for model inference to guarantee user privacy.

1. **Download Ollama**: Visit [ollama.com](https://ollama.com) and download the installer for your platform (Windows/macOS/Linux).
2. **Launch Ollama Service**: Run the installer and ensure that the server daemon is active. It binds to `http://127.0.0.1:11434` by default.
3. **Pull Target LLMs**:
   Open a terminal and pull the models you want to support (Llama 3 is the default model parsed by the system prompt):
   ```bash
   # Pull default model (Llama 3)
   ollama pull llama3
   
   # Optional configurations
   ollama pull qwen
   ollama pull gemma
   ollama pull phi
   ```
4. **Test Local REST API**: Verify that the API is up by executing a quick query in PowerShell:
   ```powershell
   Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:11434/api/chat" -Body '{"model": "llama3", "messages": [{"role": "user", "content": "hello"}], "stream": false}' -ContentType "application/json"
   ```

---

## 2. Compile & Launch Desktop Application (Tauri)

### Prerequisites
1. **Rust Toolchain**: Install rustup from [rustup.rs](https://rustup.rs/). Ensure `cargo` is in your environment variables path.
2. **Node.js**: Install Node.js (v18+) from [nodejs.org](https://nodejs.org/).
3. **Windows SDK**: Ensure standard MSVC Build Tools and Windows SDK are active.

### Steps
1. Navigate to the `avatar-desktop` directory:
   ```bash
   cd avatar-desktop
   ```
2. Install Node dependencies:
   ```bash
   npm install
   ```
3. Compile and launch the Tauri app in development/debug mode:
   ```bash
   npm run tauri dev
   ```
4. Build the production release binary:
   ```bash
   npm run tauri build
   ```
   *The installer bundle will be built inside `avatar-desktop/src-tauri/target/release/bundle/`.*

---

## 3. Configure & Compile Mobile Client (Flutter)

### Prerequisites
1. **Flutter SDK**: Install Flutter from [flutter.dev](https://flutter.dev/). Ensure the `flutter` CLI utility is in your PATH.
2. **Android Studio / Xcode**: Required for building android packages (.apk) and iOS packages (.ipa).

### Steps
1. Navigate to the `avatar-mobile` folder:
   ```bash
   cd avatar-mobile
   ```
2. Resolve packages:
   ```bash
   flutter pub get
   ```
3. Run the application on a connected device/emulator:
   ```bash
   flutter run
   ```
4. Build release packages:
   * **Android APK**:
     ```bash
     flutter build apk --release
     ```
   * **iOS IPA**:
     ```bash
     flutter build ipa --release
     ```

---

## 4. Secure Device Pairing Procedure

1. Launch the compiled **Avatar Desktop Application**.
2. Set a Master PIN/Passcode to initialize the SQLCipher-compatible SQLite database.
3. Click the **PAIR NEW DEVICE** button in the Security column on the right side of the screen.
4. A secure dialogue modal will display connection parameters (IP, Port, Certificate Fingerprint, and a temporary Pairing Token).
5. Open the **Avatar Mobile Client** on your phone, navigate past the pin screen, and type in these coordinates.
6. Press **ESTABLISH SECURE LINK**. The mobile client will verify the server fingerprint, send the token to authenticate the session, and register itself.
7. WebRTC peer connections will automatically spin up to start remote monitoring and stream screen coordinates.
