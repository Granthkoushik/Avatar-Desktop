import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'dart:convert';
import 'dart:async';
import 'package:encrypt/encrypt.dart' as enc;
import 'package:crypto/crypto.dart';
import '../main.dart';
import 'dashboard.dart';

class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key});

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final _ipController = TextEditingController(text: '192.168.1.10');
  final _portController = TextEditingController(text: '8086');
  final _tokenController = TextEditingController();
  final _fingerprintController = TextEditingController();
  final _passcodeController = TextEditingController();
  bool _isConnecting = false;
  String _errorText = '';

  Future<void> _handlePairing() async {
    var ip = _ipController.text.trim();
    if (ip.startsWith('https://')) {
      ip = ip.substring(8);
    } else if (ip.startsWith('http://')) {
      ip = ip.substring(7);
    } else if (ip.startsWith('wss://')) {
      ip = ip.substring(6);
    } else if (ip.startsWith('ws://')) {
      ip = ip.substring(5);
    }
    if (ip.endsWith('/')) {
      ip = ip.substring(0, ip.length - 1);
    }

    final port = _portController.text.trim();
    final token = _tokenController.text.trim();
    final fingerprint = _fingerprintController.text.trim();
    final passcode = _passcodeController.text.trim();

    if (ip.isEmpty || port.isEmpty || token.isEmpty || passcode.isEmpty) {
      setState(() {
        _errorText = 'IP, PORT, TOKEN, AND PASSCODE REQUIRED';
      });
      return;
    }

    setState(() {
      _isConnecting = true;
      _errorText = '';
    });

    try {
      // Establish WebSocket connection to Desktop signaling server
      final scheme = ip.contains(RegExp(r'[a-zA-Z]')) ? 'wss' : 'ws';
      final wsUrl = Uri.parse('$scheme://$ip:$port');
      final channel = WebSocketChannel.connect(wsUrl);
      
      // Send the pairing token as the first authentication payload
      channel.sink.add(token);
      
      final streamIterator = StreamIterator(channel.stream);
      
      // Wait for server's handshake challenge (NEED_AUTH)
      if (!await streamIterator.moveNext().timeout(const Duration(seconds: 15), onTimeout: () => throw Exception('Handshake challenge timeout'))) {
        throw Exception('No response from host');
      }
      final challenge = streamIterator.current;
      
      if (challenge == 'NEED_AUTH') {
        // Derive session key: SHA-256 hash of passcode
        final passcodeBytes = utf8.encode(passcode);
        final passcodeHash = sha256.convert(passcodeBytes).toString();
        
        // Encrypt authentication message
        final authPayload = jsonEncode({'action': 'authenticate', 'params': {}});
        final encryptedAuth = encryptAESGCM(authPayload, passcodeHash);
        
        // Send encrypted auth
        channel.sink.add(encryptedAuth);
        
        // Wait for response
        if (!await streamIterator.moveNext().timeout(const Duration(seconds: 15), onTimeout: () => throw Exception('Handshake verification timeout'))) {
          throw Exception('No response to auth request');
        }
        final encResponse = streamIterator.current;
        
        // Decrypt response
        try {
          final decryptedResponse = decryptAESGCM(encResponse, passcodeHash);
          if (decryptedResponse.startsWith('AUTH_OK')) {
            // Pairing successful!
            final prefs = await SharedPreferences.getInstance();
            await prefs.setString('server_ip', ip);
            await prefs.setInt('server_port', int.parse(port));
            await prefs.setString('session_token', token);
            await prefs.setString('server_fingerprint', fingerprint);
            await prefs.setString('secure_passcode', passcode);
            
            // Save local passcode hash for local app lock
            final pinHash = sha256.convert(passcodeBytes).toString();
            await prefs.setString('local_pin_hash', pinHash);
            globalPasscode = passcode; // Store in memory
            
            if (mounted) {
              Navigator.of(context).pushReplacement(
                MaterialPageRoute(builder: (_) => const DashboardScreen()),
              );
            }
          } else {
            throw Exception('Auth rejected by host');
          }
        } catch (e) {
          throw Exception('Passcode verification failed');
        }
      } else {
        throw Exception('Invalid handshake challenge: $challenge');
      }
      
      await channel.sink.close();
    } catch (e) {
      setState(() {
        _errorText = 'CONNECTION EXCEPTION: ${e.toString()}';
      });
    } finally {
      setState(() {
        _isConnecting = false;
      });
    }
  }

  Future<void> _startQrScan() async {
    final result = await Navigator.of(context).push<String>(
      MaterialPageRoute(builder: (_) => const QrScannerScreen()),
    );
    if (result != null && result.isNotEmpty) {
      try {
        final Map<String, dynamic> data = jsonDecode(result);
        setState(() {
          _ipController.text = data['ip'] ?? '';
          _portController.text = (data['port'] ?? '').toString();
          _tokenController.text = data['token'] ?? '';
          _fingerprintController.text = data['fingerprint'] ?? '';
          _errorText = '';
        });
        // Auto pairing initiation if passcode is already entered
        if (_passcodeController.text.trim().isNotEmpty) {
          _handlePairing();
        } else {
          setState(() {
            _errorText = 'ENTER DESKTOP PASSCODE TO COMPLETE LINK';
          });
        }
      } catch (e) {
        setState(() {
          _errorText = 'FAILED TO PARSE QR DETAILS: $e';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('PAIRING CORE', style: TextStyle(fontWeight: FontWeight.bold, letterSpacing: 1.5)),
        backgroundColor: Colors.transparent,
        elevation: 0,
      ),
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Container(
            width: MediaQuery.of(context).size.width * 0.9,
            padding: const EdgeInsets.all(24),
            decoration: BoxDecoration(
              color: const Color(0xFF0A0B14).withOpacity(0.7),
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: Colors.white.withOpacity(0.07)),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text(
                  'LINK ASSISTANT DEVICE',
                  style: TextStyle(fontSize: 18, fontWeight: FontWeight.bold, letterSpacing: 1),
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 8),
                Text(
                  'Type properties generated by the desktop client pairing tab.',
                  style: TextStyle(color: Colors.white.withOpacity(0.5), fontSize: 12),
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 20),
                ElevatedButton.icon(
                  onPressed: _startQrScan,
                  icon: const Icon(Icons.qr_code_scanner),
                  label: const Text('SCAN PAIRING QR CODE'),
                  style: ElevatedButton.styleFrom(
                    padding: const EdgeInsets.symmetric(vertical: 14),
                    backgroundColor: Colors.transparent,
                    foregroundColor: const Color(0xFF00F3FF),
                    side: const BorderSide(color: Color(0xFF00F3FF), width: 1.5),
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
                  ),
                ),
                const SizedBox(height: 20),
                Row(
                  children: [
                    Expanded(child: Divider(color: Colors.white.withOpacity(0.1))),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 10),
                      child: Text('OR CONNECT MANUALLY', style: TextStyle(color: Colors.white.withOpacity(0.3), fontSize: 10, letterSpacing: 1)),
                    ),
                    Expanded(child: Divider(color: Colors.white.withOpacity(0.1))),
                  ],
                ),
                const SizedBox(height: 20),
                TextField(
                  controller: _ipController,
                  decoration: const InputDecoration(
                    labelText: 'HOST SERVER IP ADDRESS',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.computer),
                  ),
                ),
                const SizedBox(height: 15),
                TextField(
                  controller: _portController,
                  decoration: const InputDecoration(
                    labelText: 'SIGNALING PORT',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.settings_ethernet),
                  ),
                ),
                const SizedBox(height: 15),
                TextField(
                  controller: _tokenController,
                  decoration: const InputDecoration(
                    labelText: 'PAIRING TOKEN',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.vpn_key),
                  ),
                ),
                const SizedBox(height: 15),
                TextField(
                  controller: _passcodeController,
                  obscureText: true,
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(
                    labelText: 'DESKTOP SECURE PASSCODE',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.lock),
                  ),
                ),
                const SizedBox(height: 15),
                TextField(
                  controller: _fingerprintController,
                  decoration: const InputDecoration(
                    labelText: 'CERTIFICATE FINGERPRINT',
                    border: OutlineInputBorder(),
                    prefixIcon: Icon(Icons.fingerprint),
                  ),
                ),
                const SizedBox(height: 25),
                if (_isConnecting)
                  const Center(child: CircularProgressIndicator(color: Color(0xFF00F3FF)))
                else
                  ElevatedButton(
                    onPressed: _handlePairing,
                    style: ElevatedButton.styleFrom(
                      padding: const EdgeInsets.symmetric(vertical: 16),
                      backgroundColor: const Color(0xFF00F3FF),
                      foregroundColor: Colors.black,
                      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
                    ),
                    child: const Text('ESTABLISH SECURE LINK', style: TextStyle(fontWeight: FontWeight.bold, fontSize: 16)),
                  ),
                if (_errorText.isNotEmpty) ...[
                  const SizedBox(height: 15),
                  Text(
                    _errorText,
                    style: const TextStyle(color: Color(0xFFFF0055), fontSize: 12, fontWeight: FontWeight.bold),
                    textAlign: TextAlign.center,
                  ),
                ]
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class QrScannerScreen extends StatefulWidget {
  const QrScannerScreen({super.key});

  @override
  State<QrScannerScreen> createState() => _QrScannerScreenState();
}

class _QrScannerScreenState extends State<QrScannerScreen> {
  final MobileScannerController _controller = MobileScannerController(
    detectionSpeed: DetectionSpeed.normal,
    facing: CameraFacing.back,
  );

  bool _hasPopped = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        title: const Text('ALIGN QR CODE', style: TextStyle(fontWeight: FontWeight.bold, letterSpacing: 1)),
        backgroundColor: Colors.black,
        elevation: 0,
        actions: [
          IconButton(
            icon: ValueListenableBuilder(
              valueListenable: _controller.torchState,
              builder: (context, state, child) {
                switch (state) {
                  case TorchState.off:
                    return const Icon(Icons.flash_off, color: Colors.grey);
                  case TorchState.on:
                    return const Icon(Icons.flash_on, color: Color(0xFF00F3FF));
                }
              },
            ),
            onPressed: () => _controller.toggleTorch(),
          ),
          IconButton(
            icon: ValueListenableBuilder(
              valueListenable: _controller.cameraFacingState,
              builder: (context, state, child) {
                switch (state) {
                  case CameraFacing.front:
                    return const Icon(Icons.camera_front, color: Color(0xFF00F3FF));
                  case CameraFacing.back:
                    return const Icon(Icons.camera_rear, color: Color(0xFF00F3FF));
                }
              },
            ),
            onPressed: () => _controller.switchCamera(),
          ),
        ],
      ),
      body: Stack(
        children: [
          MobileScanner(
            controller: _controller,
            onDetect: (capture) {
              if (_hasPopped) return;
              final List<Barcode> barcodes = capture.barcodes;
              if (barcodes.isNotEmpty) {
                final String? code = barcodes.first.rawValue;
                if (code != null && code.isNotEmpty) {
                  _hasPopped = true;
                  Navigator.of(context).pop(code);
                }
              }
            },
          ),
          // Futuristic translucent overlay surrounding a central scanning viewport cutout
          Positioned.fill(
            child: Container(
              color: Colors.black.withOpacity(0.35),
            ),
          ),
          Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 250,
                  height: 250,
                  decoration: BoxDecoration(
                    border: Border.all(color: const Color(0xFF00F3FF), width: 3),
                    borderRadius: BorderRadius.circular(16),
                    boxShadow: [
                      BoxShadow(
                        color: const Color(0xFF00F3FF).withOpacity(0.3),
                        blurRadius: 15,
                        spreadRadius: 2,
                      )
                    ],
                  ),
                  child: Stack(
                    children: [
                      // corner accents
                      Positioned(
                        top: 10,
                        left: 10,
                        child: Container(width: 20, height: 20, decoration: const BoxDecoration(border: Border(top: BorderSide(color: Colors.white, width: 2), left: BorderSide(color: Colors.white, width: 2)))),
                      ),
                      Positioned(
                        top: 10,
                        right: 10,
                        child: Container(width: 20, height: 20, decoration: const BoxDecoration(border: Border(top: BorderSide(color: Colors.white, width: 2), right: BorderSide(color: Colors.white, width: 2)))),
                      ),
                      Positioned(
                        bottom: 10,
                        left: 10,
                        child: Container(width: 20, height: 20, decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: Colors.white, width: 2), left: BorderSide(color: Colors.white, width: 2)))),
                      ),
                      Positioned(
                        bottom: 10,
                        right: 10,
                        child: Container(width: 20, height: 20, decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: Colors.white, width: 2), right: BorderSide(color: Colors.white, width: 2)))),
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: 24),
                Container(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                  decoration: BoxDecoration(
                    color: Colors.black.withOpacity(0.7),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: Colors.white.withOpacity(0.1)),
                  ),
                  child: const Text(
                    'POINT CAMERA AT PAIRING QR CODE',
                    style: TextStyle(
                      color: Color(0xFF00F3FF),
                      fontSize: 12,
                      fontWeight: FontWeight.bold,
                      letterSpacing: 1.5,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

