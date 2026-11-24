# Dock tile plugin

The macOS dock tile plugin updates NeoWarp's Dock icon when the app icon changes. It preserves the selected icon
across app restarts.

The Objective-C plugin uses `NSDockTilePlugIn`. It observes icon change notifications from the app and updates the
current Dock tile. See Apple's [NSDockTilePlugIn documentation](https://developer.apple.com/documentation/appkit/nsdocktileplugin?language=objc).

## Build and bundle

The Makefile compiles the plugin with `clang` and the `-bundle` flag. It creates a universal binary for arm64 and
x86_64. The app's macOS build script runs `make` and sets `OUTPUT_DIR` and `MACOSX_DEPLOYMENT_TARGET` for the active
build.

`script/macos/bundle` copies the built plugin to
`NeoWarp.app/Contents/PlugIns/WarpDockTilePlugin.docktileplugin`. It also adds the plugin name and app bundle
identifier to the bundle property lists. The `WarpDockTilePlugin` name is an internal identifier retained by the
NeoWarp fork.

## Debug plugin changes

macOS can continue using an older loaded plugin after a rebuild. If the Dock does not pick up the new plugin, remove
NeoWarp's icon from the Dock and restart the Dock process:

```bash
killall Dock
```

Add NeoWarp to the Dock again after rebuilding and launching the new bundle.

The plugin writes diagnostic logs to `/tmp/neowarp_docktile_<timestamp>.log`. Use these logs when `NSLog` output is
not visible in Console. The [MacDockTileSample](https://github.com/CartBlanche/MacDockTileSample) project provides
another example of a Dock tile plugin.
