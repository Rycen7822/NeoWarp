# macOS user notifications

NeoWarp uses Apple's `UserNotifications` framework. The Objective-C bridge requests authorization and schedules
notifications through `UNUserNotificationCenter`. See Apple's
[UserNotifications documentation](https://developer.apple.com/documentation/usernotifications?language=objc).

## Build and run a signed app

Notification authorization requires an app bundle with a valid signature. A bare `cargo run` process does not
provide that bundle identity.

From the repository root, use the configured development environment and run:

```bash
./script/run
```

The macOS runner builds `NeoWarp.app`, signs it with an existing Apple Development identity, and falls back to
ad-hoc signing. It then runs the bundled executable. The default output path is
`.work/target/debug/bundle/osx/NeoWarp.app`.

To launch the app through LaunchServices instead, run:

```bash
./script/run --open_with_launchd
```

This mode opens the app with `/usr/bin/open` and tails `~/Library/Logs/neowarp.log`. Pressing Ctrl+C stops the log
command. It does not stop an app launched by LaunchServices.

## Check authorization and delivery

macOS stores notification authorization for the app bundle identity. NeoWarp uses `dev.neowarp.NeoWarp`.
If the current identity already has an authorization decision, the system does not show the first-run prompt again.

1. Open **System Settings → Notifications → NeoWarp** to inspect the current permissions.
2. Trigger the notification path in the app.
3. Check Notification Center for the notification.
4. Inspect `~/Library/Logs/neowarp.log` for application errors.
5. Use Console to inspect messages from NeoWarp, NotificationCenter, and usernoted when the app log does not explain
   the failure.

Do not delete Notification Center databases or kill system services to reset authorization. Use a separate test
account when a clean first-run authorization state is required.
