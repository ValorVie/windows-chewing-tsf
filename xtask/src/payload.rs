// SPDX-License-Identifier: GPL-3.0-or-later

use scoped_error::{Error, expect_error};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, io::Read, path::Path};

// MIT checkpoint d512-pf512-20260926; ORT 1.31.0 CPU win-x64.
// Runtime DLL hashes are from the official archive, whose SHA-256 is
// 3958d8a44984160692c1b4ffd0de3cb48f8c39f9d34bb31e433db06e30b82195.
pub(crate) const D512_ASSETS: &[(&str, &str)] = &[
    (
        "model.onnx",
        "71af1f0e77b2752cc71cbc04bf19c8e8f2e765662246f766e65a2822bab4e66e",
    ),
    (
        "vocab.json",
        "556dffaffbf4c05211a846dcee55ef735cea68c5754f9bffcc6f9588124172b3",
    ),
    (
        "manifest.json",
        "f8d737cf44e9ff8f90e7108f1e9df4fece0cdcae254e050968fc6ef723ee1976",
    ),
    (
        "onnxruntime.dll",
        "0c42dc4e278f0968fdf9ebf1b1e54a74c657a9942e70803e453c91993451cf4c",
    ),
    (
        "onnxruntime_providers_shared.dll",
        "e45d471ea2a842b268c51291b96d53568b0dae10807f43d1fb7dc0eb84e83723",
    ),
    (
        "D512-LICENSE.txt",
        "ab1525e5aba48f95223228d1fcd045820783e6b3846972fbec67970314177e4f",
    ),
    (
        "ORT-LICENSE.txt",
        "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674",
    ),
    (
        "ORT-ThirdPartyNotices.txt",
        "c53a76501ef60db6f865f20599f220761201ac4057683acefdab37d861b86622",
    ),
];
fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("missing asset {}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!("SHA-256 mismatch: {}", path.display()))
    }
}
fn verify_files(root: &Path, assets: &[(&str, &str)]) -> Result<(), String> {
    for (name, expected) in assets {
        verify_file(&root.join(name), expected)?;
    }
    Ok(())
}
fn is_arm64_source(source: &str) -> bool {
    matches!(source, "x64/chewing_tip_arm64.dll" | "x64/chewing_tip.dll")
}
fn installation_folder<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.ancestors()
        .find(|n| n.has_tag_name("Directory") || n.has_tag_name("StandardDirectory"))
}
fn validate_layout(
    root: &Path,
    xml: &str,
    without_arm64: bool,
    assets: &[(&str, &str)],
) -> Result<(), String> {
    verify_files(root, assets)?;
    let document = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let files: Vec<_> = document
        .descendants()
        .filter(|n| n.has_tag_name("File"))
        .filter_map(|n| n.attribute("Source").map(|s| (n, s.replace('\\', "/"))))
        .filter(|(_, source)| !without_arm64 || !is_arm64_source(source))
        .collect();
    let host = files
        .iter()
        .find(|(_, source)| source == "chewing_tip_host.exe")
        .ok_or("host missing from MSI")?
        .0;

    let host_folder = installation_folder(host).ok_or("host has no installation directory")?;
    if host_folder.attribute("Id") != Some("INSTALLFOLDER64")
        || !host
            .ancestors()
            .any(|n| n.attribute("Id") == Some("ProgramFiles64Folder"))
    {
        return Err("host must be inside protected Program Files".into());
    }
    let mut installed = HashSet::new();
    for (node, source) in &files {
        let path = root.join(source);
        if !path.is_file() || path.metadata().map_err(|e| e.to_string())?.len() == 0 {
            return Err(format!("missing/empty MSI payload: {source}"));
        }
        let directory = installation_folder(*node).ok_or("file has no installation directory")?;
        let name = node
            .attribute("Name")
            .unwrap_or(source.rsplit('/').next().unwrap_or(source))
            .to_lowercase();
        if !installed.insert((directory.id(), name)) {
            return Err("installed file names collide".into());
        }
        if assets.iter().any(|(name, _)| *name == source)
            || source.ends_with("chewing_tip_x64.dll")
            || source == "x86/chewing_tip.dll"
            || is_arm64_source(source)
        {
            if directory != host_folder {
                return Err(format!("payload is not beside host: {source}"));
            }
        }
    }
    for (name, _) in assets {
        if !files.iter().any(|(_, source)| source == name) {
            return Err(format!("asset omitted from MSI: {name}"));
        }
    }
    for source in ["x64/chewing_tip_x64.dll", "x86/chewing_tip.dll"] {
        if !files.iter().any(|(_, s)| s == source) {
            return Err(format!("TIP omitted from MSI: {source}"));
        }
    }
    Ok(())
}
fn stage_files(source: &Path, destination: &Path, assets: &[(&str, &str)]) -> Result<(), String> {
    // Verify the entire immutable package before changing an existing stage.
    verify_files(source, assets)?;
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for (name, _) in assets {
        fs::copy(source.join(name), destination.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn stage_preferences(source: &Path, destination: &Path, expected: &str) -> Result<(), String> {
    verify_file(source, expected)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(source, destination).map_err(|e| e.to_string())?;
    verify_file(destination, expected)
}

pub(crate) fn stage_d512(source: &Path) -> Result<(), Error> {
    expect_error("Failed to stage D512", || {
        stage_files(source, Path::new("build/installer"), D512_ASSETS).map_err(Error::new)?;
        Ok(())
    })
}
pub(crate) fn stage_preference_artifact(source: &Path, expected: &str) -> Result<(), Error> {
    expect_error("Failed to stage preferences", || {
        stage_preferences(
            source,
            Path::new("build/installer/ChewingPreferences.exe"),
            expected,
        )
        .map_err(Error::new)?;
        Ok(())
    })
}
pub(crate) fn check_installer(without_arm64: bool) -> Result<(), Error> {
    expect_error("Invalid installer payload", || {
        let root = Path::new("build/installer");
        let xml = fs::read_to_string(root.join("windows-chewing-tsf.wxs"))?;
        validate_layout(root, &xml, without_arm64, D512_ASSETS).map_err(Error::new)?;
        eprintln!(
            "Installer payload verified (ARM64 included: {})",
            !without_arm64
        );
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ASSETS: &[(&str, &str)] = &[("model.onnx", ABC), ("vocab.json", ABC)];
    const XML: &str = r#"<Wix><Package><StandardDirectory Id="ProgramFiles64Folder"><Directory Id="INSTALLFOLDER64">
<File Source="chewing_tip_host.exe"/><File Source="x64\chewing_tip_x64.dll"/>
<File Source="x64\chewing_tip_arm64.dll"/><File Source="x64\chewing_tip.dll"/>
<File Source="x86\chewing_tip.dll" Name="chewing_tip_x86.dll"/>
<File Source="model.onnx"/><File Source="vocab.json"/>
</Directory></StandardDirectory></Package></Wix>"#;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "c2-payload-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("x64")).unwrap();
            fs::create_dir_all(root.join("x86")).unwrap();
            for file in [
                "chewing_tip_host.exe",
                "x64/chewing_tip_x64.dll",
                "x64/chewing_tip_arm64.dll",
                "x64/chewing_tip.dll",
                "x86/chewing_tip.dll",
                "model.onnx",
                "vocab.json",
            ] {
                fs::write(root.join(file), b"abc").unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn complete_payload_is_valid() {
        let f = Fixture::new();
        assert!(validate_layout(&f.0, XML, false, ASSETS).is_ok());
    }
    #[test]
    fn missing_and_corrupt_assets_are_rejected() {
        let f = Fixture::new();
        fs::remove_file(f.0.join("model.onnx")).unwrap();
        assert!(verify_files(&f.0, ASSETS).is_err());
        fs::write(f.0.join("model.onnx"), b"changed").unwrap();
        assert!(verify_files(&f.0, ASSETS).is_err());
    }
    #[test]
    fn failed_asset_validation_does_not_create_a_partial_stage() {
        let f = Fixture::new();
        fs::write(f.0.join("vocab.json"), b"changed").unwrap();
        let dest = f.0.join("staging");
        assert!(stage_files(&f.0, &dest, ASSETS).is_err());
        assert!(!dest.exists());
    }
    #[test]
    fn an_asset_present_on_disk_but_omitted_from_msi_is_rejected() {
        let f = Fixture::new();
        let xml = XML.replace("<File Source=\"model.onnx\"/>", "");
        assert!(validate_layout(&f.0, &xml, false, ASSETS).is_err());
    }
    #[test]
    fn an_asset_installed_apart_from_the_host_is_rejected() {
        let f = Fixture::new();
        let xml = XML.replace(
            "<File Source=\"model.onnx\"/>",
            "<Directory Id=\"Elsewhere\"><File Source=\"model.onnx\"/></Directory>",
        );
        assert!(validate_layout(&f.0, &xml, false, ASSETS).is_err());
    }
    #[test]
    fn dll_names_cannot_collide_in_the_host_directory() {
        let f = Fixture::new();
        let xml = XML.replace(" Name=\"chewing_tip_x86.dll\"", "");
        assert!(validate_layout(&f.0, &xml, false, ASSETS).is_err());
    }
    #[test]
    fn only_the_explicit_x64_variant_may_omit_arm64_files() {
        let f = Fixture::new();
        fs::remove_file(f.0.join("x64/chewing_tip_arm64.dll")).unwrap();
        fs::remove_file(f.0.join("x64/chewing_tip.dll")).unwrap();
        assert!(validate_layout(&f.0, XML, true, ASSETS).is_ok());
        assert!(validate_layout(&f.0, XML, false, ASSETS).is_err());
    }
    #[test]
    fn preferences_source_and_hash_are_verified_before_replacing_staging() {
        let f = Fixture::new();
        let source = f.0.join("custom-settings.exe");
        let dest = f.0.join("ChewingPreferences.exe");
        fs::write(&source, b"abc").unwrap();
        stage_preferences(&source, &dest, ABC).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
        fs::write(&source, b"changed").unwrap();
        assert!(stage_preferences(&source, &dest, ABC).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
    }
}
