import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:http/http.dart' as http;
import 'package:crypto/crypto.dart';
import '../main.dart';

enum ControlMode { touch, trackpad, view }
enum StreamSource { screen, camera }

class RemoteViewScreen extends StatefulWidget {
  const RemoteViewScreen({super.key});

  @override
  State<RemoteViewScreen> createState() => _RemoteViewScreenState();
}

class _RemoteViewScreenState extends State<RemoteViewScreen> {
  WebSocketChannel? _channel;
  StreamSubscription? _sub;

  final ValueNotifier<Uint8List?> _frameNotifier = ValueNotifier(null);
  final ValueNotifier<String> _statusNotifier = ValueNotifier('Initializing...');
  final ValueNotifier<bool> _connectingNotifier = ValueNotifier(true);

  bool _showKeyboard = false;
  bool _streamRequested = false;
  final TextEditingController _textInputController = TextEditingController();

  int _desktopWidth = 1920;
  int _desktopHeight = 1080;

  bool _disposed = false;
  int _reconnectDelay = 1;
  Timer? _frameWatchdog;
  ControlMode _activeMode = ControlMode.trackpad;
  StreamSource _streamSource = StreamSource.screen;

  // Trackpad cursor and pointer states
  double _cursorX = 960.0;
  double _cursorY = 540.0;
  final Map<int, Offset> _pointerPositions = {};
  Offset? _twoFingerStart;
  double _scrollYAccumulator = 0.0;
  Offset? _threeFingerStart;
  bool _threeFingerGestureTriggered = false;
  Offset? _oneFingerStartPos;
  DateTime? _oneFingerStartTime;

  @override
  void initState() {
    super.initState();
    _connect();
  }

  @override
  void dispose() {
    _disposed = true;
    _frameWatchdog?.cancel();
    _sub?.cancel();
    _channel?.sink.close();
    _frameNotifier.dispose();
    _statusNotifier.dispose();
    _connectingNotifier.dispose();
    _textInputController.dispose();
    super.dispose();
  }

  Future<void> _connect() async {
    if (_disposed) return;

    _streamRequested = false;
    final prefs = await SharedPreferences.getInstance();
    var ip = prefs.getString('server_ip');
    var port = prefs.getInt('server_port');
    final token = prefs.getString('session_token');

    if (token == null) {
      _statusNotifier.value = 'Configuration missing. Please pair device first.';
      _connectingNotifier.value = false;
      return;
    }

    _statusNotifier.value = 'Connecting...';
    _connectingNotifier.value = true;
    _frameNotifier.value = null;

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
          debugPrint('RemoteView resolved dynamic connection target from KV: $ip:$port');
        }
      }
    } catch (e) {
      debugPrint('RemoteView failed to resolve dynamic connection target from KV: $e. Falling back to cached IP: $ip:$port');
    }

    if (ip == null || port == null) {
      _statusNotifier.value = 'Target IP/Port resolution failed.';
      _connectingNotifier.value = false;
      return;
    }

    try {
      final scheme = ip.contains(RegExp(r'[a-zA-Z]')) ? 'wss' : 'ws';
      final wsUrl = Uri.parse('$scheme://$ip:$port');
      final channel = WebSocketChannel.connect(wsUrl);
      _channel = channel;

      channel.sink.add(token);

      _sub = channel.stream.listen(
        (data) => _handleMessage(channel, data),
        onDone: () => _handleDisconnect('Connection closed.'),
        onError: (err) => _handleDisconnect('Error: ${err.toString()}'),
        cancelOnError: true,
      );
    } catch (e) {
      _handleDisconnect('WebSocket exception: ${e.toString()}');
    }
  }

  void _handleMessage(WebSocketChannel channel, dynamic data) {
    final passcode = globalPasscode;
    if (passcode == null) {
      _statusNotifier.value = 'Security session expired. Please re-authenticate.';
      _connectingNotifier.value = false;
      return;
    }
    final sessionKey = sha256.convert(utf8.encode(passcode)).toString();

    if (data is String) {
      if (data == 'HOST_LOCKED') {
        _statusNotifier.value = 'Host is locked.';
        return;
      } else if (data == 'NEED_AUTH') {
        // Send encrypted authenticate message immediately
        final authPayload = jsonEncode({'action': 'authenticate', 'params': {}});
        final encryptedAuth = encryptAESGCM(authPayload, sessionKey);
        channel.sink.add(encryptedAuth);
        return;
      } else if (data == 'AUTH_FAILED') {
        _statusNotifier.value = 'Authentication failed. Re-pair your device.';
        _connectingNotifier.value = false;
        return;
      }

      // Decrypt the text frame using sessionKey
      String decrypted;
      try {
        decrypted = decryptAESGCM(data, sessionKey);
      } catch (e) {
        debugPrint('Failed to decrypt text message in remote view: $e');
        return;
      }

      if (decrypted.startsWith('AUTH_OK')) {
        final parts = decrypted.split(' ');
        if (parts.length >= 3) {
          final w = int.tryParse(parts[1]);
          final h = int.tryParse(parts[2]);
          if (w != null && h != null) {
            setState(() {
              _desktopWidth = w;
              _desktopHeight = h;
            });
          }
        }
        _requestStream(channel);
      } else if (decrypted.trim().startsWith('{')) {
        try {
          final payload = jsonDecode(decrypted) as Map<String, dynamic>;
          if (payload['type'] == 'screen_info' || payload['type'] == 'camera_info') {
            final w = payload['width'];
            final h = payload['height'];
            if (w is num && h is num) {
              setState(() {
                _desktopWidth = w.toInt();
                _desktopHeight = h.toInt();
              });
            }
          } else if (payload['type'] == 'error') {
            final errMsg = payload['message'] ?? 'An error occurred';
            setState(() {
              _statusNotifier.value = errMsg;
              _connectingNotifier.value = true;
            });
          }
        } catch (_) {}
      }
      return;
    }

    // Binary message represents encrypted frame
    final encryptedFrame = _toUint8List(data);
    if (encryptedFrame == null || encryptedFrame.isEmpty) return;

    try {
      final decryptedFrame = decryptAESGCMBin(encryptedFrame, sessionKey);
      _frameNotifier.value = decryptedFrame;
      if (_connectingNotifier.value) {
        _connectingNotifier.value = false;
        _reconnectDelay = 1;
      }
      _resetFrameWatchdog();
    } catch (e) {
      debugPrint('Failed to decrypt screen frame binary: $e');
    }
  }

  Uint8List? _toUint8List(dynamic data) {
    if (data is Uint8List) return data;
    if (data is List<int>) return Uint8List.fromList(data);
    return null;
  }

  void _requestStream(WebSocketChannel channel) {
    if (_streamRequested) return;
    _streamRequested = true;
    if (_streamSource == StreamSource.camera) {
      _statusNotifier.value = 'Requesting camera...';
      _sendEncryptedCommand({'action': 'start_camera', 'params': {}});
    } else {
      _statusNotifier.value = 'Requesting screen...';
      _sendEncryptedCommand({'action': 'start_screen', 'params': {}});
    }
    _resetFrameWatchdog();
  }

  void _toggleStreamSource() {
    setState(() {
      if (_streamSource == StreamSource.screen) {
        _streamSource = StreamSource.camera;
        _statusNotifier.value = 'Requesting camera...';
        _sendEncryptedCommand({'action': 'start_camera', 'params': {}});
      } else {
        _streamSource = StreamSource.screen;
        _statusNotifier.value = 'Requesting screen...';
        _sendEncryptedCommand({'action': 'start_screen', 'params': {}});
      }
    });
    _resetFrameWatchdog();
  }

  void _resetFrameWatchdog() {
    _frameWatchdog?.cancel();
    _frameWatchdog = Timer(const Duration(seconds: 8), () {
      if (_disposed || !_connectingNotifier.value && _frameNotifier.value != null) return;
      _statusNotifier.value = 'No frames received. Check desktop is unlocked and on same network.';
      _connectingNotifier.value = true;
    });
  }

  void _handleDisconnect(String reason) {
    if (_disposed) return;
    _frameWatchdog?.cancel();
    _sub?.cancel();
    _channel?.sink.close();
    _channel = null;
    _frameNotifier.value = null;
    _statusNotifier.value = '$reason\nReconnecting in ${_reconnectDelay}s...';
    _connectingNotifier.value = true;

    Future.delayed(Duration(seconds: _reconnectDelay), () {
      if (!_disposed) {
        _reconnectDelay = (_reconnectDelay * 2).clamp(1, 16);
        _connect();
      }
    });
  }

  void _sendEncryptedCommand(Map<String, dynamic> payload) {
    if (_channel == null) return;
    final passcode = globalPasscode;
    if (passcode == null) return;
    final sessionKey = sha256.convert(utf8.encode(passcode)).toString();
    final encrypted = encryptAESGCM(jsonEncode(payload), sessionKey);
    _channel!.sink.add(encrypted);
  }

  void _sendMouseEvent(String action, Offset localPosition, Size viewSize) {
    if (_channel == null || viewSize.width <= 0 || viewSize.height <= 0) return;
    final double scaleX = _desktopWidth / viewSize.width;
    final double scaleY = _desktopHeight / viewSize.height;
    final payload = {
      'action': action,
      'params': {
        'x': (localPosition.dx * scaleX).toInt(),
        'y': (localPosition.dy * scaleY).toInt(),
      },
    };
    _sendEncryptedCommand(payload);
  }

  void _onTrackpadPointerDown(PointerDownEvent event) {
    _pointerPositions[event.pointer] = event.position;
    
    if (_pointerPositions.length == 1) {
      _oneFingerStartPos = event.position;
      _oneFingerStartTime = DateTime.now();
    } else if (_pointerPositions.length == 2) {
      final list = _pointerPositions.values.toList();
      _twoFingerStart = (list[0] + list[1]) / 2;
      _scrollYAccumulator = 0.0;
      _oneFingerStartPos = null;
    } else if (_pointerPositions.length == 3) {
      final list = _pointerPositions.values.toList();
      _threeFingerStart = (list[0] + list[1] + list[2]) / 3;
      _threeFingerGestureTriggered = false;
      _oneFingerStartPos = null;
    }
  }

  void _onTrackpadPointerMove(PointerMoveEvent event) {
    final prevPos = _pointerPositions[event.pointer];
    _pointerPositions[event.pointer] = event.position;
    
    if (prevPos == null) return;
    
    if (_pointerPositions.length == 1) {
      final delta = event.position - prevPos;
      const double sensitivity = 1.8;
      
      setState(() {
        _cursorX = (_cursorX + delta.dx * sensitivity).clamp(0.0, _desktopWidth.toDouble());
        _cursorY = (_cursorY + delta.dy * sensitivity).clamp(0.0, _desktopHeight.toDouble());
      });
      
      _sendEncryptedCommand({
        'action': 'move_mouse',
        'params': {
          'x': _cursorX.toInt(),
          'y': _cursorY.toInt(),
        }
      });
    } else if (_pointerPositions.length == 2 && _twoFingerStart != null) {
      final delta = event.delta;
      _scrollYAccumulator += delta.dy;
      const double scrollThreshold = 15.0;
      
      if (_scrollYAccumulator.abs() >= scrollThreshold) {
        final direction = _scrollYAccumulator > 0 ? 'down' : 'up';
        final clicks = (_scrollYAccumulator.abs() / scrollThreshold).floor();
        
        _sendEncryptedCommand({
          'action': 'scroll',
          'params': {
            'clicks': clicks,
            'direction': direction,
          }
        });
        _scrollYAccumulator = _scrollYAccumulator % scrollThreshold;
      }
    } else if (_pointerPositions.length == 3 && _threeFingerStart != null && !_threeFingerGestureTriggered) {
      final list = _pointerPositions.values.toList();
      final currentAvg = (list[0] + list[1] + list[2]) / 3;
      final swipeDelta = currentAvg - _threeFingerStart!;
      const double swipeThreshold = 40.0;
      
      if (swipeDelta.dy.abs() > swipeThreshold && swipeDelta.dy.abs() > swipeDelta.dx.abs()) {
        _threeFingerGestureTriggered = true;
        if (swipeDelta.dy < 0) {
          _sendEncryptedCommand({
            'action': 'press_shortcut',
            'params': {'shortcut': 'win+tab'},
          });
        } else {
          _sendEncryptedCommand({
            'action': 'press_shortcut',
            'params': {'shortcut': 'win+d'},
          });
        }
      } else if (swipeDelta.dx.abs() > swipeThreshold && swipeDelta.dx.abs() > swipeDelta.dy.abs()) {
        _threeFingerGestureTriggered = true;
        _sendEncryptedCommand({
          'action': 'press_shortcut',
          'params': {'shortcut': 'alt+tab'},
        });
      }
    }
  }

  void _onTrackpadPointerUp(PointerUpEvent event) {
    final prevLen = _pointerPositions.length;
    _pointerPositions.remove(event.pointer);
    
    if (prevLen == 1 && _pointerPositions.isEmpty && _oneFingerStartPos != null && _oneFingerStartTime != null) {
      final elapsed = DateTime.now().difference(_oneFingerStartTime!);
      final dist = (event.position - _oneFingerStartPos!).distance;
      
      if (elapsed.inMilliseconds < 300 && dist < 15.0) {
        _sendEncryptedCommand({
          'action': 'click',
          'params': {'button': 'left'}
        });
      }
    }
    
    if (_pointerPositions.length < 2) {
      _twoFingerStart = null;
    }
    if (_pointerPositions.length < 3) {
      _threeFingerStart = null;
    }
  }

  void _onTrackpadPointerCancel(PointerCancelEvent event) {
    _pointerPositions.remove(event.pointer);
    if (_pointerPositions.length < 2) {
      _twoFingerStart = null;
    }
    if (_pointerPositions.length < 3) {
      _threeFingerStart = null;
    }
  }

  void _sendClickEvent(String button) {
    _sendEncryptedCommand({'action': 'click', 'params': {'button': button}});
  }

  void _sendSpecialKeyEvent(String keyName) {
    _sendEncryptedCommand({'action': 'press_key', 'params': {'key': keyName}});
  }

  void _sendTypeTextEvent(String text) {
    _sendEncryptedCommand({'action': 'type_text', 'params': {'text': text}});
  }

  void _handleTapDown(TapDownDetails d, Size viewSize) {
    if (_activeMode != ControlMode.touch) return;
    _sendMouseEvent('move_mouse', d.localPosition, viewSize);
    _sendClickEvent('left');
  }

  void _handleLongPressStart(LongPressStartDetails d, Size viewSize) {
    if (_activeMode != ControlMode.touch) return;
    _sendMouseEvent('move_mouse', d.localPosition, viewSize);
    _sendClickEvent('right');
  }

  void _handlePanStart(DragStartDetails d, Size viewSize) {
    if (_activeMode != ControlMode.touch) return;
    _sendMouseEvent('move_mouse', d.localPosition, viewSize);
    _sendEncryptedCommand({'action': 'mouse_down', 'params': {'button': 'left'}});
  }

  void _handlePanUpdate(DragUpdateDetails d, Size viewSize) {
    if (_activeMode != ControlMode.touch) return;
    _sendMouseEvent('move_mouse', d.localPosition, viewSize);
  }

  void _handlePanEnd(DragEndDetails d) {
    if (_activeMode != ControlMode.touch) return;
    _sendEncryptedCommand({'action': 'mouse_up', 'params': {'button': 'left'}});
  }

  Widget _buildModeTab(ControlMode mode, IconData icon, String label) {
    final isActive = _activeMode == mode;
    return GestureDetector(
      onTap: () {
        setState(() {
          _activeMode = mode;
        });
      },
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        decoration: BoxDecoration(
          color: isActive ? const Color(0xFF00F3FF).withOpacity(0.15) : Colors.transparent,
          borderRadius: BorderRadius.circular(16),
        ),
        child: Row(
          children: [
            Icon(
              icon,
              size: 16,
              color: isActive ? const Color(0xFF00F3FF) : Colors.white54,
            ),
            const SizedBox(width: 4),
            Text(
              label,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.bold,
                color: isActive ? const Color(0xFF00F3FF) : Colors.white54,
                fontFamily: 'monospace',
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSpecialKeyBtn(String label, String keyName) {
    return Container(
      margin: const EdgeInsets.only(right: 6),
      child: ElevatedButton(
        style: ElevatedButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          backgroundColor: Colors.white.withOpacity(0.08),
          foregroundColor: const Color(0xFF00F3FF),
          minimumSize: const Size(40, 35),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(4),
            side: BorderSide(color: Colors.cyan.withOpacity(0.3)),
          ),
        ),
        onPressed: () => _sendSpecialKeyEvent(keyName),
        child: Text(label,
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12, fontWeight: FontWeight.bold)),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: Stack(
        fit: StackFit.expand,
        children: [
          // Live screen frame — Positioned MUST be a direct child of Stack
          Positioned.fill(
            child: RepaintBoundary(
              child: InteractiveViewer(
                maxScale: 4.0,
                minScale: 1.0,
                clipBehavior: Clip.none,
                child: Center(
                  child: AspectRatio(
                    aspectRatio: _desktopWidth / _desktopHeight,
                    child: LayoutBuilder(
                      builder: (context, constraints) {
                        final viewSize = Size(constraints.maxWidth, constraints.maxHeight);
                        
                        Widget imageWidget = ValueListenableBuilder<Uint8List?>(
                          valueListenable: _frameNotifier,
                          builder: (context, frame, _) {
                            if (frame == null) {
                              return Container(
                                color: const Color(0xFF1A1A1A),
                                alignment: Alignment.center,
                                child: ValueListenableBuilder<String>(
                                  valueListenable: _statusNotifier,
                                  builder: (context, status, _) => Text(
                                    status,
                                    style: const TextStyle(
                                      color: Colors.white54,
                                      fontSize: 12,
                                      fontFamily: 'monospace',
                                    ),
                                    textAlign: TextAlign.center,
                                  ),
                                ),
                              );
                            }
                            return Image.memory(
                              frame,
                              gaplessPlayback: true,
                              filterQuality: FilterQuality.low,
                              fit: BoxFit.contain,
                              width: double.infinity,
                              height: double.infinity,
                              errorBuilder: (context, error, stackTrace) {
                                return Container(
                                  color: const Color(0xFF1A1A1A),
                                  alignment: Alignment.center,
                                  child: Column(
                                    mainAxisAlignment: MainAxisAlignment.center,
                                    children: [
                                      const Icon(Icons.broken_image, color: Colors.redAccent, size: 36),
                                      const SizedBox(height: 8),
                                      Text(
                                        'Decode Error:\n$error',
                                        style: const TextStyle(
                                          color: Colors.white70,
                                          fontSize: 11,
                                          fontFamily: 'monospace',
                                        ),
                                        textAlign: TextAlign.center,
                                      ),
                                    ],
                                  ),
                                );
                              },
                            );
                          },
                        );

                        if (_activeMode == ControlMode.trackpad) {
                          final localX = _cursorX * (viewSize.width / _desktopWidth);
                          final localY = _cursorY * (viewSize.height / _desktopHeight);
                          return Listener(
                            onPointerDown: _onTrackpadPointerDown,
                            onPointerMove: _onTrackpadPointerMove,
                            onPointerUp: _onTrackpadPointerUp,
                            onPointerCancel: _onTrackpadPointerCancel,
                            child: Stack(
                              clipBehavior: Clip.none,
                              children: [
                                Positioned.fill(child: imageWidget),
                                Positioned(
                                  left: localX,
                                  top: localY,
                                  child: IgnorePointer(
                                    child: Transform.rotate(
                                      angle: -3.14159 / 4,
                                      child: const Icon(
                                        Icons.navigation,
                                        size: 22,
                                        color: Color(0xFF00F3FF),
                                        shadows: [
                                          Shadow(
                                            blurRadius: 4.0,
                                            color: Colors.black87,
                                            offset: Offset(1.0, 1.0),
                                          )
                                        ],
                                      ),
                                    ),
                                  ),
                                ),
                              ],
                            ),
                          );
                        } else if (_activeMode == ControlMode.touch) {
                          return GestureDetector(
                            onTapDown: (d) => _handleTapDown(d, viewSize),
                            onLongPressStart: (d) => _handleLongPressStart(d, viewSize),
                            onPanStart: (d) => _handlePanStart(d, viewSize),
                            onPanUpdate: (d) => _handlePanUpdate(d, viewSize),
                            onPanEnd: (d) => _handlePanEnd(d),
                            child: imageWidget,
                          );
                        } else {
                          return imageWidget;
                        }
                      },
                    ),
                  ),
                ),
              ),
            ),
          ),

          // Connecting overlay
          ValueListenableBuilder<bool>(
            valueListenable: _connectingNotifier,
            builder: (context, isConnecting, _) {
              if (!isConnecting) return const SizedBox.shrink();
              return Container(
                color: Colors.black87,
                child: Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const CircularProgressIndicator(color: Color(0xFF00F3FF)),
                      const SizedBox(height: 20),
                      ValueListenableBuilder<String>(
                        valueListenable: _statusNotifier,
                        builder: (context, status, _) => Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 24),
                          child: Text(
                            status,
                            style: const TextStyle(
                              color: Colors.white70,
                              fontSize: 13,
                              fontFamily: 'monospace',
                            ),
                            textAlign: TextAlign.center,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              );
            },
          ),

          Positioned(
            top: 40,
            left: 20,
            child: Container(
              decoration: BoxDecoration(
                color: Colors.black.withOpacity(0.6),
                borderRadius: BorderRadius.circular(20),
                border: Border.all(color: Colors.cyan.withOpacity(0.3)),
              ),
              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _buildModeTab(ControlMode.touch, Icons.touch_app, 'Touch'),
                  _buildModeTab(ControlMode.trackpad, Icons.mouse, 'Trackpad'),
                  _buildModeTab(ControlMode.view, Icons.zoom_in, 'View'),
                ],
              ),
            ),
          ),

          Positioned(
            top: 40,
            right: 70,
            child: FloatingActionButton.small(
              backgroundColor: const Color(0xFF00F3FF).withOpacity(0.8),
              foregroundColor: Colors.black,
              child: Icon(_streamSource == StreamSource.screen ? Icons.videocam : Icons.monitor),
              onPressed: _toggleStreamSource,
            ),
          ),

          Positioned(
            top: 40,
            right: 20,
            child: FloatingActionButton.small(
              backgroundColor: const Color(0xFF00F3FF).withOpacity(0.8),
              foregroundColor: Colors.black,
              child: Icon(_showKeyboard ? Icons.keyboard_hide : Icons.keyboard),
              onPressed: () => setState(() => _showKeyboard = !_showKeyboard),
            ),
          ),

          if (_showKeyboard)
            Positioned(
              bottom: 0,
              left: 0,
              right: 0,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                decoration: BoxDecoration(
                  color: const Color(0xFF0A0B14).withOpacity(0.9),
                  borderRadius: const BorderRadius.only(
                    topLeft: Radius.circular(16),
                    topRight: Radius.circular(16),
                  ),
                  border: Border.all(color: Colors.cyan.withOpacity(0.2)),
                ),
                child: SafeArea(
                  top: false,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Row(
                        children: [
                          Expanded(
                            child: TextField(
                              controller: _textInputController,
                              style: const TextStyle(color: Colors.white, fontSize: 14, fontFamily: 'monospace'),
                              decoration: InputDecoration(
                                hintText: 'TYPE SYSTEM INPUT...',
                                hintStyle: TextStyle(color: Colors.white.withOpacity(0.3), fontSize: 12),
                                border: const OutlineInputBorder(),
                                contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
                              ),
                              onSubmitted: (val) {
                                if (val.isNotEmpty) {
                                  _sendTypeTextEvent(val);
                                  _textInputController.clear();
                                }
                              },
                            ),
                          ),
                          const SizedBox(width: 8),
                          IconButton(
                            icon: const Icon(Icons.send, color: Color(0xFF00F3FF)),
                            onPressed: () {
                              final val = _textInputController.text;
                              if (val.isNotEmpty) {
                                _sendTypeTextEvent(val);
                                _textInputController.clear();
                              }
                            },
                          ),
                        ],
                      ),
                      const SizedBox(height: 10),
                      SingleChildScrollView(
                        scrollDirection: Axis.horizontal,
                        child: Row(
                          children: [
                            _buildSpecialKeyBtn('Win', 'win'),
                            _buildSpecialKeyBtn('Esc', 'esc'),
                            _buildSpecialKeyBtn('Tab', 'tab'),
                            _buildSpecialKeyBtn('Enter', 'enter'),
                            _buildSpecialKeyBtn('Bksp', 'backspace'),
                            _buildSpecialKeyBtn('Space', 'space'),
                            _buildSpecialKeyBtn('Ctrl', 'ctrl'),
                            _buildSpecialKeyBtn('Alt', 'alt'),
                            _buildSpecialKeyBtn('Shift', 'shift'),
                            _buildSpecialKeyBtn('◀', 'left'),
                            _buildSpecialKeyBtn('▲', 'up'),
                            _buildSpecialKeyBtn('▼', 'down'),
                            _buildSpecialKeyBtn('▶', 'right'),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
