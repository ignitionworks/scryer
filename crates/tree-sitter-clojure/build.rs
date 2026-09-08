fn main() {
    let src_dir = std::path::Path::new("grammar-src/src");
    let parser_path = src_dir.join("parser.c");

    cc::Build::new()
        .include(src_dir)
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-trigraphs")
        .file(&parser_path)
        .compile("tree-sitter-clojure-parser");

    println!("cargo:rerun-if-changed={}", parser_path.display());
}
