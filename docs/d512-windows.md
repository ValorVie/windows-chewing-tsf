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

`D512LogCalibration` is a DWORD (0 off by default, 1 on), stored unchanged in
`ChewingTsfConfig::d512_log_calibration`. The TIP collects only when D512 is on
and this value is exactly 1. When disabled, it does not read the engine's
calibration getter or send calibration IPC. The libchewing dependency must
provide both `d512_calibration()` and the single-candidate scoring skip
described in its `docs/d512.md`.

The engine reuses a successful evaluation when Enter commits the same input
and confirmed context with unchanged native candidates and D512 parameters.
This keeps committed text consistent with the displayed preedit and avoids
another scoring call. Failed or timed-out evaluations are not cached.

Each fresh collected preedit evaluation gets a random ID and a `rerank` record.
Repeated reads of that attempt do not create another record. All pending records
are retained until commit or cancel. Before passing a commit key to the editor,
the TIP captures its last available preedit evaluation. Any hidden recomputation
inside the editor's commit is excluded from collection. Accepting the current suggestion marks its
receipt `accepted=true` and `applied=true`; it does not commit text. Commit
receipts are detached before decoding any remaining preedit, then confirmed
after the TSF document write. They contain the actual output text, including
simplified conversion. Canceled, denied or replaced writes produce `cancel`.
Once insertion or the composition-prefix commit succeeds, later display or
selection errors do not change that record to `cancel`.
Focus/context changes cancel pending records and invalidate delayed commits;
turning collection off discards pending and detached receipts.

The host accepts complete records via `im.chewing.d512.CalibrationLog` with
`oneway=true`. A separate background worker performs connection, attestation,
serialization and nonblocking pipe writes. The UI only tries a bounded handoff.
Busy senders and records exceeding the 64 KiB frame limit are dropped without
retry. Calibration transport failure does not delay the scoring worker.

The host checks the current registry DWORD again for each record; only exactly
1 enables writing. A shared mutex serializes append operations across pipe
connections. It creates the directory/file only after that check, writes sorted
UTF-8 JSON with one trailing newline, and emits one content-free error if a
record cannot be saved. The IPC loop keeps running without a reply.

Records go to `calibration.jsonl` in the Chewing user data directory. The host
uses `SHGetKnownFolderPath` for the current user's Profile and RoamingAppData,
so a host launched with a cleared environment still resolves the directory.
It preserves Chewing's priority: `CHEWING_USER_PATH` when present, then an
existing Profile/`ChewingTextService` directory, otherwise RoamingAppData/`Chewing`.
It does not use its installation or working directory for records. The launcher's
cleared environment and `ORT_DISABLE_TELEMETRY=1` remain unchanged.

Fields match the Mac calibration record, including explicit null `modelLogP`
and `fused` values when scores are unavailable. Windows emits zero personal
hits/bonus and false personal/POM conflicts. Mac's actual wire modes are
`automatic` and `suggest`. The analysis script accepts both these modes and
legacy automatic records whose mode is null. Scores from failed or timed-out
attempts remain null, so they cannot be used as scored calibration examples.

`D512SuggestionAcceptKey` is a DWORD: 0 Tab (default), 1 Ctrl+Tab, 2 Right at
composition end. In normal Chinese composition without unfinished Zhuyin,
Tab/Ctrl+Tab in their configured mode consume the gesture even without a valid
suggestion; Right keeps native behavior when acceptance fails. Other modifiers,
candidate windows and keybindings retain their existing behavior. The hint
always uses `⇥ ` followed by the engine-provided changed words joined with `／`.
It does not display or diff the full winning sentence; acceptance still uses
that complete proposal. Only the pure Ctrl prefix of configured
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
