fn main() {
    // The embuild half: kicks off/locates the ESP-IDF build and re-exports
    // its cfg flags (esp_idf_bt_enabled, ...) and linker args to cargo.
    // This replaces the hand-rolled linker args in boards/esp32/build.rs.
    embuild::espidf::sysenv::output();

    // The .env half: same compile-time secret injection as boards/esp32
    // (see its build.rs) — SPEAKER_NAME instead of WiFi credentials.
    println!("cargo:rerun-if-changed=.env");
    if let Ok(contents) = std::fs::read_to_string(".env") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'');
                println!("cargo:rustc-env={}={}", k, v);
            }
        }
    }

    println!("cargo:rerun-if-env-changed=SPEAKER_NAME");
}
