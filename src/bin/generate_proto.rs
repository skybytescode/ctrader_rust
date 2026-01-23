extern crate prost_build;

use std::path::Path;

fn main() {
    let out_dir = Path::new("src/proto/generated");
    std::fs::create_dir_all(out_dir).expect("failed to create output directory");

    match prost_build::Config::new()
        .out_dir(out_dir)
        .include_file("openapi.rs")
        .compile_protos(
            &[
                "src/proto/OpenApiCommonMessages.proto",
                "src/proto/OpenApiMessages.proto",
                "src/proto/OpenApiModelMessages.proto",
                "src/proto/OpenApiCommonModelMessages.proto",
            ],
            &["src/proto"],
        ) {
        Ok(_) => println!("Proto compilation successful"),
        Err(e) => {
            eprintln!("Proto compilation failed: {}", e);
            std::process::exit(1);
        }
    }
}