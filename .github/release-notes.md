## Install

Clipward isn't code-signed yet, so macOS and Windows warn the first time you open it. The app is safe to run. Here's how to get past the warning.

### macOS (Apple silicon)

1. Open `Clipward_<version>_aarch64.dmg` and drag **Clipward** into **Applications**.
2. Open Clipward. macOS says it can't verify the app: click **Done**.
3. Go to **System Settings → Privacy & Security**, scroll down to the message about Clipward, and click **Open Anyway**. Confirm with your password.

If macOS says the app "is damaged and can't be opened", or there's no **Open Anyway** button, run this in Terminal, then open Clipward again:

```bash
xattr -dr com.apple.quarantine /Applications/Clipward.app
```

### Windows (x64)

1. Unzip `Clipward_<version>_windows_x64.zip` into a folder you keep, for example `Documents\Clipward`. Keep the `resources` folder next to `clipward.exe`.
2. Open `clipward.exe`. If **Windows protected your PC** appears, click **More info → Run anyway**.

Work or school PCs can block unsigned apps entirely. Ask your IT team in that case.

### Linux (x64)

```bash
tar xzf Clipward_<version>_linux_x64.tar.gz
./Clipward/clipward
```

Keep the `resources` folder next to `clipward`. Needs a desktop with Vulkan graphics drivers (most current distributions have them).

### Known limits

- Recording a voiceover in the editor works on macOS only for now.
- Clipward updates itself from here on: it checks when it opens, and **Settings → Help & about → Update and restart** installs the new version (no install warning this time). If that fails, download the new version from its release page.
