// This is a basic Flutter widget test.

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:avatar_mobile/main.dart';

void main() {
  testWidgets('App loads smoke test', (WidgetTester tester) async {
    // Initialize SharedPreferences with empty mock values for test execution
    SharedPreferences.setMockInitialValues({});

    // Build our app and trigger a frame.
    await tester.pumpWidget(const AvatarMobileApp());
    await tester.pump();

    // Verify that our app starts at the login/PIN screen.
    expect(find.text('AVATAR CORE'), findsOneWidget);
  });
}

