use super::*;

#[test]
fn app_bundle_install_atomically_replaces_verified_target_and_cleans_backup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("package").join(MACOS_APP_BUNDLE);
    let source_contents = source.join("Contents");
    fs::create_dir_all(&source_contents).expect("create source Contents");
    fs::write(
        source_contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.156</string>
</dict></plist>"#,
    )
    .expect("write source plist");
    fs::write(source_contents.join("payload"), "new").expect("write source payload");

    let install_dir = temp.path().join("install");
    let target = install_dir.join(MACOS_APP_BUNDLE);
    let target_contents = target.join("Contents");
    fs::create_dir_all(&target_contents).expect("create old Contents");
    fs::write(
        target_contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>0.0.155</string>
</dict></plist>"#,
    )
    .expect("write old plist");

    install_desktop_package(
        &source,
        &install_dir,
        &target,
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect("install verified app bundle");

    assert_eq!(
        installed_desktop_app_version(&target).as_deref(),
        Some("0.0.156")
    );
    assert_eq!(
        fs::read_to_string(target.join("Contents/payload")).expect("read payload"),
        "new"
    );
    let backup = install_dir.join(format!(".{}.backup", MACOS_APP_BUNDLE));
    assert!(!backup.exists(), "successful swap must remove its backup");
    copy_dir_replace(&target, &target, "0.0.156", CALLER_MANAGED_PROGRESS_SOURCE)
        .expect("same verified source and target is already complete");

    let no_parent = copy_dir_replace(
        &target,
        Path::new(""),
        "0.0.156",
        CALLER_MANAGED_PROGRESS_SOURCE,
    )
    .expect_err("empty target has no parent directory");
    assert!(no_parent.to_string().contains("target has no parent"));
}
