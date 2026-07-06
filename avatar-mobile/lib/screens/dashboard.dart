import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:speech_to_text/speech_to_text.dart' as stt;
import 'package:http/http.dart' as http;
import 'package:crypto/crypto.dart';
import 'package:encrypt/encrypt.dart' as enc;
import 'remote_view.dart';
import 'chat.dart';
import '../main.dart';

class DashboardScreen extends StatefulWidget {
  const DashboardScreen({super.key});

  @override
  State<DashboardScreen> createState() => _DashboardScreenState();
}

class _DashboardScreenState extends State<DashboardScreen> with WidgetsBindingObserver {
  WebSocketChannel? _channel;
  bool _isConnected = false;
  bool _isHostLocked = false;
  
  double _cpu = 0.0;
  String _ram = '0 GB / 0 GB';
  double _ramPct = 0.0;
  String _storage = '0 GB / 0 GB';
  double _storagePct = 0.0;
  double _battery = 100.0;
  bool _isCharging = false;
  List<dynamic> _processes = [];
  String _gpuName = 'N/A';

  int _currentIndex = 0;
  final TextEditingController _passcodeController = TextEditingController();

  // Voice Assistant state fields
  final stt.SpeechToText _speech = stt.SpeechToText();
  bool _speechEnabled = false;
  bool _isListening = false;
  String _wordsSpoken = "";
  String _assistantReply = "";
  void Function(void Function())? _modalSetState;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _connectToHost();
    _initSpeech();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _channel?.sink.close();
    _passcodeController.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed && !_isConnected) {
      _connectToHost();
    }
    // Keep socket alive on pause.
  }




  void _initSpeech() async {
    try {
      _speechEnabled = await _speech.initialize(
        onError: (val) => debugPrint('Assistant Speech Init Error: $val'),
        onStatus: (val) => debugPrint('Assistant Speech Init Status: $val'),
      );
      if (!_speechEnabled && mounted) {
        debugPrint('Speech recognition unavailable on this device.');
      }
    } catch (e) {
      debugPrint('Assistant Speech Init Exception: $e');
    }
  }

  Future<void> _connectToHost() async {
    final prefs = await SharedPreferences.getInstance();
    var ip = prefs.getString('server_ip');
    var port = prefs.getInt('server_port');
    final token = prefs.getString('session_token');

    if (token == null) return;

    // Fetch dynamic connection target from KV registry
    final inputBytes = utf8.encode('discovery_$token');
    final discoveryKey = sha256.convert(inputBytes).toString();
    try {
      final url = Uri.parse('https://keyvalue.immanuel.co/api/KeyVal/GetValue/0gcpgxva/$discoveryKey');
      final response = await http.get(url).timeout(const Duration(seconds: 4));
      if (response.statusCode == 200) {
        var val = response.body.trim();
        if (val.startsWith('"') && val.endsWith('"') && val.length > 1) {
          val = val.substring(1, val.length - 1);
        }
        if (val.isNotEmpty && val != 'offline' && val != 'null') {
          final separator = val.contains('_') ? '_' : ':';
          final parts = val.split(separator);
          ip = parts[0];
          port = parts.length > 1 ? int.tryParse(parts[1]) ?? 8086 : 8086;
          debugPrint('Resolved dynamic connection target from KV: $ip:$port');
        }
      }
    } catch (e) {
      debugPrint('Failed to resolve dynamic connection target from KV: $e. Falling back to cached IP: $ip:$port');
    }

    if (ip == null || port == null) return;

    try {
      final scheme = ip.contains(RegExp(r'[a-zA-Z]')) ? 'wss' : 'ws';
      final wsUrl = Uri.parse('$scheme://$ip:$port');
      final channel = WebSocketChannel.connect(wsUrl);
      
      // Send token for authentication
      channel.sink.add(token);
      
      final temporaryKey = sha256.convert(utf8.encode(token)).toString();

      channel.stream.listen(
        (data) {
          if (data is String) {
            if (data == 'HOST_LOCKED') {
              setState(() {
                _isHostLocked = true;
                _isConnected = true;
              });
              final passcode = globalPasscode;
              if (passcode != null && passcode.isNotEmpty) {
                _sendUnlockRequest(passcode);
              }
              return;
            } else if (data == 'NEED_AUTH') {
              // Send encrypted authenticate message
              final passcode = globalPasscode;
              if (passcode != null) {
                final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
                final authPayload = jsonEncode({'action': 'authenticate', 'params': {}});
                final encryptedAuth = encryptAESGCM(authPayload, sessionKey);
                channel.sink.add(encryptedAuth);
              }
              return;
            } else if (data == 'AUTH_FAILED') {
              setState(() {
                _isConnected = false;
                _isHostLocked = false;
              });
              return;
            }

            // Decrypt GCM payload
            String? decrypted;
            final passcode = globalPasscode;
            if (passcode != null) {
              final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
              try {
                decrypted = decryptAESGCM(data, sessionKey);
              } catch (_) {
                // ignore
              }
            }

            if (decrypted == null) {
              try {
                decrypted = decryptAESGCM(data, temporaryKey);
              } catch (_) {
                // ignore
              }
            }

            if (decrypted != null) {
              if (decrypted.startsWith('AUTH_OK')) {
                setState(() {
                  _isConnected = true;
                  _isHostLocked = false;
                });
                _requestMetricsLoop();
              } else if (decrypted.trim().startsWith('{')) {
                _handleJsonMessage(decrypted);
              }
            }
          }
        },
        onError: (err) {
          _handleDisconnect();
        },
        onDone: () {
          _handleDisconnect();
        },
      );

      _channel = channel;
    } catch (_) {
      _handleDisconnect();
    }
  }

  void _handleJsonMessage(String data) {
    try {
      final payload = jsonDecode(data) as Map<String, dynamic>;
      if (payload['cpu_usage_pct'] != null) {
        _updateMetrics(payload);
        return;
      }

      String text = '';
      if (payload['Success'] != null) {
        text = payload['Success'].toString();
        if (text.contains('Remote unlock successful') || text.contains('Already unlocked')) {
          setState(() {
            _isHostLocked = false;
            _isConnected = true;
          });
          _requestMetricsLoop();
        }
      } else if (payload['Failed'] != null) {
        text = 'Failed: ${payload['Failed']}';
        if (_isHostLocked) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text(text), backgroundColor: Colors.redAccent),
          );
        }
      } else if (payload['PermissionRequired'] != null) {
        final req = payload['PermissionRequired'] as Map<String, dynamic>;
        text = 'Approval required: ${req['details'] ?? 'unknown action'}';
      }

      if (text.isNotEmpty) {
        setState(() => _assistantReply = text);
        _modalSetState?.call(() {});
      }
    } catch (_) {}
  }

  double _asDouble(dynamic value) {
    if (value is double) return value;
    if (value is int) return value.toDouble();
    if (value is num) return value.toDouble();
    return 0.0;
  }

  int _asInt(dynamic value) {
    if (value is int) return value;
    if (value is num) return value.toInt();
    return 0;
  }

  void _updateMetrics(Map<String, dynamic> data) {
    setState(() {
      _cpu = _asDouble(data['cpu_usage_pct']);

      final ramUsed = _asInt(data['ram_used_bytes']);
      final ramTotal = _asInt(data['ram_total_bytes']);
      _ram = '${_formatBytes(ramUsed)} / ${_formatBytes(ramTotal)}';
      _ramPct = ramTotal > 0 ? (ramUsed / ramTotal) : 0.0;

      final diskUsed = _asInt(data['disk_used_bytes']);
      final diskTotal = _asInt(data['disk_total_bytes']);
      _storage = '${_formatBytes(diskUsed)} / ${_formatBytes(diskTotal)}';
      _storagePct = diskTotal > 0 ? (diskUsed / diskTotal) : 0.0;

      _battery = _asDouble(data['battery_pct']);
      _isCharging = data['is_charging'] == true;
      _processes = data['running_processes'] as List<dynamic>? ?? [];

      final gpus = data['gpus'] as List<dynamic>? ?? [];
      if (gpus.isNotEmpty) {
        _gpuName = gpus[0]['name']?.toString() ?? 'N/A';
      }
    });
  }

  void _requestMetricsLoop() async {
    while (_isConnected && _channel != null) {
      _sendControlCommand('get_metrics', {});
      await Future.delayed(const Duration(milliseconds: 2500));
    }
  }

  void _handleDisconnect() {
    setState(() {
      _isConnected = false;
      _channel = null;
    });
    // Retry connection after 5 seconds
    Future.delayed(const Duration(seconds: 5), () {
      if (mounted && !_isConnected) {
        _connectToHost();
      }
    });
  }

  Future<void> _sendControlCommand(String action, Map<String, dynamic> params) async {
    if (!_isConnected || _channel == null) return;
    final payload = {
      'action': action,
      'params': params,
    };
    final passcode = globalPasscode;
    if (passcode != null) {
      final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
      final encrypted = encryptAESGCM(jsonEncode(payload), sessionKey);
      _channel?.sink.add(encrypted);
    }
  }

  void _showUnlockOsDialog() {
    final textController = TextEditingController();
    showDialog(
      context: context,
      builder: (context) {
        return AlertDialog(
          backgroundColor: const Color(0xFF0A0B14),
          title: const Text('Unlock Windows Laptop', style: TextStyle(color: Color(0xFF00F3FF))),
          content: TextField(
            controller: textController,
            obscureText: true,
            style: const TextStyle(color: Colors.white),
            decoration: const InputDecoration(
              hintText: 'Enter Windows password...',
              hintStyle: TextStyle(color: Colors.white24),
              enabledBorder: UnderlineInputBorder(borderSide: BorderSide(color: Colors.white30)),
              focusedBorder: UnderlineInputBorder(borderSide: BorderSide(color: Color(0xFF00F3FF))),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('CANCEL', style: TextStyle(color: Colors.grey)),
            ),
            TextButton(
              onPressed: () {
                final password = textController.text;
                if (password.isNotEmpty) {
                  _sendControlCommand('unlock_os', {'password': password});
                  Navigator.pop(context);
                  ScaffoldMessenger.of(context).showSnackBar(
                    const SnackBar(content: Text('OS unlock sequence transmitted.')),
                  );
                }
              },
              child: const Text('UNLOCK', style: TextStyle(color: Color(0xFF00F3FF))),
            ),
          ],
        );
      },
    );
  }

  void _showOpenWebsiteDialog() {
    final textController = TextEditingController();
    showDialog(
      context: context,
      builder: (context) {
        return AlertDialog(
          backgroundColor: const Color(0xFF0A0B14),
          title: const Text('Open Website on Laptop', style: TextStyle(color: Color(0xFF00F3FF))),
          content: TextField(
            controller: textController,
            style: const TextStyle(color: Colors.white),
            decoration: const InputDecoration(
              hintText: 'e.g. youtube.com or google.com',
              hintStyle: TextStyle(color: Colors.white24),
              enabledBorder: UnderlineInputBorder(borderSide: BorderSide(color: Colors.white30)),
              focusedBorder: UnderlineInputBorder(borderSide: BorderSide(color: Color(0xFF00F3FF))),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('CANCEL', style: TextStyle(color: Colors.grey)),
            ),
            TextButton(
              onPressed: () {
                final url = textController.text.trim();
                if (url.isNotEmpty) {
                  _sendControlCommand('open_url', {'url': url});
                  Navigator.pop(context);
                  ScaffoldMessenger.of(context).showSnackBar(
                    SnackBar(content: Text('Opening $url on laptop.')),
                  );
                }
              },
              child: const Text('OPEN', style: TextStyle(color: Color(0xFF00F3FF))),
            ),
          ],
        );
      },
    );
  }

  Future<void> _sendUnlockRequest(String passcode) async {
    if (_channel == null) return;
    final prefs = await SharedPreferences.getInstance();
    final token = prefs.getString('session_token');
    if (token == null) return;

    final temporaryKey = sha256.convert(utf8.encode(token)).toString();
    final payload = {
      'action': 'unlock_host',
      'params': {'passcode': passcode},
    };
    final encrypted = encryptAESGCM(jsonEncode(payload), temporaryKey);
    _channel?.sink.add(encrypted);
    globalPasscode = passcode;
    _passcodeController.clear();
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text('Auto-unlocking secure vault...'), duration: Duration(seconds: 2)),
    );
  }

  Future<void> _triggerEmergencyKill() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.clear(); // Clear all security tokens locally
    
    // Send kill switch signal to desktop if connected
    await _sendControlCommand('emergency_kill', {});
    
    if (mounted) {
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(builder: (_) => const AuthGuard()),
      );
    }
  }

  String _formatBytes(int bytes) {
    if (bytes <= 0) return '0 B';
    const suffixes = ['B', 'KB', 'MB', 'GB', 'TB'];
    var i = 0;
    double val = bytes.toDouble();
    while (val >= 1024 && i < suffixes.length - 1) {
      val /= 1024;
      i++;
    }
    return '${val.toStringAsFixed(1)} ${suffixes[i]}';
  }

  void _startVoiceAssistant() async {
    _wordsSpoken = "";
    _assistantReply = "";
    if (!_speechEnabled) {
      _speechEnabled = await _speech.initialize(
        onError: (val) => debugPrint('Assistant Speech Init Error: $val'),
        onStatus: (val) => debugPrint('Assistant Speech Init Status: $val'),
      );
    }

    if (_speechEnabled) {
      setState(() {
        _isListening = true;
      });

      if (mounted) {
        showModalBottomSheet(
          context: context,
          backgroundColor: const Color(0xFF0A0B14),
          isDismissible: false,
          enableDrag: false,
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.only(
              topLeft: Radius.circular(20),
              topRight: Radius.circular(20),
            ),
          ),
          builder: (context) {
            return StatefulBuilder(
              builder: (context, setModalState) {
                _modalSetState = setModalState;
                return Container(
                  padding: const EdgeInsets.all(24),
                  decoration: BoxDecoration(
                    border: Border.all(color: Colors.cyan.withOpacity(0.2)),
                    borderRadius: const BorderRadius.only(
                      topLeft: Radius.circular(20),
                      topRight: Radius.circular(20),
                    ),
                  ),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Row(
                        mainAxisAlignment: MainAxisAlignment.spaceBetween,
                        children: [
                          const Text(
                            'AVATAR VOICE ASSISTANT',
                            style: TextStyle(color: Color(0xFF00F3FF), fontWeight: FontWeight.bold, letterSpacing: 1.5, fontSize: 16),
                          ),
                          IconButton(
                            icon: const Icon(Icons.close, color: Colors.white60),
                            onPressed: () {
                              _speech.stop();
                              Navigator.pop(context);
                              setState(() {
                                _isListening = false;
                                _modalSetState = null;
                              });
                            },
                          )
                        ],
                      ),
                      const SizedBox(height: 25),
                      Center(
                        child: CircleAvatar(
                          radius: 35,
                          backgroundColor: const Color(0xFFBD00FF).withOpacity(0.2),
                          child: Icon(
                            _isListening ? Icons.mic : Icons.mic_none,
                            color: const Color(0xFF00F3FF),
                            size: 40,
                          ),
                        ),
                      ),
                      const SizedBox(height: 20),
                      Text(
                        _isListening ? 'Listening...' : 'Thinking...',
                        style: TextStyle(color: Colors.white.withOpacity(0.5), fontSize: 12, fontFamily: 'monospace'),
                      ),
                      const SizedBox(height: 15),
                      Text(
                        _wordsSpoken.isEmpty ? '"Say a command like: Lock PC"' : '"$_wordsSpoken"',
                        style: const TextStyle(color: Colors.white, fontSize: 16, fontWeight: FontWeight.bold, fontFamily: 'monospace'),
                        textAlign: TextAlign.center,
                      ),
                      if (_assistantReply.isNotEmpty) ...[
                        const SizedBox(height: 20),
                        const Divider(color: Colors.white10),
                        const SizedBox(height: 10),
                        const Text(
                          'AVATAR:',
                          style: TextStyle(color: Color(0xFFBD00FF), fontSize: 11, fontWeight: FontWeight.bold, letterSpacing: 1),
                        ),
                        const SizedBox(height: 5),
                        Text(
                          _assistantReply,
                          style: const TextStyle(color: Colors.cyanAccent, fontSize: 14, fontWeight: FontWeight.bold),
                          textAlign: TextAlign.center,
                        ),
                      ],
                      const SizedBox(height: 20),
                      if (_isListening)
                        ElevatedButton(
                          onPressed: () async {
                            setModalState(() {
                              _isListening = false;
                            });
                            await _speech.stop();
                            _processVoiceCommand(setModalState);
                          },
                          style: ElevatedButton.styleFrom(
                            backgroundColor: const Color(0xFFFF0055),
                            padding: const EdgeInsets.symmetric(horizontal: 30, vertical: 12),
                          ),
                          child: const Text('STOP & SEND', style: TextStyle(fontWeight: FontWeight.bold)),
                        ),
                    ],
                  ),
                );
              }
            );
          },
        ).then((_) {
          setState(() {
            _isListening = false;
            _modalSetState = null;
          });
        });
      }

      await _speech.listen(
        onResult: (result) {
          _wordsSpoken = result.recognizedWords;
          setState(() {});
          _modalSetState?.call(() {});
        },
        listenFor: const Duration(seconds: 30),
        pauseFor: const Duration(seconds: 3),
        cancelOnError: true,
      );
    } else {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Speech recognition initialization failed.')),
      );
    }
  }

  void _processVoiceCommand(void Function(void Function()) setModalState) async {
    if (_wordsSpoken.trim().isEmpty) {
      setModalState(() {
        _assistantReply = "No command heard.";
      });
      return;
    }

    if (!_isConnected || _channel == null) {
      setModalState(() {
        _assistantReply = _isHostLocked
            ? "Enter your desktop PIN on the lock screen to unlock first."
            : "Host is offline. Check your network connection.";
      });
      return;
    }

    final payload = {
      'action': 'send_command',
      'params': {'text': _wordsSpoken, 'session_id': 'mobile_chat_session'},
    };
    _channel?.sink.add(jsonEncode(payload));

    setModalState(() {
      _assistantReply = "Transmitting voice query...";
    });
  }

  @override
  Widget build(BuildContext context) {
    if (_isHostLocked) {
      return Scaffold(
        backgroundColor: const Color(0xFF020208),
        body: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(24),
            child: Container(
              width: MediaQuery.of(context).size.width * 0.9,
              padding: const EdgeInsets.all(24),
              decoration: BoxDecoration(
                color: const Color(0xFF0A0B14).withOpacity(0.8),
                borderRadius: BorderRadius.circular(16),
                border: Border.all(color: Colors.redAccent.withOpacity(0.3)),
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.lock_outline, color: Colors.redAccent, size: 60),
                  const SizedBox(height: 15),
                  const Text(
                    'HOST LAPTOP IS SECURE',
                    style: TextStyle(color: Colors.white, fontWeight: FontWeight.bold, fontSize: 18, letterSpacing: 1),
                  ),
                  const SizedBox(height: 8),
                  const Text(
                    'Enter system PIN to unlock database vault remotely.',
                    style: TextStyle(color: Colors.white54, fontSize: 12),
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 25),
                  TextField(
                    controller: _passcodeController,
                    obscureText: true,
                    keyboardType: TextInputType.number,
                    style: const TextStyle(color: Colors.white, fontFamily: 'monospace', letterSpacing: 5, fontSize: 18),
                    textAlign: TextAlign.center,
                    decoration: const InputDecoration(
                      hintText: '••••••••',
                      hintStyle: TextStyle(color: Colors.white24, letterSpacing: 5),
                      border: OutlineInputBorder(),
                    ),
                    onSubmitted: (val) {
                      if (val.isNotEmpty) {
                        _sendUnlockRequest(val);
                      }
                    },
                  ),
                  const SizedBox(height: 20),
                  ElevatedButton(
                    onPressed: () {
                      final val = _passcodeController.text.trim();
                      if (val.isNotEmpty) {
                        _sendUnlockRequest(val);
                      }
                    },
                    style: ElevatedButton.styleFrom(
                      backgroundColor: Colors.redAccent,
                      padding: const EdgeInsets.symmetric(horizontal: 40, vertical: 12),
                    ),
                    child: const Text('UNLOCK HOST', style: TextStyle(fontWeight: FontWeight.bold)),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
    }

    final List<Widget> screens = [
      _buildDashboardView(),
      const ChatScreen(),
      const RemoteViewScreen(),
    ];

    return Scaffold(
      appBar: AppBar(
        title: const Text('AVATAR CORE', style: TextStyle(fontWeight: FontWeight.w800, letterSpacing: 1)),
        actions: [
          Container(
            margin: const EdgeInsets.only(right: 15),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            decoration: BoxDecoration(
              color: _isConnected ? Colors.cyan.withOpacity(0.08) : Colors.red.withOpacity(0.08),
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: _isConnected ? Colors.cyan : Colors.red, width: 0.8),
            ),
            child: Row(
              children: [
                Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(shape: BoxShape.circle, color: _isConnected ? Colors.cyan : Colors.red),
                ),
                const SizedBox(width: 6),
                Text(
                  _isConnected ? 'LINKED' : 'OFFLINE',
                  style: TextStyle(fontSize: 10, color: _isConnected ? Colors.cyan : Colors.red, fontWeight: FontWeight.bold),
                )
              ],
            ),
          )
        ],
      ),
      body: IndexedStack(
        index: _currentIndex,
        children: screens,
      ),
      floatingActionButton: _currentIndex == 0 ? FloatingActionButton(
        onPressed: _startVoiceAssistant,
        backgroundColor: const Color(0xFFBD00FF),
        child: const Icon(Icons.mic, color: Colors.white),
      ) : null,
      bottomNavigationBar: BottomNavigationBar(
        currentIndex: _currentIndex,
        onTap: (index) {
          setState(() {
            _currentIndex = index;
          });
        },
        selectedItemColor: const Color(0xFF00F3FF),
        unselectedItemColor: Colors.grey,
        items: const [
          BottomNavigationBarItem(icon: Icon(Icons.dashboard), label: 'Telemetry'),
          BottomNavigationBarItem(icon: Icon(Icons.chat), label: 'AI Chat'),
          BottomNavigationBarItem(icon: Icon(Icons.screen_share), label: 'Live Stream'),
        ],
      ),
    );
  }

  Widget _buildDashboardView() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Resource Metrics Card
          Card(
            color: const Color(0xFF0A0B14),
            child: Padding(
              padding: const EdgeInsets.all(16),
              key: const ValueKey('telemetry_card'),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('RESOURCES', style: TextStyle(fontWeight: FontWeight.bold, color: Colors.grey)),
                  const SizedBox(height: 15),
                  _buildMetricRow('CPU Usage', '${_cpu.toStringAsFixed(1)} %', _cpu / 100),
                  const Divider(color: Colors.white10),
                  _buildMetricRow('RAM Utilization', _ram, _ramPct),
                  const Divider(color: Colors.white10),
                  _buildMetricRow('Storage Capacity', _storage, _storagePct),
                  const Divider(color: Colors.white10),
                  _buildMetricRow('Battery Level', '${_battery.toStringAsFixed(0)}% ${_isCharging ? "(Charging)" : ""}', _battery / 100),
                ],
              ),
            ),
          ),
          const SizedBox(height: 15),
          
          // System Commands Card
          Card(
            color: const Color(0xFF0A0B14),
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('QUICK RUNS', style: TextStyle(fontWeight: FontWeight.bold, color: Colors.grey)),
                  const SizedBox(height: 15),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.spaceEvenly,
                    children: [
                      _buildQuickBtn(Icons.volume_up, 'Vol Up', () => _sendControlCommand('adjust_volume', {'action': 'up'})),
                      _buildQuickBtn(Icons.volume_down, 'Vol Down', () => _sendControlCommand('adjust_volume', {'action': 'down'})),
                      _buildQuickBtn(Icons.lock, 'Lock OS', () => _sendControlCommand('power_action', {'action': 'lock'})),
                      _buildQuickBtn(Icons.mode_night, 'Sleep OS', () => _sendControlCommand('power_action', {'action': 'sleep'})),
                    ],
                  ),
                  const SizedBox(height: 15),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.spaceEvenly,
                    children: [
                      _buildQuickBtn(Icons.lock_open, 'Unlock OS', _showUnlockOsDialog),
                      _buildQuickBtn(Icons.language, 'Open Web', _showOpenWebsiteDialog),
                      const SizedBox(width: 70),
                      const SizedBox(width: 70),
                    ],
                  )
                ],
              ),
            ),
          ),
          const SizedBox(height: 15),

          // Top Processes Card
          Card(
            color: const Color(0xFF0A0B14),
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('PROCESS TABLE (BY MEMORY)', style: TextStyle(fontWeight: FontWeight.bold, color: Colors.grey)),
                  const SizedBox(height: 10),
                  if (_processes.isEmpty)
                    const Text('No telemetry diagnostics received yet.', style: TextStyle(fontSize: 12, color: Colors.white30))
                  else
                    ListView.builder(
                      shrinkWrap: true,
                      physics: const NeverScrollableScrollPhysics(),
                      itemCount: _processes.length > 5 ? 5 : _processes.length,
                      itemBuilder: (context, idx) {
                        final p = _processes[idx];
                        return ListTile(
                          contentPadding: EdgeInsets.zero,
                          title: Text(p['name'] as String, style: const TextStyle(fontSize: 14, fontWeight: FontWeight.bold)),
                          subtitle: Text('PID: ${p['pid']}', style: const TextStyle(fontSize: 10, color: Colors.white54)),
                          trailing: Text(
                            _formatBytes(p['memory_bytes'] as int),
                            style: const TextStyle(color: Color(0xFF00F3FF), fontSize: 13, fontWeight: FontWeight.bold),
                          ),
                        );
                      },
                    )
                ],
              ),
            ),
          ),
          const SizedBox(height: 25),

          ElevatedButton(
            onPressed: _triggerEmergencyKill,
            style: ElevatedButton.styleFrom(
              backgroundColor: const Color(0x20FF0055),
              side: const BorderSide(color: Color(0xFFFF0055)),
              padding: const EdgeInsets.symmetric(vertical: 16),
              shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
            ),
            child: const Text('EMERGENCY LOCKDOWN', style: TextStyle(color: Color(0xFFFF0055), fontWeight: FontWeight.bold, letterSpacing: 1.5)),
          )
        ],
      ),
    );
  }

  Widget _buildMetricRow(String label, String value, double pct) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8.0),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(label, style: const TextStyle(fontSize: 12, color: Colors.white54)),
              const SizedBox(height: 4),
              Text(value, style: const TextStyle(fontSize: 15, fontWeight: FontWeight.bold)),
            ],
          ),
          SizedBox(
            width: 45,
            height: 45,
            child: CircularProgressIndicator(
              value: pct,
              backgroundColor: Colors.white.withOpacity(0.05),
              color: const Color(0xFF00F3FF),
              strokeWidth: 3.5,
            ),
          )
        ],
      ),
    );
  }

  Widget _buildQuickBtn(IconData icon, String label, VoidCallback action) {
    return InkWell(
      onTap: action,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        width: 70,
        padding: const EdgeInsets.symmetric(vertical: 12),
        decoration: BoxDecoration(
          color: Colors.white.withOpacity(0.02),
          border: Border.all(color: Colors.white.withOpacity(0.05)),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          children: [
            Icon(icon, color: const Color(0xFF00F3FF)),
            const SizedBox(height: 6),
            Text(label, style: const TextStyle(fontSize: 10, color: Colors.grey)),
          ],
        ),
      ),
    );
  }
}

