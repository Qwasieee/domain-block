# Domain Blocker — Quickstart

## Prerequisites

- [Rust](https://rustup.rs/) + rustup
- Flutter 3.x
- Android NDK (for Android builds)

---

## 1. Clone & enter the project

```bash
git clone <repo-url>
cd domain-block
```

---

## 2. Run setup (first time only)

```bash
bash setup.sh
```

Pick **option 5** — it builds Rust and integrates into your Flutter app in one go.

---

## 3. Or run manually

```bash
# Build for your target platform
./build.sh -t android -r     # Android
./build.sh -t linux -r       # Linux desktop
./build.sh -t all -r         # Everything

# Integrate into Flutter
./migrate_to_flutter.sh /path/to/your/flutter/app
```

That's it. The script copies native libraries, updates `pubspec.yaml`, and runs `flutter pub get` automatically.

---

## 4. Use in Dart

```dart
import 'package:your_app/services/domain_blocker_service.dart';

// Block a domain
await DomainBlocker.insert('ads.example.com');

// Check if blocked
bool blocked = await DomainBlocker.isBlocked('ads.example.com');

// Remove
await DomainBlocker.remove('ads.example.com');
```

---

## Supported Platforms

| Platform | Build target      |
|----------|-------------------|
| Android  | `-t android`      |
| iOS      | `-t ios`          |
| Linux    | `-t linux`        |
| macOS    | `-t macos`        |
| Windows  | `-t windows`      |

---

## Troubleshooting

**`EM_X86_64 instead of EM_AARCH64` crash on Android**
You have a stale host-platform `.so` in your JNI libs. Fix:
```bash
./build.sh -t android -r
./migrate_to_flutter.sh /path/to/flutter/app
```
The migrate script cleans stale artifacts automatically on each run.

**`flutter` not found**
The script will still copy libraries — just run `flutter pub get` manually afterward.

**Missing Android NDK**
Install via Android Studio → SDK Manager → SDK Tools → NDK.