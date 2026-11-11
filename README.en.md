<p align="center">
  <img src="assets/icon.png" width="112" alt="Lenovo Toolkit icon">
</p>

<h1 align="center">Lenovo Toolkit</h1>

<p align="center"><b>A small Windows app for Lenovo Legion, LOQ and IdeaPad Gaming laptops.</b></p>

<p align="center">
  Power modes, fans and sensors, battery, lights and Fn keys in one light window that lives in the tray.<br>
  A Rust port of LenovoLegionToolkit with a Slint interface, about 10 MB, no .NET.
</p>

<p align="center"><a href="README.md">Tiếng Việt</a> · <b>English</b></p>

> This is an unofficial project and is not affiliated with Lenovo. It writes to firmware through the Lenovo drivers and WMI, so use it at your own risk. Download builds only from this repository's [Releases](https://github.com/erikpuw/lenovo-toolkit/releases). The app is not code-signed, so Windows may show a SmartScreen warning the first time.

## Features

| Feature | What it does |
|---|---|
| Power modes | Quiet, Balance, Performance, Extreme and Custom, as far as your laptop supports them. Fn+Q works and the app follows it. Performance, Extreme and Custom need the charger plugged in, and on BIOS versions with known switching bugs the app goes through an intermediate mode first, as the original does. In Balance, the gear button opens the AI Engine switch. |
| Custom mode | Two presets. The Lenovo default is locked. The Custom preset edits the firmware limits in the firmware's own steps and warns on risky values. Full fan speed is off by default and is written off again on every apply, so the fans are never left at 100%. |
| Fans and sensors | CPU, GPU and PCH temperature, fan speed, CPU, GPU and RAM usage, battery level. |
| Battery | Charge mode (Normal, Rapid, Conservation at 80%), overnight charge, always-on USB. A Battery page shows temperature, charge and discharge rate, capacity, health, cycle count and dates. |
| Graphics and display | GPU working mode, discrete GPU status, GPU overclock switch where supported, refresh rate, overdrive. Fn+R cycles the refresh rate. |
| Keys and lights | Keyboard backlight, logo and port lights, touchpad lock, Fn lock, Windows key lock. Fn+F4 mutes the microphone and Fn+F9 opens the app. |
| Key notifications | A short message at the bottom of the screen for power mode, Fn lock, camera, microphone, touchpad and keyboard backlight, each with its own icon. Every type can be switched off in Settings. Nothing shows over fullscreen games or presentations. |
| Lenovo software | Vantage, Legion Zone, Legion Space and the Lenovo hotkey service can be disabled. The app applies the choice again at every start. |
| Tray | Hover shows power mode, CPU, GPU, RAM, fans and battery. The right-click menu switches power mode. A window hidden in the tray stops reading sensors and gives its memory back. |
| Autorun | A scheduled task at logon starts the app hidden in the tray. |
| Languages | English and Vietnamese. |

## Supported laptops

| Group | Lines | Status |
|---|---|---|
| Gaming | Legion, LOQ, IdeaPad Gaming | Everything above |
| Not gaming | ThinkBook, IdeaPad, Slim, Yoga, XiaoXin, Motobook | Not supported. These laptops use Intelligent Thermal Solution (Auto, Cool, Performance, Geek) instead of the Legion power modes. The app only reads sensors there, hides every control that writes to hardware and leaves the Lenovo software alone. |

The group comes from the model name. It has only been tested on a Legion Y7000 IRX9.

## Install

1. Download `lenovo-toolkit.exe` from Releases and run it. It asks for administrator permission because it talks to the drivers.
2. To start with Windows, open Settings, App Behavior and turn Autorun on.

## Build from source

You need Windows x64, a recent stable Rust (edition 2024) and the Windows SDK, which provides `rc.exe`.

```
cargo test --workspace
cargo build --release
```

The binaries are `target/release/lenovo-toolkit.exe` and `target/release/lenovo-toolkit-cli.exe`. The app embeds an administrator manifest, so run `cargo run --release -p lenovo-toolkit` from an elevated terminal.

## Command line

`lenovo-toolkit-cli` covers most of the app. Commands that read from the battery driver or the registry work as a normal user. The ones that use WMI or write to hardware need administrator rights.

```
lenovo-toolkit-cli battery get
lenovo-toolkit-cli battery set <normal|rapid|conservation>
lenovo-toolkit-cli power get
lenovo-toolkit-cli power set <quiet|balance|performance|extreme|godmode>
lenovo-toolkit-cli gamezone <winkey|gsync|overdrive|touchpad|igpu> <get|supported|set <n>>
lenovo-toolkit-cli <fnlock|nightcharge|onelevel> <get|set <on|off>>
lenovo-toolkit-cli fan <sensors|max|fullspeed <get|set <on|off>>>
lenovo-toolkit-cli light <ports|logo|kbd> <get|supported|set <n>>
lenovo-toolkit-cli usb <get|set <off|sleeping|always>>
lenovo-toolkit-cli kbd <get|max|set <level>>
lenovo-toolkit-cli threshold <get|off|set <start> <stop>>
lenovo-toolkit-cli info
lenovo-toolkit-cli ai <status|refresh|set <on|off>>
lenovo-toolkit-cli custom show
lenovo-toolkit-cli games
lenovo-toolkit-cli software status
lenovo-toolkit-cli software <fnkeys|vantage|legionzone|legionspace> <disable|enable>
lenovo-toolkit-cli keys
lenovo-toolkit-cli events
```

`threshold` needs the registry key `HKLM\SOFTWARE\WOW6432Node\Lenovo\PWRMGRV`, which Lenovo's own software creates. Without it the command reports that the laptop is not supported.

## Left out

Macros, automation, downloads, Spectrum RGB, the OSD, update checks and integrations are not ported, on purpose. Still missing:

- ITS laptops (see above).
- Notifications for the AC adapter, Caps Lock and Num Lock.
- The Fn+F8 airplane mode key and the LED on the Fn+F4 key.
- Moving or resizing notifications.

## Project layout

| Crate | Contents |
|---|---|
| `lenovo-toolkit` | The app: Slint interface, Win32 tray, key notifications. |
| `lenovo-toolkit-core` | Drivers (`EnergyDrv`, `IBMPmDrv`), WMI, battery, fans, lights, keys, settings. |
| `lenovo-toolkit-cli` | The command line. |

## Credits and license

Released under the GNU General Public License v3.0, see [LICENSE](LICENSE).

The ideas and the way this app talks to Lenovo hardware come from [LenovoLegionToolkit](https://github.com/BartoszCichecki/LenovoLegionToolkit) by Bartosz Cichecki, who created it, and from the version the [LenovoLegionToolkit-Team](https://github.com/LenovoLegionToolkit-Team/LenovoLegionToolkit) keeps developing. Both are under GPL-3.0. This version was rewritten in Rust and drops most of their features. Thanks to the authors.
