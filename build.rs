use std::fs;
use std::path::PathBuf;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>OneAPI</title>
  </head>
  <body>
    <h1>OneAPI</h1>
    <p>前端尚未构建。请运行 <code>make web</code> 后重新构建后端。</p>
  </body>
</html>
"#;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo"),
    );
    let dist = manifest_dir.join("web").join("dist");
    let index = dist.join("index.html");

    if !index.exists() {
        fs::create_dir_all(&dist).expect("failed to create web/dist");
        fs::write(&index, PLACEHOLDER).expect("failed to write placeholder index.html");
        println!(
            "cargo:warning=web/dist/index.html was missing; wrote a placeholder. Run `make web` for the real frontend."
        );
    }

    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
