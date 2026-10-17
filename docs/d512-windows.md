<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# D512 scoring on Windows

The TIP keeps its libchewing editor and enables only `d512`. It sends candidate
sentences to `chewing_tip_host`, which enables `d512-ort` and owns one model
session. Build and inspect these packages separately because Cargo unifies
features in a workspace build.

Each scoring call has the engine's deadline (100 ms by default). A bounded
background worker isolates synchronous server attestation. Pipe connection,
write and read operations have deadlines and a 64 KiB frame limit. Errors,
busy state and late replies preserve the engine's complete native order. Host
launches are throttled to one attempt per 30 seconds, with no scoring retry.
Mode 0 creates no IPC scorer and makes no connection or launch attempt. Mode 2
keeps native text and shows a separate suggestion. Acceptance calls the
engine's atomic suggestion API and leaves composition active without committing.
Automatic parameters use `D512Weight` / `D512Margin`; suggestion parameters use
`D512SuggestWeight` / `D512SuggestMargin`. Both pairs are decimal strings, with
missing, invalid, nonfinite or negative values using that mode's defaults.
Defaults use the balanced pairs below. Existing saved values, including former
defaults, remain unchanged when loaded or saved with unrelated settings.

| Mode | Conservative | Balanced (default) | Aggressive |
| --- | --- | --- | --- |
| Automatic (weight / margin) | 0.25 / 1 | 0.25 / 0.5 | 0.25 / 0.2 |
| Suggestion (weight / margin) | 0.25 / 3 | 0.25 / 2 | 0.25 / 1 |

The margins follow the Mac presets by product decision; they are not the
libchewing calibration points. Weight is 0.25 for all six presets.

`D512SuggestionAcceptKey` is a DWORD: 0 Tab (default), 1 Ctrl+Tab, 2 Right at
composition end. In normal Chinese composition without unfinished Zhuyin,
Tab/Ctrl+Tab in their configured mode consume the gesture even without a valid
suggestion; Right keeps native behavior when acceptance fails. Other modifiers,
candidate windows and keybindings retain their existing behavior. The hint
identifies the configured gesture. Only the pure Ctrl prefix of configured
Ctrl+Tab preserves the hint; other keys withdraw it.

Hints reuse Notification with an independent owner and no timeout. UI suppression
by TSF is respected. Acceptance requires a visible current hint, a fresh engine
report, matching TSF context/composition, and a synchronous read of the actual
preedit, confirmed context and collapsed selection. Read errors or mismatches
never apply cached text. No TextEditSink is subscribed: a mouse edit can leave
a hint visible until the next key/focus/composition event, but acceptance checks
its current document state. Deferred preedit callbacks use a shared generation
and weak ownership so old contexts cannot restore a dismissed hint.

Confirmed context is read before the composition range, with at most 64 Unicode
scalar values. A failed TSF read uses empty context. Requests and scores are not
logged. AppContainer and elevated application acceptance still requires native
application testing; a connection or attestation failure uses native conversion.

An unsigned host must be beside the calling TIP DLL in protected Program Files.
The installer therefore puts the x64 and x86 TIP DLLs beside the host, naming the
x86 file `chewing_tip_x86.dll`. COM registry entries retain their respective
32-bit and 64-bit views. Signed-server verification remains supported.

## Prepare the installer

Use the libchewing checkout specified by the integration contract at
`../libchewing`; its `docs/d512.md` defines the scorer API. Supply these immutable
assets in one directory:

- `model.onnx`, `vocab.json`, `manifest.json` and `D512-LICENSE.txt` from the MIT
  [d512-pf512-20260926 checkpoint](https://huggingface.co/Funmula/0c-sequence-d512/tree/d512-pf512-20260926).
  Rename the checkpoint's `LICENSE` to `D512-LICENSE.txt`.
- `onnxruntime.dll`, `onnxruntime_providers_shared.dll`, `ORT-LICENSE.txt` and
  `ORT-ThirdPartyNotices.txt` from the official
  [ORT 1.31.0 CPU win-x64 archive](https://github.com/microsoft/onnxruntime/releases/tag/v1.31.0).
  Rename `LICENSE` and `ThirdPartyNotices.txt` as above. The archive SHA-256 is
  `3958d8a44984160692c1b4ffd0de3cb48f8c39f9d34bb31e433db06e30b82195`.

`stage-d512` checks every file's pinned SHA-256 before copying anything. The
host loads the model and ORT DLL only from its own installation directory, using
an absolute runtime path. It sets `ORT_DISABLE_TELEMETRY=1` before ORT starts.

In a Windows development shell, set the three variables to approved artifacts:

```powershell
$assets = 'C:\work\d512-assets'
$preferences = 'C:\work\preferences\ChewingPreferences.exe'
$preferencesSha256 = '<expected SHA-256 from the preferences build>'
cargo xtask download-components
cargo xtask stage-d512 --from $assets
cargo xtask stage-preferences --from $preferences --sha256 $preferencesSha256
cargo xtask build-installer --target msvc --release --without-arm64
cargo xtask check-installer --without-arm64
cargo xtask package-installer --without-arm64
```

`check-installer` checks every active MSI file source, D512 hashes, co-location
with the host and installed-name collisions. Packaging runs this check first.
Run `stage-preferences` after `download-components`, which includes upstream
preferences. The preferences source and expected hash are arguments so a later
integration can select a new build without editing this repository.

`--without-arm64` is this fork's x64-only build option; it also includes the x86
TIP for 32-bit applications. The upstream ARM64 build and MSI paths remain
available when the flag is omitted. Both build and package commands must use
the same option. The x64-only MSI rejects ARM64 Windows.

WiX 7 must already be installed and its OSMF EULA accepted by the user. These
commands do not accept the EULA automatically. Packaging creates
`dist/windows-chewing-tsf-unsigned.msi`; it does not sign or install it.
