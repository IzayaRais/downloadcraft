# Download Craft 0.9.6.1: review and proposed product changes

The confirmed bugs are fixed in the supplied source. The following product
ideas have deliberately not been implemented. This review does not establish
that the application is free of bugs or works with every site.

## What the adversarial review changed

| Initial assumption | Counterargument from the code / test | Decision |
| --- | --- | --- |
| A full byte counter means the video is finished. | Audio or duration validation runs afterward and can fail with every byte present. | Require an explicit successful engine outcome before completion. |
| A 20-second lock timeout prevents stale locks. | A healthy 45-minute download also looks stale, letting another writer in. | Use a lock tied to an OS file handle; process exit releases it. |
| Cookies can be shared when the site and session match. | Two jobs then own one path; one job's cleanup removes the other's cookies. | Give every invocation its own private file and cleanup lifetime. |
| Cancelling the Rust task stops its work. | An external process can outlive the future, and its merger can outlive it. | Own the process tree and stop it on cancellation / helper pause. |
| A neighboring filename probably belongs to the downloader. | Prefix matching can rename a user's unrelated file or an intermediate stream. | Accept the exact final path emitted by this helper run. |
| Taking over every download is the goal. | POST exports carry request bodies Download Craft cannot reconstruct. | Leave observed non-GET requests intact in the browser. |
| An optional scanner command is user-controlled, so substitution is safe. | The user controls the command, but a server can influence the filename. | Tokenize first, substitute into argument values, and avoid an implicit shell. |
| A version command is too small to block. | A noisy custom executable fills a pipe before exiting; waiting before draining deadlocks. | Capture without a blocking pipe and preserve the timeout. |

Four original regressions were reproduced before editing: identical cookie
jars, stolen old-but-live locks, blocked large version output, and stale redirect
URLs. Additional fixes were verified through code review and targeted tests,
including a mock helper that launches a separate merger process.

## Proposed improvements — not implemented

### 1. A transfer preview that describes the actual result

Before Download, show the available resolution, audio availability, subtitle
options, approximate size, and whether merging is required. Distinguish known
information from guesses. The value is avoiding another '45 minutes, no audio'
surprise.

Counterargument: probing can delay a download and some sites cannot expose this
information until extraction. Proposed decision: show a quick initial dialog
and enrich it asynchronously; do not invent a duration or promise audio when
unknown. Offer a simple default and expandable technical details.

### 2. Capture rules by site and file type

Examples: capture ZIP/installers automatically, ask for videos, leave PDFs in the
browser; a per-site 'never show the video button here' setting; a temporary pause
for capture. The existing X would remain a quick dismissal for one video.

Counterargument: a rules editor can become harder than the downloads themselves.
Proposed decision: start with three plain choices — automatic, ask, leave in
browser — and add advanced overrides only when needed.

### 3. A 'repair this download' flow

Let the user revisit a page and attach a fresh signed URL to an existing failed
row. Preserve verified partial bytes only when content identity / validators
match; otherwise explain why restarting is required.

Counterargument: the same title or filename does not prove identical content.
Proposed decision: use extractor IDs and HTTP validators when available; require
an explicit choice when identity is uncertain. Never stitch unrelated videos
based on a similar name.

### 4. A guided setup and health screen

One place should test browser connection, selected yt-dlp, FFmpeg, ffprobe and
JavaScript runtime; offer install progress, cancellation, and a tiny sample
video/audio check. Explain a detected problem in terms of the missing outcome,
such as '1080p needs the merger', rather than an executable path alone.

Counterargument: automatically installing everything adds size and changes a
user's machine. Proposed decision: download only selected per-user tools, show
their source and size, and preserve custom/system installations.

### 5. Safe diagnostic export

Provide a Copy diagnostics button containing app/extension versions, platform,
helper versions and recent failure stages. Strip cookies, credentials, signed
URL tokens and personal file paths by default, and let the user inspect it.

Counterargument: even a diagnostic bundle can expose a private video title.
Proposed decision: preview and opt in; avoid background uploads or telemetry.
This would make a maintainer's bug report far more actionable.

### 6. Resource-aware downloads

An optional 'quiet' profile could lower concurrency on battery or metered
connections, limit bandwidth, and delay heavy merging until a suitable time.

Counterargument: silent throttling makes the app seem broken. Proposed decision:
keep it opt-in, show the active reason beside the speed, and offer a one-click
'full speed for this download' override.

### 7. Better duplicate detection

Expiring URLs can make the same episode appear new. Show a possible duplicate
based on extractor identity and the actual file, with Keep both / Use existing /
Replace options.

Counterargument: edits and quality variants can share a title or site ID.
Proposed decision: suggest rather than silently delete or overwrite. Include
resolution, audio language and file size in the comparison.

### 8. A reliable desktop lifecycle and update experience

After native validation, consider platform-appropriate background operation,
signed/notarized installers and a reversible update channel. Closing a window
while work continues should have an explicit, consistent meaning.

Counterargument: adding tray behavior and more package formats before testing
existing platforms increases maintenance risk. Proposed decision: first test
Windows, macOS, Fedora and Ubuntu with real browsers and desktop sessions;
expand distribution mechanisms after those workflows are proven.

## Highest-value next step

Prioritize the setup health screen and transfer preview. They explain and
prevent the failures the user has already encountered. Follow with capture
rules and redacted diagnostics. Defer a broad UI redesign until the cross-OS
integration matrix is exercised.

## What still needs native integration testing

- Windows Chrome/Brave/Firefox: location prompt timing, redirection, POST exports,
  native-host failure, pause during extraction/download/merge and removal.
- macOS Intel and Apple Silicon: helper installation, Dock launch paths,
  browser-native messaging, file dialogs and power actions.
- Fedora/Ubuntu under Wayland and X11, plus AppImage normal and extract/run modes;
  sandboxed browsers need their native-messaging integration checked separately.
- Network loss, disk-full, forced application exit, simultaneous dialogs and
  expired signed URLs against authorized test media.

The model tests validate decision logic, not real browser prompt ordering.
A video may be short, long, a fragment, silent by design, or live; there is no
universal answer from one .mp4 URL. Preserve positive manifest/player/extractor
evidence and expose uncertainty instead of promising every site is supported.
