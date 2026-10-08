fn main() {
    println!("cargo:rustc-check-cfg=cfg(bifrost_proxy_test_io)");
    println!("cargo:rerun-if-env-changed=BIFROST_BUILD_PROXY_TEST_IO");
    match std::env::var("BIFROST_BUILD_PROXY_TEST_IO").as_deref() {
        Ok("1") => println!("cargo:rustc-cfg=bifrost_proxy_test_io"),
        Ok("") | Ok("0") | Err(std::env::VarError::NotPresent) => {}
        _ => panic!("BIFROST_BUILD_PROXY_TEST_IO must be 1, 0, or unset"),
    }

    println!("cargo:rerun-if-env-changed=BIFROST_VERSION");

    if let Ok(version) = std::env::var("BIFROST_VERSION") {
        println!("cargo:rustc-env=CARGO_PKG_VERSION={}", version);
    }
}
