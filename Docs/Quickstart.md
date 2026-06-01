# Domain Blocker — Quickstart

## Prerequisites

- [Rust](https://rustup.rs/) + rustup (1.70+)
- Flutter 3.0+
- Android NDK (for Android builds)

---

## 1. Clone and enter the project

```bash
git clone <repo-url>
cd domain-block
```

---

## 2. Run setup (first time only)

```bash
bash setup.sh
```

Pick **option 5** — builds Rust and integrates into your Flutter app in one go.

---

## 3. Or run manually

```bash
# Build for your target platform
./build.sh -t android -r     # Android
./build.sh -t linux -r       # Linux desktop
./build.sh -t all -r         # All platforms

# Copy native libraries into your Flutter project
./migrate_to_flutter.sh /path/to/your/flutter/app
```

The migrate script copies native libraries into platform-specific directories, updates `pubspec.yaml`, and runs `flutter pub get`.

---

## 4. Use in Dart

```dart
import 'package:your_app/services/domain_blocker_service.dart';

// Block a domain
await DomainBlockerService.insert('ads.example.com');

// Check if blocked
bool blocked = await DomainBlockerService.isBlocked('ads.example.com');
```

---

## Supported platforms

| Platform | Build target |
|----------|-------------|
| Android  | `-t android` |
| iOS      | `-t ios`     |
| Linux    | `-t linux`   |
| macOS    | `-t macos`   |
| Windows  | `-t windows` |

---

## Troubleshooting

**`EM_X86_64 instead of EM_AARCH64` crash on Android**
Stale host-platform `.so` in your JNI libs directory. Rebuild and re-migrate:
```bash
./build.sh -t android -r
./migrate_to_flutter.sh /path/to/flutter/app
```
The migrate script removes stale artifacts automatically.

**`flutter` not found**
The script still copies the libraries. Run `flutter pub get` manually afterward.

**Missing Android NDK**
Install via Android Studio → SDK Manager → SDK Tools → NDK.
