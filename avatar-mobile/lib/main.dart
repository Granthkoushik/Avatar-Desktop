import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:crypto/crypto.dart';
import 'dart:convert';
import 'dart:typed_data';
import 'package:encrypt/encrypt.dart' as enc;
import 'screens/dashboard.dart';
import 'screens/pairing.dart';

String? globalPasscode;

String encryptAESGCM(String plaintext, String keyHex) {
  final keyBytes = enc.Key.fromBase16(keyHex);
  final iv = enc.IV.fromSecureRandom(12);
  final encrypter = enc.Encrypter(enc.AES(keyBytes, mode: enc.AESMode.gcm));
  final encrypted = encrypter.encrypt(plaintext, iv: iv);
  
  final combined = Uint8List(iv.bytes.length + encrypted.bytes.length);
  combined.setRange(0, iv.bytes.length, iv.bytes);
  combined.setRange(iv.bytes.length, combined.length, encrypted.bytes);
  
  return base64.encode(combined);
}

String decryptAESGCM(String ciphertextB64, String keyHex) {
  final keyBytes = enc.Key.fromBase16(keyHex);
  final combined = base64.decode(ciphertextB64);
  if (combined.length < 12) {
    throw Exception('Ciphertext too short');
  }
  
  final ivBytes = combined.sublist(0, 12);
  final encryptedBytes = combined.sublist(12);
  
  final encrypter = enc.Encrypter(enc.AES(keyBytes, mode: enc.AESMode.gcm));
  final decrypted = encrypter.decrypt(enc.Encrypted(encryptedBytes), iv: enc.IV(ivBytes));
  return decrypted;
}

Uint8List decryptAESGCMBin(Uint8List combined, String keyHex) {
  final keyBytes = enc.Key.fromBase16(keyHex);
  if (combined.length < 12) {
    throw Exception('Ciphertext too short');
  }
  
  final ivBytes = combined.sublist(0, 12);
  final encryptedBytes = combined.sublist(12);
  
  final encrypter = enc.Encrypter(enc.AES(keyBytes, mode: enc.AESMode.gcm));
  final decrypted = encrypter.decryptBytes(enc.Encrypted(encryptedBytes), iv: enc.IV(ivBytes));
  return Uint8List.fromList(decrypted);
}

void main() {
  runApp(const AvatarMobileApp());
}

class AvatarMobileApp extends StatelessWidget {
  const AvatarMobileApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Avatar Mobile',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark().copyWith(
        scaffoldBackgroundColor: const Color(0xFF030306),
        colorScheme: const ColorScheme.dark(
          primary: Color(0xFF00F3FF),
          secondary: Color(0xFFBD00FF),
          background: Color(0xFF030306),
        ),
        useMaterial3: true,
      ),
      home: const AuthGuard(),
    );
  }
}

class AuthGuard extends StatefulWidget {
  const AuthGuard({super.key});

  @override
  State<AuthGuard> createState() => _AuthGuardState();
}

class _AuthGuardState extends State<AuthGuard> {
  bool _isPaired = false;
  bool _isLoading = true;
  String _pin = '';
  String _errorText = '';

  @override
  void initState() {
    super.initState();
    _checkPairingStatus();
  }

  Future<void> _checkPairingStatus() async {
    final prefs = await SharedPreferences.getInstance();
    final serverIp = prefs.getString('server_ip');
    final savedPasscode = prefs.getString('secure_passcode');
    if (serverIp != null && savedPasscode != null) {
      globalPasscode = savedPasscode;
      setState(() {
        _isPaired = true;
        _isLoading = false;
      });
      WidgetsBinding.instance.addPostFrameCallback((_) {
        Navigator.of(context).pushReplacement(
          MaterialPageRoute(builder: (_) => const DashboardScreen()),
        );
      });
      return;
    }
    setState(() {
      _isPaired = serverIp != null;
      _isLoading = false;
    });
  }

  void _handlePinPress(String value) {
    setState(() {
      _errorText = '';
      if (_pin.length < 4) {
        _pin += value;
      }
    });
  }

  void _handleClear() {
    setState(() {
      _pin = '';
      _errorText = '';
    });
  }

  String _hashPin(String pin) {
    final bytes = utf8.encode(pin);
    return sha256.convert(bytes).toString();
  }

  Future<void> _handleUnlock() async {
    if (_pin.isEmpty) return;

    final prefs = await SharedPreferences.getInstance();
    final storedHash = prefs.getString('local_pin_hash');
    final pinHash = _hashPin(_pin);

    if (storedHash == null) {
      await prefs.setString('local_pin_hash', pinHash);
      globalPasscode = _pin;
      _navigateToNext();
    } else if (storedHash == pinHash) {
      globalPasscode = _pin;
      _navigateToNext();
    } else {
      setState(() {
        _errorText = 'INVALID PIN ACCESS DENIED';
        _pin = '';
      });
    }
  }

  void _navigateToNext() {
    if (_isPaired) {
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(builder: (_) => const DashboardScreen()),
      );
    } else {
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(builder: (_) => const PairingScreen()),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_isLoading) {
      return const Scaffold(
        body: Center(child: CircularProgressIndicator(color: Color(0xFF00F3FF))),
      );
    }

    return Scaffold(
      body: Center(
        child: Container(
          width: 360,
          padding: const EdgeInsets.all(24),
          decoration: BoxDecoration(
            color: const Color(0xFF0A0B14).withOpacity(0.7),
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: const Color(0xFFBD00FF).withOpacity(0.15)),
            boxShadow: [
              BoxShadow(
                color: const Color(0xFFBD00FF).withOpacity(0.08),
                blurRadius: 40,
              )
            ],
          ),
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text(
                  'AVATAR CORE',
                  style: TextStyle(
                    fontSize: 32,
                    fontWeight: FontWeight.w900,
                    color: Color(0xFF00F3FF),
                    letterSpacing: 2,
                  ),
                ),
                const SizedBox(height: 10),
                Text(
                  'ENTER SECURITY PIN',
                  style: TextStyle(color: Colors.white.withOpacity(0.5), fontSize: 12),
                ),
                const SizedBox(height: 25),
                Container(
                  height: 60,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: Colors.black.withOpacity(0.4),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: Colors.white.withOpacity(0.1)),
                  ),
                  child: Text(
                    '*' * _pin.length,
                    style: const TextStyle(
                      fontFamily: 'monospace',
                      fontSize: 28,
                      letterSpacing: 8,
                      color: Color(0xFF00F3FF),
                    ),
                  ),
                ),
                const SizedBox(height: 25),
                GridView.builder(
                  shrinkWrap: true,
                  physics: const NeverScrollableScrollPhysics(),
                  gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: 3,
                    mainAxisSpacing: 12,
                    crossAxisSpacing: 12,
                    childAspectRatio: 1.3,
                  ),
                  itemCount: 12,
                  itemBuilder: (context, idx) {
                    if (idx == 9) {
                      return ElevatedButton(
                        onPressed: _handleClear,
                        style: ElevatedButton.styleFrom(backgroundColor: const Color(0x15FF0055)),
                        child: const Text('CLR', style: TextStyle(color: Color(0xFFFF0055), fontWeight: FontWeight.bold)),
                      );
                    }
                    if (idx == 11) {
                      return ElevatedButton(
                        onPressed: _handleUnlock,
                        style: ElevatedButton.styleFrom(backgroundColor: const Color(0x1539FF14)),
                        child: const Text('ENTER', style: TextStyle(color: Color(0xFF39FF14), fontWeight: FontWeight.bold)),
                      );
                    }
                    final value = idx == 10 ? '0' : '${idx + 1}';
                    return ElevatedButton(
                      onPressed: () => _handlePinPress(value),
                      style: ElevatedButton.styleFrom(backgroundColor: Colors.white.withOpacity(0.02)),
                      child: Text(value, style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
                    );
                  },
                ),
                if (_errorText.isNotEmpty) ...[
                  const SizedBox(height: 15),
                  Text(
                    _errorText,
                    style: const TextStyle(color: Color(0xFFFF0055), fontSize: 12, fontWeight: FontWeight.bold),
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
