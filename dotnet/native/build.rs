//! Generate the C# DllImport surface with csbindgen and place it next to the
//! managed wrapper it belongs to.
use std::path::Path;

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/NativeMethods.g.cs");
    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .csharp_dll_name("anydoc_dotnet")
        .csharp_namespace("Firecrawl.Anydoc.Native")
        .csharp_class_name("AnydocNative")
        .csharp_class_accessibility("internal")
        .csharp_use_function_pointer(true)
        .generate_csharp_file(out.to_str().expect("csharp output path is utf-8"))
        .unwrap();
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/serialize.rs");
}
