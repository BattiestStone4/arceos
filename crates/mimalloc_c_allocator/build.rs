use std::env;

fn main() {
    let mut build = cc::Build::new();
    build.include("c_src/mimalloc/include");
    build.include("c_src/mimalloc/src");
    build.file("c_src/mimalloc/src/static.c");
    build.compile("mimalloc");
}
