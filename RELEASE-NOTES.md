# Download Craft 0.9.6.1 — bug-fix development build

This release keeps the requested public version 0.9.6.1 and includes all
the previously validated deep-review fixes. It fixes verified defects;
product and feature proposals in REVIEW-0.9.6.1.md have not been implemented.

## Changes

- Require a successful engine result before the main window marks a download
  Completed. Audio, duration, merge and checksum failures cannot be overridden
  by full byte counters. Keep the resolved filename available to final checks.
- Use OS-held file locks rather than expiring a live owner's lock after 20
  seconds. Stop shared-file writes if lock acquisition fails.
- Assign and persist new GUI queue IDs together. CLI commands release their
  queue lock before running a download and re-lock before updating its result;
  unreadable queue JSON produces an error rather than being overwritten.
- Create a separate private cookie jar for each helper invocation, with Unix
  permissions 0600 and automatic deletion on success, failure or cancellation.
  Protect saved queue/inbox/preferences with private state files and a Unix
  configuration directory accessible only to its owner. The native host passes
  captured cookies through that inbox rather than child-process arguments.
- Stop external helpers when their async task is aborted. Supervise yt-dlp's
  descendant merger processes with Windows jobs / Unix process groups. Pause
  stops yt-dlp; Resume restarts it using whatever partial files remain.
- Use the final redirect URL received during browser capture. Retain observed
  non-GET downloads in the browser because their request body cannot be replayed
  by Download Craft. This guard is best effort: it depends on webRequest events.
- Use yt-dlp's reported final path rather than scanning similarly named files.
  Escape percent characters in literal output paths.
- Avoid pipe deadlock during helper version checks; retain the five-second
  timeout and bound the captured version prefix in memory.
- Discover the previous FFmpeg/ffprobe pair after an interrupted installation,
  and recover it before attempting a subsequent replacement.
- Match HTTP client platform hints to the configured user agent. Remove a
  Windows-only FFmpeg instruction and the duplicate Unix Open with action.
- Parse the optional after-download executable and arguments before inserting
  the filename; no implicit shell interprets downloaded filenames. Templates
  using shell operators must be moved into an explicitly chosen script.
- Bump both desktop and extension to 0.9.6.1. Packaging reads RELEASE-NOTES.md;
  the Linux CI smoke command no longer hardcodes the previous version.

## Updating Windows

Close all Download Craft windows and exit its tray process. Extract the whole
Windows ZIP to a permanent folder and run download_craft.exe. Reload the bundled
extension in Chrome/Brave/Firefox and refresh open video tabs. Both the native
app and the extension need updating for all fixes. Existing preferences and
queue data stay in their usual per-user locations.

Keep the packaged extension folders alongside the executable when registering
browser integration. The included Firefox XPI is a development build; it is not
Mozilla-signed. No release has been published upstream by this task.

## Packages

The app, UI, updater checks, command line, both browser extensions and download
filenames use the requested public version **0.9.6.1**. The fixes are retained.
Cargo internally stores the SemVer equivalent `0.9.6+1`; `VERSION` is the public
version source. Apple's numeric bundle-build field has three components, while
its display version and the app itself retain 0.9.6.1.

- Windows x64: extract the entire ZIP into a permanent folder and run the EXE.
- Mac: the universal ZIP contains `Download Craft.app` for Apple Silicon and
  Intel. Move the app to Applications and launch it before browser setup.
- Fedora: install the RPM with your software installer or `sudo dnf install ./downloadcraft-0.9.6.1-1.x86_64.rpm`.
- Debian, Ubuntu, Mint, Pop!_OS: install the DEB through your software installer
  or `sudo apt install ./downloadcraft_0.9.6.1_amd64.deb`.
- Other compatible Linux x64 desktops: make the AppImage executable. If FUSE
  is unavailable, launch it with `--appimage-extract-and-run`.

Linux artifacts target glibc 2.28. Mac targets macOS 12+. Windows and Mac builds
are cross-compiled here. The Mac bundle is ad-hoc signed, not Apple-notarized;
macOS may require approval in Privacy & Security.

If a numerically newer package is already installed, the requested version is
an explicit downgrade: use your package manager's downgrade/reinstall operation
or replace the portable ZIP/AppImage. This release still contains the latest fixes.

## Browser and helper setup

Reload the bundled `browser-extension` folder in Chrome/Brave. In Firefox use
`about:debugging` and the `browser-extension-firefox` folder or included XPI.
Both extension variants report 0.9.6.1. Refresh existing video tabs after updating.
The Firefox development add-on is unsigned; permanent distribution needs signing.

In Settings → Plugins, Mac/Linux Download tools installs the native FFmpeg and
ffprobe pair into per-user storage without administrator access. yt-dlp download,
system/custom tools and supported JavaScript runtime detection remain available.
Windows retains its existing FFmpeg workflow. Minimize the Mac/Linux main window
while work runs; closing it exits the application.

Snap/Flatpak browsers may need their distribution's native-messaging integration.
Normal native browser host manifests are installed into per-user locations.

## Validation

See VALIDATION.txt in the source bundle for the exact checks on these artifacts.
Feature proposals in REVIEW-0.9.6.1.md remain unimplemented. No upstream publication
has been performed.
