fn main() {
    println!("cargo:rerun-if-changed=SpoutLibrary.dll");
    let dll_path = std::path::PathBuf::from("SpoutLibrary.dll");
    if dll_path.exists() {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        std::fs::copy(
            &dll_path,
            std::path::Path::new(&out_dir).join("SpoutLibrary.dll"),
        )
        .unwrap();
    }
}
