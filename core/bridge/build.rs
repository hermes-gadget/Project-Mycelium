fn main() {
    println!("cargo:rerun-if-changed=src/meshemu_bridge_clock.cpp");
    println!("cargo:rerun-if-changed=include/meshemu_bridge_clock.h");

    let objects = cc::Build::new()
        .cpp(true)
        .file("src/meshemu_bridge_clock.cpp")
        .include("include")
        .flag_if_supported("-std=c++17")
        .cargo_metadata(false)
        .compile_intermediates();

    // Link the object directly instead of through cc-rs's static archive.
    // Rust cdylib builds pass --exclude-libs=ALL, which would otherwise make
    // these C ABI symbols local even when they have default visibility.
    for object in objects {
        println!("cargo:rustc-cdylib-link-arg={}", object.display());
    }

    // The direct object contains C++ standard-library references. Publish the
    // runtime dependency for the bridge and for Rust test/executable linkers.
    println!("cargo:rustc-link-lib=dylib=stdc++");

    // Symbols originating in a linked C++ object are not automatically
    // promoted to the dynamic export table of a Rust cdylib. Add them to a
    // linker version script so the public header and the shipped .so agree.
    let symbols = [
        "meshemu_clock_create",
        "meshemu_clock_millis",
        "meshemu_clock_set_offset",
        "meshemu_clock_destroy",
    ];
    let export_script = std::env::var_os("OUT_DIR")
        .expect("Cargo always provides OUT_DIR")
        .into_string()
        .expect("OUT_DIR must be valid UTF-8")
        + "/meshemu_clock.exports";
    let body = format!("{{\n  global:\n{};\n}};\n", symbols.join(";\n"));
    std::fs::write(&export_script, body).expect("write clock export list");
    println!("cargo:rustc-cdylib-link-arg=-Wl,--version-script={export_script}");
}
