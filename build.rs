use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc.in");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");

    // Delay-loaded DLLs:
    // 1) speed — network, dialogs, shell and COM are not needed to show a document;
    // 2) security — uxtheme, dwmapi and bcryptprimitives are not KnownDLLs, so with a normal
    //    import Windows looks for them in the exe folder first (DLL planting next to a downloaded
    //    installer). Delay-loaded, they are loaded after SetDefaultDllDirectories(SYSTEM32)
    //    from security.rs — only from System32.
    // Check: dumpbin /imports dist\vibe-md-viewer.exe — only KnownDLLs must remain at startup.
    for dll in [
        "winhttp.dll",
        "shell32.dll",
        "comdlg32.dll",
        "ole32.dll",
        "oleaut32.dll",
        "uxtheme.dll",
        "dwmapi.dll",
        "ws2_32.dll",
        "bcryptprimitives.dll",
    ] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
    // In the test build some DLLs are not imported — the LNK4199 warning is expected there.
    println!("cargo:rustc-link-arg-bins=/ignore:4199");

    // Resources: the exe version (file properties) comes from Cargo.toml — the single source of the version.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<String> = version.split(['.', '-']).take(3).map(|s| s.to_string()).collect();
    while parts.len() < 3 {
        parts.push("0".into());
    }
    let v4 = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let esc = |p: PathBuf| p.to_string_lossy().replace('\\', "\\\\");
    let tpl = std::fs::read_to_string(manifest_dir.join("assets/app.rc.in")).unwrap();
    let rc = tpl
        .replace("{{ICON}}", &esc(manifest_dir.join("assets/app.ico")))
        .replace("{{MANIFEST}}", &esc(manifest_dir.join("assets/app.manifest")))
        .replace("{{V4}}", &v4)
        .replace("{{VERSION}}", &version);
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_required().unwrap();
}
