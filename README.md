# 新酷音 Windows ＋ D512

這是新酷音 Windows 版的 fork，加入 D512 句級重排，由模型替候選句打分。
支援關閉、自動、僅建議三種模式，參數檔位與建議接受鍵比照 Mac 唯音的 fork；
另有 D512 校準紀錄開關，預設關閉。

## 原始碼

三個 fork 都使用 `valor-feature` 分支：

| Fork | 上游 | 用途 |
| --- | --- | --- |
| [ValorVie/libchewing](https://github.com/ValorVie/libchewing/tree/valor-feature) | [Codeberg](https://codeberg.org/chewing/libchewing) | 注音引擎、D512 重排與模型打分介面 |
| [ValorVie/windows-chewing-tsf](https://github.com/ValorVie/windows-chewing-tsf/tree/valor-feature) | [Codeberg](https://codeberg.org/chewing/windows-chewing-tsf) | Windows 輸入法、打分宿主與 MSI；本 repo 為下載及建置入口 |
| [ValorVie/windows-chewing-preferences](https://github.com/ValorVie/windows-chewing-preferences/tree/valor-feature) | [Codeberg](https://codeberg.org/chewing/windows-chewing-preferences) | 設定程式，包含 D512 模式、參數、接受鍵與校準紀錄開關 |

## 下載

請到[本 repo 的 Releases](https://github.com/ValorVie/windows-chewing-tsf/releases) 下載。
目前只提供 x64 Windows 的未簽章安裝包，安裝時會顯示「未知的發行者」。

## 建置與詞庫轉換

先取得本 fork：

```sh
git clone --branch valor-feature https://github.com/ValorVie/windows-chewing-tsf.git
cd windows-chewing-tsf
```

工具需求沿用下方的 [Tool Requirements](#tool-requirements)。
D512 模型、ONNX Runtime 與 MSI 步驟見 [Prepare the installer](docs/d512-windows.md#prepare-the-installer)。
設定程式須從 `windows-chewing-preferences` 的 `valor-feature` 建出 `ChewingPreferences.exe`，
再用 `stage-preferences` 帶入，詳見[設定程式建置說明](https://github.com/ValorVie/windows-chewing-preferences/blob/valor-feature/docs/d512-preferences.md)。
唯音使用者詞庫轉換見 [vchewing-to-chewing](https://github.com/ValorVie/vchewing-to-chewing)。

## 授權

本 repo 採 GPL-3.0-or-later，libchewing 採 LGPL-2.1-or-later。
[D512 模型（Funmula/0c-sequence-d512）](https://huggingface.co/Funmula/0c-sequence-d512)與 ONNX Runtime 採 MIT；
模型與 ONNX Runtime 的授權檔已隨 MSI 安裝。

# windows-chewing-tsf

Implement chewing in Windows via Text Services Framework:

* chewing_tip contains an implementation of Windows text service for libchewing.
* tsfreg contains TSF registration helper used in the installer.
* preferences contains the user preference and phrase editor GUI.

All parts are licensed under GPL-3.0-or-later license.

# Development

## Tool Requirements

**Build natively on Windows using MSVC**

* [Build Tools for Visual Studio 2022](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)
* [NodeJS Native](https://nodejs.org/)
* [Rust](https://rustup.rs/)

**Cross compile on Windows using MinGW-W64**

* [MSYS2](https://www.msys2.org/)
* [NodeJS Native](https://nodejs.org/)
* [Rust](https://rustup.rs/)

**Cross compile on Fedora using MinGW-W64**

* [NodeJS Native](https://nodejs.org/)
* [Rust](https://rustup.rs/)

## How to Build

* Get source from codeberg
    ```bash
    git clone https://codeberg.org/chewing/windows-chewing-tsf.git
    cd windows-chewing-tsf
    ```
* Use this xtask command to build the installer
    ```
    cargo xtask build-installer --target msvc --release
    cargo xtask download-components
    cargo xtask package-installer
    ```

## TSF References

* [Text Services Framework](http://msdn.microsoft.com/en-us/library/windows/desktop/ms629032%28v=vs.85%29.aspx)
* [Guidelines and checklist for IME development (Windows Store apps)](http://msdn.microsoft.com/en-us/library/windows/apps/hh967425.aspx)
* [Input Method Editors (Windows Store apps)](http://msdn.microsoft.com/en-us/library/windows/apps/hh967426.aspx)
* [Third-party input method editors](http://msdn.microsoft.com/en-us/library/windows/desktop/hh848069%28v=vs.85%29.aspx)
* [Strategies for App Communication between Windows 8 UI and Windows 8 Desktop](http://software.intel.com/en-us/articles/strategies-for-app-communication-between-windows-8-ui-and-windows-8-desktop)
* [TSF Aware, Dictation, Windows Speech Recognition, and Text Services Framework. (blog)](http://blogs.msdn.com/b/tsfaware/?Redirected=true)
* [Win32 and COM for Windows Store apps](http://msdn.microsoft.com/en-us/library/windows/apps/br205757.aspx)
* [Input Method Editor (IME) sample supporting Windows 8](http://code.msdn.microsoft.com/windowsdesktop/Input-Method-Editor-IME-b1610980)

## Windows ACL (Access Control List) references

* [The Windows Access Control Model Part 1](http://www.codeproject.com/Articles/10042/The-Windows-Access-Control-Model-Part-1#SID)
* [The Windows Access Control Model: Part 2](http://www.codeproject.com/Articles/10200/The-Windows-Access-Control-Model-Part-2#SidFun)
* [Windows 8 App Container Security Notes - Part 1](http://recxltd.blogspot.tw/2012/03/windows-8-app-container-security-notes.html)
* [How AccessCheck Works](http://msdn.microsoft.com/en-us/library/windows/apps/aa446683.aspx)
* [GetAppContainerNamedObjectPath function (enable accessing object outside app containers using ACL)](http://msdn.microsoft.com/en-us/library/windows/desktop/hh448493)
* [Creating a DACL](http://msdn.microsoft.com/en-us/library/windows/apps/ms717798.aspx)

# Code Signing Policy

This [project](https://signpath.org/projects/chewing-im/) is sponsored by SignPath. We use free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

**People and roles:**

* Committers and reviewers: [Chewing core team](https://codeberg.org/org/chewing/teams/owners), [Windows Chewing maintainers](https://codeberg.org/org/chewing/teams/windows)
* Approvers: [Chewing core team](https://codeberg.org/org/chewing/teams/owners)

# Privacy Policy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it.

# Bug Report
Please report any issue to [here](https://codeberg.org/chewing/windows-chewing-tsf/issues).
