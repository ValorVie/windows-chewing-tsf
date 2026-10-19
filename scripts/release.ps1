# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
Build an unsigned x64 D512 MSI locally; publish only with -Publish.
.DESCRIPTION
Requires 64-bit Windows PowerShell 5.1 or pwsh 7, Git, Rust MSVC with x64/i686
 targets, VS 2022 C++ Build Tools/SDK, Node/npm, and WiX 7. The user must accept
 the WiX EULA beforehand; this script never accepts it or installs tools.

Flow: validate clean HEAD/tools; create a detached temporary worktree; clone the
 exact preferences commit; verify/download pinned model/ORT assets; build
 preferences; update the MSI version; run upstream download/stage/build/check/
 package tasks. Version files change only in the temporary worktree.

Without -Publish this is a preview and does not need gh or authentication.
-Publish also requires gh auth login, HEAD = origin/valor-feature and -ChangesFile
 (a Markdown list of user-visible changes, shown above the build provenance). It pushes
 only the annotated version tag and creates a prerelease with MSI/checksum.
 Existing tags/assets at another commit/hash stop; no force, deletion or clobber.

Example: .\scripts\release.ps1 -Version 26.9.0-valor.1 -PreferencesRef <full SHA>
 Add -Publish only after inspecting the preview and passing user acceptance.
-SelfTest tests pure version/tag/release decisions without tools or network.

Success: MSI ProductVersion and SHA-256 verified; MSI, .msi.sha256, release notes
 and build receipt remain under CacheDir/releases. Matching inputs reuse that
 verified artifact (MSI package identities/timestamps need not be reproducible).
 All asset hashes come from xtask/src/payload.rs. Default CacheDir is under
 LOCALAPPDATA; an in-repo cache must be gitignored. Caches can be large.

Stop/recovery: any tool/download/hash/tag/Release conflict exits nonzero without
 retry. Logs redact tokens. Failure preserves its log and temporary path; inspect
 them before retrying. Successful runs alone remove their own temporary worktree
 and clone. Never delete a tag/Release or force-push to recover a partial publish.
 Do not install/register the MSI as part of this script.
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$PreferencesRef,
    [string]$CacheDir,
    [string]$ChangesFile,
    [switch]$Publish,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

function Convert-ReleaseVersion([string]$Value) {
    $m = [regex]::Match($Value, '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-valor\.(0|[1-9][0-9]*)$')
    if (-not $m.Success) { throw 'Invalid version: use major.minor.patch-valor.n' }
    $parts = @()
    $limits = @(255, 255, 65535, 65535)
    for ($i = 0; $i -lt 4; $i++) {
        $n = 0
        if (-not [int]::TryParse($m.Groups[$i + 1].Value, [ref]$n) -or $n -gt $limits[$i]) {
            throw 'Invalid version: component exceeds MSI/resource limits'
        }
        $parts += $n
    }
    return [pscustomobject]@{ Major = $parts[0]; Minor = $parts[1]; Patch = $parts[2]; Build = $parts[3]; Msi = $parts -join '.' }
}
function Get-TagPlan([string]$Head, [string]$Local, [string]$Remote) {
    if (($Local -and $Local -cne $Head) -or ($Remote -and $Remote -cne $Head)) { throw 'Tag conflict: tag points elsewhere' }
    if ($Remote) { return 'Reuse' }
    if ($Local) { return 'PushExisting' }
    return 'CreateAndPush'
}
function Get-ReleasePlan([bool]$Exists, [bool]$Prerelease, [bool]$Draft, [string]$LocalMsi, [string]$RemoteMsi, [string]$LocalSum, [string]$RemoteSum) {
    if (-not $Exists) { return 'Create' }
    # Matching assets are a no-op even if the existing release flags changed.
    if (-not $RemoteMsi -or -not $RemoteSum -or
        $LocalMsi -cne $RemoteMsi -or $LocalSum -cne $RemoteSum) { throw 'Release conflict: existing release/assets differ; nothing will be overwritten' }
    return 'Reuse'
}

function Assert-Equal($Actual, $Expected) {
    if ($Actual -cne $Expected) { throw "Expected '$Expected', got '$Actual'" }
}
function Assert-Rejected([scriptblock]$Action, [string]$Message) {
    try { & $Action } catch {
        if ($_.Exception.Message.StartsWith($Message)) { return }
        throw
    }
    throw 'Expected rejection'
}
function Join-ReleaseNotes([string]$Notes, [string]$Changes) {
    if (-not $Changes -or -not $Changes.Trim()) { throw 'Release changes are empty' }
    $title, $rest = $Notes -split "`n", 2
    return ($title + "`n`n## 本版變更`n`n" + $Changes.Trim() + "`n`n## 建置來源`n" + $rest)
}
function Invoke-SelfTests {
    $a = 'a' * 40
    $b = 'b' * 40
    $cases = @(
        @{ Name = 'version mapping'; Run = { Assert-Equal (Convert-ReleaseVersion '26.9.0-valor.1').Msi '26.9.0.1' } },
        @{ Name = 'zero build'; Run = { Assert-Equal (Convert-ReleaseVersion '0.0.0-valor.0').Build 0 } },
        @{ Name = 'version suffix'; Run = { Assert-Rejected { Convert-ReleaseVersion '26.9.0' } 'Invalid version' } },
        @{ Name = 'version injection'; Run = { Assert-Rejected { Convert-ReleaseVersion '26.9.0-valor.1;echo bad' } 'Invalid version' } },
        @{ Name = 'version leading zero'; Run = { Assert-Rejected { Convert-ReleaseVersion '026.9.0-valor.1' } 'Invalid version' } },
        @{ Name = 'version MSI bound'; Run = { Assert-Rejected { Convert-ReleaseVersion '256.9.0-valor.1' } 'Invalid version' } },
        @{ Name = 'version resource bound'; Run = { Assert-Rejected { Convert-ReleaseVersion '26.9.0-valor.65536' } 'Invalid version' } },
        @{ Name = 'version case'; Run = { Assert-Rejected { Convert-ReleaseVersion '26.9.0-VALOR.1' } 'Invalid version' } },
        @{ Name = 'new tag'; Run = { Assert-Equal (Get-TagPlan $a '' '') 'CreateAndPush' } },
        @{ Name = 'local tag'; Run = { Assert-Equal (Get-TagPlan $a $a '') 'PushExisting' } },
        @{ Name = 'remote tag'; Run = { Assert-Equal (Get-TagPlan $a '' $a) 'Reuse' } },
        @{ Name = 'both tags'; Run = { Assert-Equal (Get-TagPlan $a $a $a) 'Reuse' } },
        @{ Name = 'local conflict'; Run = { Assert-Rejected { Get-TagPlan $a $b $a } 'Tag conflict' } },
        @{ Name = 'remote conflict'; Run = { Assert-Rejected { Get-TagPlan $a $a $b } 'Tag conflict' } },
        @{ Name = 'new release'; Run = { Assert-Equal (Get-ReleasePlan $false $false $false 'm' '' 's' '') 'Create' } },
        @{ Name = 'same release'; Run = { Assert-Equal (Get-ReleasePlan $true $true $false 'm' 'm' 's' 's') 'Reuse' } },
        @{ Name = 'MSI conflict'; Run = { Assert-Rejected { Get-ReleasePlan $true $true $false 'm' 'x' 's' 's' } 'Release conflict' } },
        @{ Name = 'checksum conflict'; Run = { Assert-Rejected { Get-ReleasePlan $true $true $false 'm' 'm' 's' 'x' } 'Release conflict' } },
        @{ Name = 'missing asset'; Run = { Assert-Rejected { Get-ReleasePlan $true $true $false 'm' '' 's' '' } 'Release conflict' } },
        @{ Name = 'same stable release'; Run = { Assert-Equal (Get-ReleasePlan $true $false $false 'm' 'm' 's' 's') 'Reuse' } },
        @{ Name = 'same draft release'; Run = { Assert-Equal (Get-ReleasePlan $true $true $true 'm' 'm' 's' 's') 'Reuse' } },
        @{ Name = 'notes changes'; Run = { Assert-Equal (Join-ReleaseNotes "# T`n`nbody" "- a`n") "# T`n`n## 本版變更`n`n- a`n`n## 建置來源`n`nbody" } },
        @{ Name = 'notes empty changes'; Run = { Assert-Rejected { Join-ReleaseNotes "# T" " `n" } 'Release changes are empty' } }
    )
    $passed = 0
    $failed = 0
    foreach ($case in $cases) {
        try { & $case.Run; $passed++; Write-Host "PASS $($case.Name)" }
        catch { $failed++; Write-Host "FAIL $($case.Name): $($_.Exception.Message)" }
    }
    Write-Host "SelfTest: $passed passed / $failed failed"
    if ($failed) { throw 'SelfTest failed' }
}

if ($SelfTest) {
    try { Invoke-SelfTests; exit 0 } catch { Write-Error $_; exit 1 }
}
function Write-Log([string]$Text) {
    $clean = Protect-Log $Text
    Write-Host $clean
    if ($script:LogFile) { [IO.File]::AppendAllText($script:LogFile, $clean + [Environment]::NewLine, $script:Utf8) }
}
function Protect-Log([string]$Text) {
    foreach ($secret in $script:Secrets) { if ($secret) { $Text = $Text.Replace($secret, '[REDACTED]') } }
    $Text = [regex]::Replace($Text, '(?i)(github_pat_|gh[pousr]_|npm_)[A-Za-z0-9_]+', '[REDACTED]')
    $Text = [regex]::Replace($Text, '(?i)(authorization\s*[:=]\s*(?:bearer|basic)\s+)\S+', '$1[REDACTED]')
    $Text = [regex]::Replace($Text, '(https?://)[^/\s@]+@', '$1[REDACTED]@')
    return [regex]::Replace($Text, '(?i)((?:access_token|token|x-amz-signature)=)[^&\s]+', '$1[REDACTED]')
}
function New-Directory([string]$Path) { [void][IO.Directory]::CreateDirectory($Path) }
function Write-Utf8([string]$Path, [string]$Text) { [IO.File]::WriteAllText($Path, $Text, $script:Utf8) }
function Get-Hash([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash }
function Get-TextHash([string]$Text) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($script:Utf8.GetBytes($Text)))).Replace('-', '') }
    finally { $sha.Dispose() }
}
function Invoke-Tool([string]$Tool, [string[]]$Arguments, [string]$At = $script:RepoRoot, [switch]$Quiet, [switch]$AllowFailure) {
    $executable = @(Get-Command $Tool -CommandType Application -ErrorAction Stop)[0].Source
    Write-Log ('Run: ' + $Tool + ' ' + ($Arguments -join ' '))
    $lines = New-Object 'Collections.Generic.List[string]'
    $oldPreference = $ErrorActionPreference
    Push-Location -LiteralPath $At
    try {
        # Windows PowerShell 5.1 wraps native stderr in ErrorRecord objects.
        $ErrorActionPreference = 'Continue'
        & $executable @Arguments 2>&1 | ForEach-Object {
            $line = Protect-Log $_.ToString()
            $lines.Add($line)
            if (-not $Quiet) { Write-Log $line }
        }
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $oldPreference; Pop-Location }
    if ($code -ne 0 -and -not $AllowFailure) { throw "Command failed: $Tool (exit $code); no automatic retry" }
    return [pscustomobject]@{ Code = $code; Lines = $lines.ToArray(); Text = ($lines.ToArray() -join "`n") }
}
function Get-Git([string[]]$Arguments, [string]$At = $script:RepoRoot) {
    return (Invoke-Tool 'git.exe' $Arguments $At -Quiet).Text.Trim()
}
function Get-Pins([string]$Root) {
    $source = [IO.File]::ReadAllText((Join-Path $Root 'xtask/src/payload.rs'))
    $block = [regex]::Match($source, '(?s)const\s+D512_ASSETS:[^=]*=\s*&\[(.*?)\];')
    $pins = @{}
    foreach ($m in [regex]::Matches($block.Groups[1].Value, '"([^"]+)"\s*,\s*"([a-f0-9]{64})"')) { $pins[$m.Groups[1].Value] = $m.Groups[2].Value.ToUpperInvariant() }
    $names = @('model.onnx','vocab.json','manifest.json','D512-LICENSE.txt','onnxruntime.dll','onnxruntime_providers_shared.dll','ORT-LICENSE.txt','ORT-ThirdPartyNotices.txt')
    if ($pins.Count -ne 8 -or @($names | Where-Object { -not $pins.ContainsKey($_) }).Count) { throw 'Cannot read the stage-d512 pinned asset table' }
    $archive = [regex]::Match($source, '(?m)^//\s+([a-f0-9]{64})\.')
    if (-not $archive.Success) { throw 'Cannot read the pinned ORT archive hash' }
    return [pscustomobject]@{ Files = $pins; Archive = $archive.Groups[1].Value.ToUpperInvariant(); SourceHash = (Get-Hash (Join-Path $Root 'xtask/src/payload.rs')) }
}
function Get-CachedFile([string]$Path, [string]$Expected, [string]$Url) {
    if ((Test-Path -LiteralPath $Path) -and (Get-Hash $Path) -ceq $Expected) { Write-Log ('Asset cache hit: ' + [IO.Path]::GetFileName($Path)); return }
    if ((Test-Path -LiteralPath $Path) -and ((Get-Item -LiteralPath $Path).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to replace a cache reparse point' }
    $partial = $Path + '.partial-' + [Guid]::NewGuid().ToString('N')
    Write-Log ('Download: ' + [IO.Path]::GetFileName($Path))
    try { Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $partial -TimeoutSec 180 -ErrorAction Stop | Out-Null }
    catch { throw ('Download failed, no retry: ' + (Protect-Log $_.Exception.Message)) }
    if ((Get-Hash $partial) -cne $Expected) { throw ('Downloaded asset SHA-256 mismatch: ' + [IO.Path]::GetFileName($Path)) }
    Move-Item -LiteralPath $partial -Destination $Path -Force
}
function Get-Assets($Pins, [string]$Cache, [string]$Temporary) {
    $assets = Join-Path $Cache 'assets/d512-pf512-20260926-ort-1.31.0'
    New-Directory $assets
    foreach ($name in @('model.onnx','vocab.json','manifest.json','D512-LICENSE.txt')) {
        $remote = $name
        if ($name -eq 'D512-LICENSE.txt') { $remote = 'LICENSE' }
        Get-CachedFile (Join-Path $assets $name) $Pins.Files[$name] ('https://huggingface.co/Funmula/0c-sequence-d512/resolve/d512-pf512-20260926/' + $remote)
    }
    $ortNames = @('onnxruntime.dll','onnxruntime_providers_shared.dll','ORT-LICENSE.txt','ORT-ThirdPartyNotices.txt')
    $needOrt = $false
    foreach ($name in $ortNames) {
        $path = Join-Path $assets $name
        if (-not (Test-Path -LiteralPath $path) -or (Get-Hash $path) -cne $Pins.Files[$name]) { $needOrt = $true }
    }
    if ($needOrt) {
        $zip = Join-Path $Cache 'assets/onnxruntime-win-x64-1.31.0.zip'
        Get-CachedFile $zip $Pins.Archive 'https://github.com/microsoft/onnxruntime/releases/download/v1.31.0/onnxruntime-win-x64-1.31.0.zip'
        $unpacked = Join-Path $Temporary 'ort'
        Microsoft.PowerShell.Archive\Expand-Archive -LiteralPath $zip -DestinationPath $unpacked
        $ortRoot = Join-Path $unpacked 'onnxruntime-win-x64-1.31.0'
        foreach ($pair in @(@('lib/onnxruntime.dll','onnxruntime.dll'),@('lib/onnxruntime_providers_shared.dll','onnxruntime_providers_shared.dll'),@('LICENSE','ORT-LICENSE.txt'),@('ThirdPartyNotices.txt','ORT-ThirdPartyNotices.txt'))) {
            $source = Join-Path $ortRoot $pair[0]
            if ((Get-Hash $source) -cne $Pins.Files[$pair[1]]) { throw ('ORT member SHA-256 mismatch: ' + $pair[1]) }
            Copy-Item -LiteralPath $source -Destination (Join-Path $assets $pair[1]) -Force
        }
    }
    foreach ($name in $Pins.Files.Keys) {
        if ((Get-Hash (Join-Path $assets $name)) -cne $Pins.Files[$name]) { throw ('Asset verification failed: ' + $name) }
        Write-Log ('Asset verified: ' + $name)
    }
    return $assets
}
function Get-LibchewingRevision([string]$Lockfile) {
    $lock = [IO.File]::ReadAllText($Lockfile)
    $m = [regex]::Match($lock, '(?ms)^name = "chewing"\r?\nversion = "0\.14\.[^"]+"\r?\nsource = "git\+https://github\.com/ValorVie/libchewing\.git\?branch=valor-feature#([a-f0-9]{40})"')
    if (-not $m.Success) { throw 'Cargo.lock must pin the public D512 libchewing revision' }
    return $m.Groups[1].Value
}
function Get-MsiVersion([string]$Path) {
    if (-not ('ChewingReleaseMsi' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class ChewingReleaseMsi {
 [DllImport("msi.dll",CharSet=CharSet.Unicode)] static extern uint MsiOpenDatabaseW(string p,IntPtr m,out uint d);
 [DllImport("msi.dll",CharSet=CharSet.Unicode)] static extern uint MsiDatabaseOpenViewW(uint d,string q,out uint v);
 [DllImport("msi.dll")] static extern uint MsiViewExecute(uint v,uint r);
 [DllImport("msi.dll")] static extern uint MsiViewFetch(uint v,out uint r);
 [DllImport("msi.dll",CharSet=CharSet.Unicode)] static extern uint MsiRecordGetStringW(uint r,uint f,StringBuilder b,ref uint n);
 [DllImport("msi.dll")] static extern uint MsiCloseHandle(uint h);
 static void Check(uint e){if(e!=0)throw new System.ComponentModel.Win32Exception((int)e);}
 public static string Version(string path){uint d=0,v=0,r=0;try {
  Check(MsiOpenDatabaseW(path,IntPtr.Zero,out d));
  Check(MsiDatabaseOpenViewW(d,"SELECT `Value` FROM `Property` WHERE `Property` = 'ProductVersion'",out v));
  Check(MsiViewExecute(v,0));Check(MsiViewFetch(v,out r));uint n=128;var b=new StringBuilder(128);Check(MsiRecordGetStringW(r,1,b,ref n));return b.ToString();
 }finally{if(r!=0)MsiCloseHandle(r);if(v!=0)MsiCloseHandle(v);if(d!=0)MsiCloseHandle(d);}}
}
'@
    }
    return [ChewingReleaseMsi]::Version($Path)
}
function Enter-BuildShell {
    if ((Get-Command cl.exe -ErrorAction SilentlyContinue) -and $env:VSCMD_ARG_TGT_ARCH -eq 'x64') { return }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Use a Visual Studio 2022 Developer PowerShell, or install VS Build Tools before running this script' }
    $vs = (Invoke-Tool $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') -Quiet).Text.Trim()
    if (-not $vs) { throw 'MSVC x64/x86 Build Tools not found' }
    Import-Module (Join-Path $vs 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
}
function Invoke-Xtask([string[]]$Arguments) {
    [void](Invoke-Tool 'cargo.exe' (@('run','--locked','--package','xtask','--target','x86_64-pc-windows-msvc','--') + $Arguments) $script:SourceWork)
    if ((Get-Hash (Join-Path $script:SourceWork 'Cargo.lock')) -cne $script:LockHash) { throw 'TSF Cargo.lock changed during build' }
}
function Remove-OwnedTree([string]$Path) {
    # Called only for this invocation's newly created random temporary root.
    foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($Path)) {
        $attributes = [IO.File]::GetAttributes($entry)
        if ($attributes -band [IO.FileAttributes]::Directory) {
            if ($attributes -band [IO.FileAttributes]::ReparsePoint) { [IO.Directory]::Delete($entry) }
            else { Remove-OwnedTree $entry }
        } else { [IO.File]::SetAttributes($entry, [IO.FileAttributes]::Normal); [IO.File]::Delete($entry) }
    }
    [IO.Directory]::Delete($Path)
}
function Read-Release([string]$Tag) {
    $response = Invoke-Tool 'gh.exe' @('api','--include',('repos/' + $script:ReleaseRepo + '/releases/tags/' + $Tag)) -Quiet -AllowFailure
    $codes = [regex]::Matches($response.Text, '(?m)^HTTP/\S+\s+(\d{3})')
    $status = 0
    if ($codes.Count) { $status = [int]$codes[$codes.Count - 1].Groups[1].Value }
    if ($response.Code -ne 0) {
        if ($status -eq 404) { return $null }
        throw 'GitHub release lookup failed; only an explicit HTTP 404 means absent'
    }
    if ($status -ne 200) { throw 'Unexpected GitHub release lookup status' }
    $start = $response.Text.IndexOf('{')
    if ($start -lt 0) { throw 'Invalid GitHub release response' }
    return $response.Text.Substring($start) | ConvertFrom-Json
}
function Publish-Result($Result, [string]$Head, [string]$Tag, [string]$Temporary) {
    [void](Invoke-Tool 'gh.exe' @('auth','status','--hostname','github.com') -Quiet)
    $origin = Get-Git @('remote','get-url','origin')
    if ($origin -notmatch '^(https://github\.com/ValorVie/windows-chewing-tsf(?:\.git)?|git@github\.com:ValorVie/windows-chewing-tsf(?:\.git)?|ssh://git@github\.com/ValorVie/windows-chewing-tsf(?:\.git)?)$') { throw 'Publish requires origin to be the ValorVie TSF fork' }
    [void](Invoke-Tool 'git.exe' @('fetch','--no-tags','origin','valor-feature'))
    if ((Get-Git @('rev-parse','FETCH_HEAD')) -cne $Head -or (Get-Git @('rev-parse','origin/valor-feature')) -cne $Head) { throw 'Publish requires HEAD = origin/valor-feature (already pushed)' }
    $local = ''
    $found = Invoke-Tool 'git.exe' @('show-ref','--verify','--quiet',('refs/tags/' + $Tag)) -Quiet -AllowFailure
    if ($found.Code -eq 0) { $local = Get-Git @('rev-parse',($Tag + '^{commit}')) }
    elseif ($found.Code -ne 1) { throw 'Cannot inspect local tag' }
    $remoteLines = (Get-Git @('ls-remote','--tags','origin',('refs/tags/' + $Tag),('refs/tags/' + $Tag + '^{}'))).Split("`n")
    $remote = ''
    foreach ($line in $remoteLines) { if ($line -match '^([a-f0-9]{40})\s+refs/tags/.+\^\{\}$') { $remote = $matches[1] } }
    if (-not $remote) { foreach ($line in $remoteLines) { if ($line -match '^([a-f0-9]{40})\s+') { $remote = $matches[1] } } }
    $tagPlan = Get-TagPlan $Head $local $remote
    $release = Read-Release $Tag
    $remoteMsi = ''; $remoteSum = ''
    if ($null -ne $release) {
        $names = @($release.assets | ForEach-Object { $_.name })
        if ($names -contains $Result.MsiName -and $names -contains ($Result.MsiName + '.sha256')) {
            $downloads = Join-Path $Temporary 'remote-assets'; New-Directory $downloads
            [void](Invoke-Tool 'gh.exe' @('release','download',$Tag,'--repo',$script:ReleaseRepo,'--pattern',$Result.MsiName,'--pattern',($Result.MsiName + '.sha256'),'--dir',$downloads))
            $remoteMsi = Get-Hash (Join-Path $downloads $Result.MsiName)
            $remoteSum = Get-Hash (Join-Path $downloads ($Result.MsiName + '.sha256'))
        }
        $releasePlan = Get-ReleasePlan $true $release.prerelease $release.draft $Result.MsiHash $remoteMsi $Result.SumHash $remoteSum
    } else { $releasePlan = Get-ReleasePlan $false $false $false $Result.MsiHash '' $Result.SumHash '' }
    # All conflicts have been checked before the first remote mutation.
    $freshBranch = Get-Git @('ls-remote','origin','refs/heads/valor-feature')
    if ($freshBranch -notmatch ('^' + $Head + '\s')) { throw 'origin/valor-feature changed before publishing' }
    if ($tagPlan -eq 'CreateAndPush') {
        [void](Invoke-Tool 'git.exe' @('var','GIT_COMMITTER_IDENT') -Quiet)
        [void](Invoke-Tool 'git.exe' @('tag','-a',$Tag,$Head,'-m',('Preview ' + $Version)))
    }
    if ($tagPlan -ne 'Reuse') { [void](Invoke-Tool 'git.exe' @('push','origin',('refs/tags/' + $Tag))) }
    if ($releasePlan -eq 'Create') {
        $notesPath = Join-Path $Temporary 'release-notes-publish.md'
        Write-Utf8 $notesPath (Join-ReleaseNotes ([IO.File]::ReadAllText($Result.NotesPath)) $script:Changes)
        [void](Invoke-Tool 'gh.exe' @('release','create',$Tag,$Result.MsiPath,$Result.SumPath,'--repo',$script:ReleaseRepo,'--prerelease','--verify-tag','--title',('預覽版 ' + $Version),'--notes-file',$notesPath))
    } else { Write-Log 'Release assets match: no upload or overwrite' }
}

function Invoke-Release {
    if ($env:OS -ne 'Windows_NT' -or [IntPtr]::Size -ne 8 -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw 'Use 64-bit Windows PowerShell/pwsh on x64 Windows' }
    $parsed = Convert-ReleaseVersion $Version
    if ($PreferencesRef -notmatch '^[a-fA-F0-9]{40}$') { throw 'PreferencesRef must be a full 40-character commit SHA' }
    $PreferencesRef = $PreferencesRef.ToLowerInvariant()
    if ($Publish) {
        # Fail before the long build: every published Release must say what changed.
        if (-not $ChangesFile) { throw 'Publish requires -ChangesFile' }
        $script:Changes = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $ChangesFile).Path)
        [void](Join-ReleaseNotes '' $script:Changes)
    }
    $script:RepoRoot = Get-Git @('rev-parse','--show-toplevel') $script:RepoRoot
    if (Get-Git @('status','--porcelain','--untracked-files=all')) { throw 'The source worktree must be clean, including untracked files' }
    $head = Get-Git @('rev-parse','HEAD')
    if (-not $CacheDir) { $CacheDir = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ChewingRelease' }
    $CacheDir = [IO.Path]::GetFullPath($CacheDir)
    $repoPrefix = $script:RepoRoot.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if ($CacheDir.Equals($script:RepoRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'CacheDir cannot be the source root' }
    if ($CacheDir.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        $relativeCache = $CacheDir.Substring($repoPrefix.Length).Replace('\','/') + '/'
        $ignored = Invoke-Tool 'git.exe' @('check-ignore','--quiet','--',$relativeCache) -Quiet -AllowFailure
        if ($ignored.Code -ne 0) { throw 'CacheDir inside the repo must be gitignored' }
    }
    New-Directory $CacheDir
    New-Directory (Join-Path $CacheDir 'logs')
    $runId = [Guid]::NewGuid().ToString('N')
    $script:LogFile = Join-Path $CacheDir ('logs/' + $Version + '-' + $runId + '.log')
    Write-Log ('Source HEAD: ' + $head)
    Write-Log ('Log: ' + $script:LogFile)
    $script:CacheLock = [IO.File]::Open((Join-Path $CacheDir 'release.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $toolStamp = @()
    foreach ($entry in @(@('git.exe','--version'),@('cargo.exe','--version'),@('rustc.exe','--version'),@('node.exe','--version'),@('npm.cmd','--version'),@('wix.exe','--version'))) {
        $versionText = (Invoke-Tool $entry[0] @($entry[1]) -Quiet).Text.Trim()
        Write-Log ($entry[0] + ': ' + $versionText)
        if ($entry[0] -eq 'wix.exe' -and $versionText -notmatch '^7\.') { throw 'WiX 7 is required; this script never installs WiX or accepts its EULA' }
        $toolStamp += $versionText
    }
    if ($Publish) { [void](Invoke-Tool 'gh.exe' @('auth','status','--hostname','github.com') -Quiet) }
    Enter-BuildShell
    $toolStamp += $env:VCToolsVersion
    $toolStamp += $env:WindowsSDKVersion
    foreach ($target in @('x86_64-pc-windows-msvc','i686-pc-windows-msvc')) {
        $lib = (Invoke-Tool 'rustc.exe' @('--print','target-libdir','--target',$target) -Quiet).Text.Trim()
        if (-not (Test-Path -LiteralPath $lib)) { throw ('Install the Rust target before running: ' + $target) }
    }
    $script:Temporary = Join-Path $CacheDir ('w/' + $runId.Substring(0, 12))
    New-Directory $script:Temporary
    New-Directory (Join-Path $script:Temporary 'tmp')
    $env:TEMP = Join-Path $script:Temporary 'tmp'; $env:TMP = $env:TEMP
    $env:CARGO_NET_RETRY = '0'; $env:CARGO_HTTP_TIMEOUT = '60'; $env:CARGO_INCREMENTAL = '0'
    $env:RUSTFLAGS = '-Ctarget-feature=+crt-static'
    Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    $env:ORT_DISABLE_TELEMETRY = '1'
    $env:npm_config_cache = Join-Path $CacheDir 'b/npm'
    $script:SourceWork = Join-Path $script:Temporary 't'
    [void](Invoke-Tool 'git.exe' @('worktree','add','--detach',$script:SourceWork,$head))
    $script:WorktreeAdded = $true
    $preferences = Join-Path $script:Temporary 'p'
    [void](Invoke-Tool 'git.exe' @('clone','--branch','valor-feature','--no-checkout','https://github.com/ValorVie/windows-chewing-preferences.git',$preferences))
    [void](Invoke-Tool 'git.exe' @('checkout','--detach',$PreferencesRef) $preferences)
    if ((Get-Git @('rev-parse','HEAD') $preferences) -cne $PreferencesRef) { throw 'Preferences checkout does not match PreferencesRef' }
    $script:LockHash = Get-Hash (Join-Path $script:SourceWork 'Cargo.lock')
    $preferenceLock = Get-Hash (Join-Path $preferences 'src-tauri/Cargo.lock')
    $libRevision = Get-LibchewingRevision (Join-Path $script:SourceWork 'Cargo.lock')
    $pins = Get-Pins $script:SourceWork
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $assets = Get-Assets $pins $CacheDir $script:Temporary
    $inputKey = Get-TextHash ("schema=1`nversion=$Version`nhead=$head`npref=$PreferencesRef`nlock=$script:LockHash`nprefLock=$preferenceLock`npins=$($pins.SourceHash)`ntools=$($toolStamp -join '|')`n")
    $artifactDir = Join-Path $CacheDir ('releases/' + $inputKey)
    $msiName = 'windows-chewing-tsf-d512-' + $Version + '-x64-unsigned.msi'
    $receiptPath = Join-Path $artifactDir 'build-manifest.json'
    if (Test-Path -LiteralPath $artifactDir) {
        $receipt = Get-Content -LiteralPath $receiptPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($receipt.InputKey -cne $inputKey -or $receipt.ProductVersion -cne $parsed.Msi -or $receipt.CheckInstaller -ne $true) { throw 'Cached build manifest does not match inputs' }
        foreach ($file in @(@($msiName,$receipt.MsiHash),@(($msiName + '.sha256'),$receipt.SumHash),@('release-notes.md',$receipt.NotesHash))) {
            if ((Get-Hash (Join-Path $artifactDir $file[0])) -cne $file[1]) { throw 'Cached build output changed; inspect it before rebuilding' }
        }
        if ((Get-MsiVersion (Join-Path $artifactDir $msiName)) -cne $parsed.Msi) { throw 'Cached MSI ProductVersion mismatch' }
        Write-Log 'Build cache hit: prior check-installer receipt, MSI version and all output hashes verified'
    } else {
        $env:CARGO_TARGET_DIR = Join-Path $CacheDir 'b/p'
        # Cargo searches ancestors even across Git boundaries. An ignored in-repo
        # cache must not make this standalone Tauri package join another workspace.
        $preferenceManifest = Join-Path $preferences 'src-tauri/Cargo.toml'
        $manifestBytes = [IO.File]::ReadAllBytes($preferenceManifest)
        $manifestText = [IO.File]::ReadAllText($preferenceManifest)
        $ancestor = [IO.Directory]::GetParent($preferences)
        $nestedCargo = $false
        while ($null -ne $ancestor) {
            if (Test-Path -LiteralPath (Join-Path $ancestor.FullName 'Cargo.toml')) { $nestedCargo = $true; break }
            $ancestor = $ancestor.Parent
        }
        if ($nestedCargo -and $manifestText -notmatch '(?m)^\[workspace(?:\]|\.)') {
            Write-Utf8 $preferenceManifest ($manifestText + "`n[workspace]`n")
            Write-Log 'Isolated temporary preferences package from ancestor Cargo workspace'
        }
        [void](Invoke-Tool 'npm.cmd' @('ci','--fetch-retries=0') $preferences)
        [void](Invoke-Tool 'npm.cmd' @('run','tauri','--','build','--no-bundle','--ci','--target','x86_64-pc-windows-msvc','--','--locked') $preferences)
        [IO.File]::WriteAllBytes($preferenceManifest, $manifestBytes)
        if ((Get-Hash (Join-Path $preferences 'src-tauri/Cargo.lock')) -cne $preferenceLock -or (Get-Git @('status','--porcelain','--untracked-files=all') $preferences)) { throw 'Preferences source/lock changed during build' }
        $preferenceExe = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release/ChewingPreferences.exe'
        $preferenceHash = Get-Hash $preferenceExe
        $env:CARGO_TARGET_DIR = Join-Path $CacheDir 'b/t'
        Invoke-Xtask @('update-version','--major',([string]$parsed.Major),'--minor',([string]$parsed.Minor),'--patch',([string]$parsed.Patch),'-b',([string]$parsed.Build))
        Invoke-Xtask @('download-components')
        Invoke-Xtask @('stage-d512','--from',$assets)
        Invoke-Xtask @('stage-preferences','--from',$preferenceExe,'--sha256',$preferenceHash)
        # The upstream staging task expects its own relative target directory.
        # Keep the retained build cache outside the disposable worktree via a junction.
        $targetLink = Join-Path $script:SourceWork 'target'
        [void](New-Item -ItemType Junction -Path $targetLink -Target $env:CARGO_TARGET_DIR)
        Invoke-Xtask @('build-installer','--target','msvc','--release','--without-arm64')
        Invoke-Xtask @('check-installer','--without-arm64')
        Invoke-Xtask @('package-installer','--without-arm64')
        $msi = Join-Path $script:SourceWork 'dist/windows-chewing-tsf-unsigned.msi'
        if ((Get-MsiVersion $msi) -cne $parsed.Msi) { throw 'Built MSI ProductVersion mismatch' }
        if ((Get-AuthenticodeSignature -LiteralPath $msi).Status.ToString() -cne 'NotSigned') { throw 'Expected unsigned MSI' }
        $output = Join-Path $script:Temporary 'out'; New-Directory $output
        Copy-Item -LiteralPath $msi -Destination (Join-Path $output $msiName)
        $msiHash = Get-Hash (Join-Path $output $msiName)
        Write-Utf8 (Join-Path $output ($msiName + '.sha256')) ($msiHash + ' *' + $msiName + "`n")
        $notes = @(
            ('# 預覽版 ' + $Version), '',
            '僅提供 x64 Windows。安裝包沒有簽章，安裝時會顯示「未知的發行者」。', '',
            ('MSI SHA-256：`' + $msiHash + '`'), '',
            ('- TSF commit：`' + $head + '`'),
            ('- libchewing commit（Cargo.lock）：`' + $libRevision + '`'),
            ('- 設定程式 commit：`' + $PreferencesRef + '`'),
            ('- 設定程式 exe SHA-256：`' + $preferenceHash + '`'),
            '- D512 模型：`d512-pf512-20260926`（Funmula/0c-sequence-d512）',
            '- ONNX Runtime：`1.31.0`', '',
            ('[功能、建置與授權說明](https://github.com/ValorVie/windows-chewing-tsf/blob/' + $head + '/README.md)'), ''
        ) -join "`n"
        Write-Utf8 (Join-Path $output 'release-notes.md') $notes
        $receipt = [ordered]@{ InputKey=$inputKey; ProductVersion=$parsed.Msi; TsfCommit=$head; LibchewingCommit=$libRevision; PreferencesCommit=$PreferencesRef; PreferencesHash=$preferenceHash; MsiHash=$msiHash; SumHash=(Get-Hash (Join-Path $output ($msiName + '.sha256'))); NotesHash=(Get-Hash (Join-Path $output 'release-notes.md')); CheckInstaller=$true }
        Write-Utf8 (Join-Path $output 'build-manifest.json') ($receipt | ConvertTo-Json)
        New-Directory (Split-Path -Parent $artifactDir)
        Move-Item -LiteralPath $output -Destination $artifactDir
        $receipt = [pscustomobject]$receipt
    }
    $result = [pscustomobject]@{ MsiName=$msiName; MsiPath=(Join-Path $artifactDir $msiName); SumPath=(Join-Path $artifactDir ($msiName + '.sha256')); NotesPath=(Join-Path $artifactDir 'release-notes.md'); MsiHash=$receipt.MsiHash; SumHash=$receipt.SumHash }
    Write-Log ('MSI: ' + $result.MsiPath)
    Write-Log ('Bytes: ' + (Get-Item -LiteralPath $result.MsiPath).Length)
    Write-Log ('SHA-256: ' + $result.MsiHash)
    Write-Log ('ProductVersion: ' + $parsed.Msi)
    Write-Log ('Components: TSF=' + $head + ' libchewing=' + $libRevision + ' preferences=' + $PreferencesRef + ' model=d512-pf512-20260926 ORT=1.31.0')
    Write-Log ('With -Publish: require pushed HEAD, create/reuse annotated tag v' + $Version + ', create a prerelease with MSI and checksum, or no-op for matching assets; conflicts stop without overwrite')
    if ($Publish) { Publish-Result $result $head ('v' + $Version) $script:Temporary }
    else { Write-Log 'PREVIEW ONLY: no gh authentication, tag, push or Release operation was executed' }
    if ((Get-Git @('rev-parse','HEAD')) -cne $head -or (Get-Git @('status','--porcelain','--untracked-files=all'))) { throw 'Original source worktree changed during release' }
    $targetLink = Join-Path $script:SourceWork 'target'
    if ((Test-Path -LiteralPath $targetLink) -and ((Get-Item -LiteralPath $targetLink).Attributes -band [IO.FileAttributes]::ReparsePoint)) { [IO.Directory]::Delete($targetLink) }
    [void](Invoke-Tool 'git.exe' @('worktree','remove','--force',$script:SourceWork))
    $script:WorktreeAdded = $false
    Remove-OwnedTree $script:Temporary
    Write-Log 'SUCCESS: output verified; temporary worktree/clone removed; build and asset caches retained'
}

$script:Utf8 = New-Object Text.UTF8Encoding($false)
$script:Secrets = @($env:GH_TOKEN,$env:GITHUB_TOKEN,$env:GH_ENTERPRISE_TOKEN,$env:GITHUB_ENTERPRISE_TOKEN,$env:NPM_TOKEN,$env:CARGO_REGISTRIES_CRATES_IO_TOKEN)
$script:ReleaseRepo = 'ValorVie/windows-chewing-tsf'
$script:RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$script:LogFile = $null; $script:Temporary = $null; $script:CacheLock = $null; $script:WorktreeAdded = $false
$savedEnvironment = @{}
Get-ChildItem Env: | ForEach-Object { $savedEnvironment[$_.Name] = $_.Value }
$savedTls = [Net.ServicePointManager]::SecurityProtocol
$exitCode = 0
try { Invoke-Release }
catch {
    $exitCode = 1
    Write-Log ('FAILED: ' + $_.Exception.Message)
    Write-Log ('Preserved temporary path: ' + $script:Temporary)
    Write-Log ('Log: ' + $script:LogFile)
} finally {
    if ($script:CacheLock) { $script:CacheLock.Dispose() }
    [Net.ServicePointManager]::SecurityProtocol = $savedTls
    foreach ($item in @(Get-ChildItem Env:)) { if (-not $savedEnvironment.ContainsKey($item.Name)) { [Environment]::SetEnvironmentVariable($item.Name, $null, 'Process') } }
    foreach ($name in $savedEnvironment.Keys) { if ([Environment]::GetEnvironmentVariable($name,'Process') -cne $savedEnvironment[$name]) { [Environment]::SetEnvironmentVariable($name,$savedEnvironment[$name],'Process') } }
}
exit $exitCode
