import 'dart:async';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'dart:convert';
import 'dart:typed_data';
import 'package:speech_to_text/speech_to_text.dart' as stt;
import 'package:crypto/crypto.dart';
import 'package:http/http.dart' as http;
import '../main.dart';

class ChatScreen extends StatefulWidget {
  const ChatScreen({super.key});

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final List<Map<String, String>> _messages = [
    {'sender': 'system', 'text': 'AVATAR CHAT MODULE INITIALIZED. CONNECTED TO SECURE VAULT.'}
  ];
  final _textController = TextEditingController();
  WebSocketChannel? _channel;
  bool _isConnected = false;
  bool _isConnecting = false;
  bool _isRecording = false;
  String _status = 'Initializing...';
  Timer? _reconnectTimer;
  int _reconnectDelay = 1;
  bool _disposed = false;

  final stt.SpeechToText _speech = stt.SpeechToText();
  bool _speechEnabled = false;
  String _wordsSpoken = "";

  @override
  void initState() {
    super.initState();
    _connectWebSocket();
    _initSpeech();
  }

  @override
  void dispose() {
    _disposed = true;
    _reconnectTimer?.cancel();
    _channel?.sink.close();
    _textController.dispose();
    super.dispose();
  }

  void _initSpeech() async {
    try {
      _speechEnabled = await _speech.initialize(
        onError: (val) => debugPrint('Speech Init Error: $val'),
        onStatus: (val) => debugPrint('Speech Init Status: $val'),
      );
    } catch (e) {
      debugPrint('Speech Init Exception: $e');
    }
    if (mounted) {
      setState(() {});
    }
  }

  Future<void> _connectWebSocket() async {
    if (_disposed) return;
    final prefs = await SharedPreferences.getInstance();
    var ip = prefs.getString('server_ip');
    var port = prefs.getInt('server_port');
    final token = prefs.getString('session_token');

    if (token == null) {
      setState(() {
        _status = 'Configuration missing. Please pair first.';
      });
      return;
    }

    setState(() {
      _isConnecting = true;
      _status = 'Connecting...';
    });

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
        }
      }
    } catch (e) {
      debugPrint('ChatScreen failed to resolve target: $e');
    }

    if (ip == null || port == null) {
      _handleDisconnect();
      return;
    }

    try {
      final scheme = ip.contains(RegExp(r'[a-zA-Z]')) ? 'wss' : 'ws';
      final wsUrl = Uri.parse('$scheme://$ip:$port');
      final channel = WebSocketChannel.connect(wsUrl);
      _channel = channel;

      // Send token for authentication
      channel.sink.add(token);

      final temporaryKey = sha256.convert(utf8.encode(token)).toString();

      channel.stream.listen(
        (data) {
          if (data is String) {
            if (data == 'HOST_LOCKED') {
              setState(() {
                _isConnected = true;
                _isConnecting = false;
                _status = 'Host is locked.';
              });
              // Auto unlock
              final passcode = globalPasscode;
              if (passcode != null) {
                final payload = {
                  'action': 'unlock_host',
                  'params': {'passcode': passcode},
                };
                final encrypted = encryptAESGCM(jsonEncode(payload), temporaryKey);
                channel.sink.add(encrypted);
              }
              return;
            } else if (data == 'NEED_AUTH') {
              final passcode = globalPasscode;
              if (passcode != null) {
                final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
                final authPayload = jsonEncode({'action': 'authenticate', 'params': {}});
                final encryptedAuth = encryptAESGCM(authPayload, sessionKey);
                channel.sink.add(encryptedAuth);
              }
              return;
            } else if (data == 'AUTH_FAILED') {
              _handleDisconnect();
              return;
            }

            // Decrypt GCM payload
            String? decrypted;
            final passcode = globalPasscode;
            if (passcode != null) {
              final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
              try {
                decrypted = decryptAESGCM(data, sessionKey);
              } catch (_) {}
            }

            if (decrypted == null) {
              try {
                decrypted = decryptAESGCM(data, temporaryKey);
              } catch (_) {}
            }

            if (decrypted != null) {
              if (decrypted.startsWith('AUTH_OK')) {
                setState(() {
                  _isConnected = true;
                  _isConnecting = false;
                  _reconnectDelay = 1;
                  _status = 'Connected';
                });
              } else if (decrypted.trim().startsWith('{')) {
                _handleJsonMessage(decrypted);
              } else {
                setState(() {
                  _messages.add({'sender': 'assistant', 'text': decrypted!});
                });
              }
            }
          }
        },
        onError: (err) => _handleDisconnect(),
        onDone: () => _handleDisconnect(),
      );
    } catch (_) {
      _handleDisconnect();
    }
  }

  void _handleDisconnect() {
    if (_disposed) return;
    setState(() {
      _isConnected = false;
      _isConnecting = false;
      _channel = null;
      _status = 'Disconnected. Reconnecting...';
    });

    _reconnectTimer?.cancel();
    _reconnectTimer = Timer(Duration(seconds: _reconnectDelay), () {
      if (!_disposed) {
        _reconnectDelay = (_reconnectDelay * 2).clamp(1, 16);
        _connectWebSocket();
      }
    });
  }

  void _handleJsonMessage(String data) {
    try {
      final payload = jsonDecode(data) as Map<String, dynamic>;
      if (payload['Success'] != null) {
        setState(() {
          _messages.add({'sender': 'assistant', 'text': payload['Success']});
        });
      } else if (payload['PermissionRequired'] != null) {
        final details = payload['PermissionRequired']['details'];
        setState(() {
          _messages.add({
            'sender': 'system',
            'text': 'PERMISSION BLOCKED: Manual verification required on desktop for action. Details: $details'
          });
        });
      } else if (payload['Failed'] != null) {
        setState(() {
          _messages.add({'sender': 'system', 'text': 'COMMAND EXECUTION FAILED: ${payload['Failed']}'});
        });
      }
    } catch (_) {}
  }

  void _sendMessage() {
    final text = _textController.text.trim();
    if (text.isEmpty || !_isConnected || _channel == null) return;

    _textController.clear();
    setState(() {
      _messages.add({'sender': 'user', 'text': text});
    });

    final payload = {
      'action': 'send_command',
      'params': {'text': text, 'session_id': 'mobile_chat_session'},
    };
    
    final passcode = globalPasscode;
    if (passcode != null) {
      final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
      final encrypted = encryptAESGCM(jsonEncode(payload), sessionKey);
      _channel?.sink.add(encrypted);
    }
  }

  void _handlePushToTalk(bool record) async {
    setState(() {
      _isRecording = record;
    });

    if (record) {
      _wordsSpoken = "";
      if (_speechEnabled) {
        setState(() {
          _messages.add({'sender': 'system', 'text': 'Listening...'});
        });
        await _speech.listen(
          onResult: (result) {
            setState(() {
              _wordsSpoken = result.recognizedWords;
            });
          },
        );
      } else {
        setState(() {
          _messages.add({'sender': 'system', 'text': 'Speech recognition not enabled or initialized.'});
        });
      }
    } else {
      if (_speechEnabled) {
        await _speech.stop();
      }
      
      // Wait slightly for any final transcription updates
      await Future.delayed(const Duration(milliseconds: 500));

      if (_wordsSpoken.isNotEmpty) {
        final textToSend = _wordsSpoken;
        setState(() {
          _messages.add({'sender': 'user', 'text': '> [Voice] $textToSend'});
        });
        
        if (_isConnected && _channel != null) {
          final payload = {
            'action': 'send_command',
            'params': {'text': textToSend, 'session_id': 'mobile_chat_session'},
          };
          final passcode = globalPasscode;
          if (passcode != null) {
            final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
            final encrypted = encryptAESGCM(jsonEncode(payload), sessionKey);
            _channel?.sink.add(encrypted);
          }
        } else {
          setState(() {
            _messages.add({'sender': 'system', 'text': 'Error: Not connected to host.'});
          });
        }
      } else {
        setState(() {
          _messages.add({'sender': 'system', 'text': 'No speech recognized.'});
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        // Connection status indicator
        Container(
          padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 8),
          color: _isConnected ? Colors.cyan.withOpacity(0.1) : Colors.redAccent.withOpacity(0.1),
          width: double.infinity,
          alignment: Alignment.center,
          child: Text(
            'STATUS: $_status',
            style: TextStyle(
              color: _isConnected ? const Color(0xFF00F3FF) : Colors.redAccent,
              fontSize: 10,
              fontWeight: FontWeight.bold,
              fontFamily: 'monospace',
            ),
          ),
        ),
        // Message log view
        Expanded(
          child: ListView.builder(
            padding: const EdgeInsets.all(16),
            itemCount: _messages.length,
            itemBuilder: (context, idx) {
              final msg = _messages[idx];
              final isUser = msg['sender'] == 'user';
              final isSys = msg['sender'] == 'system';
              
              Color textColor = const Color(0xFF00F3FF);
              if (isUser) textColor = const Color(0xFFBD00FF);
              if (isSys) textColor = Colors.grey;

              return Padding(
                padding: const EdgeInsets.symmetric(vertical: 6.0),
                child: Text(
                  msg['text']!,
                  style: TextStyle(
                    fontFamily: 'monospace',
                    fontSize: 14,
                    color: textColor,
                    fontWeight: isSys ? FontWeight.normal : FontWeight.bold,
                  ),
                ),
              );
            },
          ),
        ),

        // Waveform / Recording feedback
        if (_isRecording)
          Container(
            height: 45,
            alignment: Alignment.center,
            color: const Color(0xFF0A0B14),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: List.generate(
                6,
                (i) => Container(
                  width: 3,
                  height: 15.0 + (i % 2 == 0 ? 15.0 : 5.0),
                  margin: const EdgeInsets.symmetric(horizontal: 2),
                  decoration: BoxDecoration(color: const Color(0xFF00F3FF), borderRadius: BorderRadius.circular(2)),
                ),
              ),
            ),
          ),

        // Text & Voice input drawer
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          color: const Color(0xFF0A0B14),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  controller: _textController,
                  decoration: InputDecoration(
                    hintText: 'ENTER COMMAND...',
                    hintStyle: TextStyle(color: Colors.white.withOpacity(0.3), fontSize: 13, fontFamily: 'monospace'),
                    border: InputBorder.none,
                  ),
                  onSubmitted: (_) => _sendMessage(),
                ),
              ),
              GestureDetector(
                onLongPressStart: (_) => _handlePushToTalk(true),
                onLongPressEnd: (_) => _handlePushToTalk(false),
                child: CircleAvatar(
                  backgroundColor: _isRecording ? const Color(0xFFFF0055) : const Color(0xFFBD00FF),
                  child: Icon(_isRecording ? Icons.mic : Icons.mic_none, color: Colors.white),
                ),
              ),
              const SizedBox(width: 8),
              IconButton(
                icon: const Icon(Icons.send, color: Color(0xFF00F3FF)),
                onPressed: _sendMessage,
              )
            ],
          ),
        )
      ],
    );
  }
}
