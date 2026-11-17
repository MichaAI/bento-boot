//! Compiles the UI. The fonts (Nunito as static instances, and JetBrains
//! Mono for the log drawer) come from
//! `BENTO_FONT_DIR`, which the Nix build sets; they are embedded.

use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=BENTO_FONT_DIR");
    let fonts = PathBuf::from(std::env::var("BENTO_FONT_DIR").expect(
        "set BENTO_FONT_DIR to a directory with Nunito-{Regular,Bold,ExtraBold}.ttf and JetBrainsMono-Regular.ttf",
    ));
    for f in ["Nunito-Regular.ttf", "Nunito-Bold.ttf", "Nunito-ExtraBold.ttf", "JetBrainsMono-Regular.ttf"] {
        assert!(fonts.join(f).is_file(), "{} is missing from BENTO_FONT_DIR", f);
    }
    let config = slint_build::CompilerConfiguration::new()
        .with_library_paths(HashMap::from([("fonts".to_owned(), fonts)]))
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("UI compiles");
}
