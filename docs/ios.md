# Horizon on iOS

The game runs on iPhone and iPad through wgpu's Metal backend. The iOS app is the
`horizon-ios` crate: a small executable that builds the game's command line and calls
`horizon_client::entry::main_with_args`. Everything else is the same gameplay, renderer and
server code the desktop game runs.

Status: early. The game boots, renders the title screen and worlds through Metal, single
player works (on its integrated server thread), touch controls cover the basics, and an iPad
plays with a hardware keyboard and mouse or trackpad as the desktop game does. Expect
rough edges: see [What's left](#whats-left).

## Build and run (simulator)

You need Xcode, rustup (Homebrew's `rust` has no iOS standard library), and the game's resources
prepared from your own copy of Minecraft, as for the desktop game.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# 1. The pinned Minecraft files, from Mojang (dev only, into .local/mojang; never committed),
#    and a JDK of the pinned major version for the exporters.
python3 tools/ios/fetch_minecraft.py
# 2. Prepare the resources (.local/horizon-client/26.3).
cargo build --profile fast -p horizon-prepare
./target/fast/horizon-prepare --prepare --complete --minecraft-dir .local/mojang \
    --server-jar .local/mojang/server/server-26.3.jar --java-home /path/to/jdk-25 --root "$PWD"
# 3. Build Horizon.app and install it on a booted simulator.
tools/ios/build_app.sh --install <simulator UDID>
```

`tools/ios/build_app.sh` builds `horizon-ios` with `--profile fast`, assembles
`target/ios/<target>/Horizon.app` (the executable, `Info.plist` from `crates/horizon-ios`, and the
resources under `GameData/`), and signs it ad hoc. `NO_SOUNDS=1` leaves the 380 MB of sounds out.

For a device: `tools/ios/build_app.sh --device --sign "Apple Development: …" --profile
path/to/profile.mobileprovision`, then install with Xcode's Devices window or `xcrun devicectl`.

On the device the game folder (saves, `options.txt`) is `Documents/Horizon` and the log and
crash reports are in `Documents/Horizon/.horizon`; the Files app shows them.

Extra arguments for development go after the bundle id:
`xcrun simctl launch <udid> dev.horizonlab.horizon.ios --world Test --creative`.

## Touch controls

`crates/horizon-client/src/game/touch.rs`. In play:

| Where | What |
|---|---|
| Left 40% of the screen (below the top quarter) | A floating stick: walk |
| Anywhere else, drag | Turn the view |
| Anywhere else, quick tap | Use / place (the use trigger) |
| Anywhere else, press and hold still | Mine / attack (the attack trigger) |
| `^` `v` `>>` | Jump, sneak, sprint (held) |
| `X` `O` | Attack, use (held) |
| `…` `@` `‖` (right edge) | Inventory, camera, pause |
| The hotbar | Select that slot |

The buttons press the virtual pad's buttons (the one `horizon-ctl pad` drives), so they do what
the controller bindings say and the game shows controller hints. In menus and the inventory a
finger is the mouse (down presses, moving drags, up releases), two fingers scroll, and `<` at the
top left closes a screen opened in play (the pad's B). The system keyboard comes up when a text
field is tapped and goes away on Return or a tap elsewhere.

## Keyboard and mouse

`crates/horizon-ios-input` (GameController) and `crates/horizon-client/src/game/hardware_input.rs`.
A keyboard and a mouse or trackpad (the Magic Keyboard's included) play as on the desktop: their
input becomes the window events the desktop's keyboard and mouse send, so the key bindings,
screens, chat and native mods take it unchanged.

| Device | What |
|---|---|
| Keys | The desktop's keys and bindings (WASD, Space, Shift, E, the digits, T and `/`, Esc, F1 to F12 where the keyboard has them), held keys repeat |
| Text | Typed through UIKit while a text field has focus, in the keyboard's own layout; the on-screen keyboard stays away |
| Mouse in play | The pointer is locked and hidden, as the desktop grabs the mouse; its raw movement turns the view, left attacks, right uses, middle picks, the wheel changes the hotbar slot |
| Mouse over screens | The system's own pointer, with its speed and scroll settings: hover, tooltips, slot hover, clicks, drags and scrolling as on the desktop |

The touch controls hide once a key is pressed, a mouse button clicked or the pointer moved a
little (a palm brushing the trackpad doesn't count), and come back when a finger touches the
screen. The log says which is in use (`keyboard and mouse are the input in use`, `touch is the
input in use`), when a keyboard or mouse connects, and each pointer lock asked for and granted
(`pointer lock wanted`, `pointer lock requested`, `the system's pointer lock changed`, `pointer
locked`).

Touches the controls make on the virtual pad don't count as a controller: no controller guide,
glyphs or menu focus appear for them.

## Platform changes

- **Metal only**: `backend_order` takes wgpu's default on iOS as on macOS; the options hide the
  graphics API row. The iOS Simulator's GPU has no indirect execution, so the flat terrain draws
  and occlusion culling check `DownlevelFlags::INDIRECT_EXECUTION` and fall back to per-section
  draws there (real iPhones and iPads have it).
- **The window**: on iOS winit's inner size is the safe area while the Metal layer fills the
  screen. `window::surface_size` (the outer size there) sizes the surface and `Resized`;
  `window::for_platform` drops the desktop window size and fullscreen, keeps landscape and defers
  the system's edge gestures.
- **No pointer to grab**: `capture` keeps the grab as the game's own state on iOS, as for
  offscreen runs; touch and a hardware mouse turn the view through `MouseHandler.onMove`.
- **Keyboard and mouse**: winit 0.30's iOS backend sends no hardware key presses (only
  `insertText:` while its view has the keyboard focus) and turns the pointer into touches. Keys
  come from GameController's `GCKeyboard`. The pointer is locked only while the game holds it
  (`capture`): winit's view controller gets a `prefersPointerLocked` method, and the game asks
  again if the system hasn't locked it within a second. In play, movement, buttons and the wheel
  come from `GCMouse`. Over screens they come from UIKit, on recognizers added to winit's view
  that hold no touch back from it: a `UIHoverGestureRecognizer` for hovering, one that sees only
  `UITouchTypeIndirectPointer` touches for presses, drags and releases (the event's
  `buttonMask` says which button; each moves the cursor to the event's place first, and those
  touches never reach the touch controls), and a `UIPanGestureRecognizer` that takes only
  scrolling. Pointer places reported while the pointer is locked are dropped: the system then
  still sends clicks as touches, at the window's corner. The in-play wheel assumes natural
  scrolling, iPadOS' default. `horizon-ios-input` builds winit's `KeyEvent` itself, which relies
  on its iOS `platform_specific` field being empty (a size check fails the build if a winit
  update changes that).
- **Main thread stack**: iOS gives the main thread 1 MB and the startup overflowed it;
  `crates/horizon-ios/build.rs` links the app with a 16 MB one.
- **Crates that don't build for iOS**: `arboard` (the clipboard stays in the game), the Vulkan
  HDR metadata (`horizon-hdr`, Vulkan only), `proc_pidinfo` (macOS only), winit's
  `key_without_modifiers` (no keyboard layouts to learn).
- **UIScene**: UIKit in the iOS 27 SDK refuses to launch an app without the UIScene life cycle,
  which winit 0.30 does not adopt. `build_app.sh` records SDK 18.0 in the binary with `vtool` so
  UIKit keeps the app-delegate life cycle. Remove it once winit's iOS backend uses scenes.

## What's left

- Safe area: the HUD and screens draw under the Dynamic Island and the home indicator's corner.
- Touch: tuning (look speed, button sizes and places, a configurable layout), item drag-splitting
  in containers, a long press for the right click, the creative inventory's tabs, chat.
- Keyboard and mouse: the edit shortcuts (copy, paste, select all) use Control as on Windows and
  Linux, not Command; the wheel in play doesn't follow the system's natural-scrolling setting;
  a screen opened from play keeps the cursor in the middle until the pointer moves or clicks
  (iPadOS can't move its pointer there); a button other than left or right may not reach screens
  if the system makes no touch for it; the startup menu (before the resources load) types with a
  US layout. Game controllers (`GCController`) aren't read yet.
- Memory and heat on devices: render distance and the worker count still use desktop defaults;
  watch for the system ending the app (jetsam) on phones with less memory.
- The narrator (`Unsupported platform ios`), the clipboard (UIPasteboard), Game Center, an app
  icon and a launch screen, an Xcode project for signing and TestFlight.
- Mods: native code mods are built by the launcher on the desktop; iOS can't load code at run time.
