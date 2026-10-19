<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# D512 scoring on Windows

The TIP keeps its libchewing editor and enables only `d512`. It sends candidate
sentences to `chewing_tip_host`, which enables `d512-ort` and owns one model
session. Build and inspect these packages separately because Cargo unifies
features in a workspace build.

Each scoring call has the engine's deadline (100 ms by default). A bounded
background worker isolates synchronous server attestation. Pipe connection,
write and read operations have deadlines and a 64 KiB frame limit. Errors,
busy state and late replies preserve the engine's current default, including
enabled personal preferences. Host
launches are throttled to one attempt per 30 seconds, with no scoring retry.
Mode 0 creates no IPC scorer and makes no connection or launch attempt. Mode 2
keeps the engine default and shows a separate suggestion. Acceptance calls the
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

`UsePersonalCandidatePreference` is a DWORD, enabled by default. It controls
`EditorOptions::personal_candidate_preference`. `D512PersonalBonus` and
`D512SuggestPersonalBonus` are decimal strings for the automatic and suggestion
bonuses. Both default to 2; missing, invalid, nonfinite or negative values use
that default. Presets do not change these bonuses. Turning the switch off keeps
saved preferences and makes the engine's effective personal hits and bonus zero.

Manual selection and suggestion acceptance do not learn until the exact commit
text reaches the client. The TIP detaches the engine's owned receipt before
`ack()` and keeps it on the editor thread through the edit session. Successful
insertion confirms it; failed, missing or replaced writes reject it. Simplified
conversion or added output text rejects learning when it changes the original
commit. A composition prefix also needs its full UTF-16 range shift to succeed.
Turning the preference off revokes pending learning; callbacks after editor
teardown discard their receipts without keeping the store alive. See the
engine's [personal preference contract](https://github.com/ValorVie/libchewing/blob/b51924d80252013f9fc64f12804022c4098efdf8/docs/personal-candidate-preference.md)
for storage, candidate ordering and lifecycle flush limits.

Settings reload and thread focus still construct a replacement editor before
dropping the old one. Editors with the same user path share a preference store,
so these rebuilds keep its worker alive. The TIP does not call `Editor::flush()`
from keys, edit-session callbacks or focus handlers. Final store teardown can
still wait for persistence during deactivation or process exit.

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
serialization and pipe writes. This worker waits for pipe buffer space so a
commit/cancel batch can outlive brief reader delays. The UI only tries a bounded handoff.
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
and `fused` values when scores are unavailable. Windows copies each candidate's
`personal_hits` and the active `bonus` from the engine. The engine supplies zeros
when its personal preference switch is off. Personal/POM conflict flags remain
false. Mac's actual wire modes are
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

Suggestion bubbles follow Mac TooltipUI's normal appearance. The app theme
selects a charcoal RGB (0.18, 0.18, 0.18) background with white text in dark mode;
light mode swaps those colors. Unknown or missing app theme uses light mode.
The regular Windows message font is read at 96 DPI: text size is at least
12 DIPs or one DIP above that font's size, and padding is half the base size,
rounded up. The bubble has a 7-DIP radius, a soft shadow, and a 4-DIP gap below
the composition caret. The shadow margin is excluded from caret alignment.
Existing monitor clamping applies. General notification colors, font and
geometry remain separate.

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

Cargo fetches [libchewing](https://github.com/ValorVie/libchewing/tree/valor-feature)
from the `valor-feature` branch. `Cargo.lock` pins the exact engine revision;
use `cargo build --locked` to keep that revision. The engine's `docs/d512.md`
defines the scorer API. To edit both repositories locally, comment out the git
entry under `[patch."crates-io"]` in `Cargo.toml` and uncomment the adjacent
`chewing = { path = "../libchewing/" }` entry. Restore the git entry and lockfile
before committing. Supply these immutable assets in one directory:

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

## Tests with an installed host

Core unit tests use a process-specific pipe. To run the full workspace while
an installed host is active, set `CHEWING_TEST_PIPE_ID` to a unique ASCII
alphanumeric or hyphenated ID (1 to 64 bytes), then use:

```powershell
cargo test --locked --workspace --features tsfreg/test-ipc --target x86_64-pc-windows-msvc
```

The explicit `test-ipc` feature lets the Stop integration listener and its CLI
child share that private pipe. A missing or invalid ID is an error. This feature
cannot compile in Release; normal product builds keep the installed host's
original pipe path. The tests do not stop the installed host.
