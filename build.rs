//! Builds the embedded web assets: compiles `web/app.jsx` to plain JS with oxc,
//! content-hashes every asset and generates `assets_generated.rs` listing them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use oxc::CompilerInterface;
use oxc::codegen::CodegenReturn;
use oxc::diagnostics::Diagnostics;
use oxc::minifier::{CompressOptions, MangleOptions};
use oxc::span::SourceType;
use oxc::transformer::{JsxRuntime, TransformOptions};

const FONTS: &[&str] = &[
    "source-serif-4-400.woff2",
    "source-serif-4-600.woff2",
    "source-serif-4-700.woff2",
    "jetbrains-mono-400.woff2",
    "jetbrains-mono-600.woff2",
    "jetbrains-mono-700.woff2",
];

struct Asset {
    /// Constant name in the generated file (when the page needs to reference it).
    constant: Option<&'static str>,
    /// Hashed file name, e.g. `app.0123456789abcdef.js`.
    name: String,
    content_type: &'static str,
    file: PathBuf,
}

/// JSX -> plain JS using the classic runtime (`React.createElement`).
struct Jsx {
    options: TransformOptions,
    output: String,
    errors: Diagnostics,
}

impl CompilerInterface for Jsx {
    fn handle_errors(&mut self, errors: Diagnostics) {
        self.errors.extend(errors);
    }

    fn transform_options(&self) -> Option<&TransformOptions> {
        Some(&self.options)
    }

    fn compress_options(&self) -> Option<CompressOptions> {
        Some(CompressOptions::smallest())
    }

    fn mangle_options(&self) -> Option<MangleOptions> {
        Some(MangleOptions::default())
    }

    fn codegen_options(&self) -> Option<oxc::codegen::CodegenOptions> {
        Some(oxc::codegen::CodegenOptions {
            minify: true,
            comments: oxc::codegen::CommentOptions::disabled(),
            ..Default::default()
        })
    }

    fn after_codegen(&mut self, ret: CodegenReturn<'_>) {
        self.output = ret.code;
    }
}

fn compile_jsx(path: &Path) -> String {
    let source = fs::read_to_string(path).expect("read app.jsx");
    let mut options = TransformOptions::default();
    options.jsx.runtime = JsxRuntime::Classic;
    let mut jsx = Jsx {
        options,
        output: String::new(),
        errors: Diagnostics::default(),
    };
    jsx.compile(&source, SourceType::jsx(), path);
    if !jsx.errors.is_empty() {
        panic!("failed to compile {}: {:?}", path.display(), jsx.errors);
    }
    jsx.output
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// `stem.ext` -> `stem.<hash>.ext`
fn hashed_name(file_name: &str, bytes: &[u8]) -> String {
    let (stem, ext) = file_name.rsplit_once('.').expect("file extension");
    format!("{stem}.{:016x}.{ext}", fnv1a(bytes))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=web");
    // sqlx::migrate!() embeds migrations/ at compile time.
    println!("cargo:rerun-if-changed=migrations");

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web = root.join("web");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_assets = out.join("assets");
    fs::create_dir_all(&out_assets).unwrap();

    let mut assets: Vec<Asset> = Vec::new();
    let mut emit = |constant: Option<&'static str>,
                    file_name: &str,
                    content_type: &'static str,
                    bytes: &[u8]|
     -> String {
        let name = hashed_name(file_name, bytes);
        let file = out_assets.join(&name);
        fs::write(&file, bytes).unwrap();
        assets.push(Asset {
            constant,
            name: name.clone(),
            content_type,
            file,
        });
        name
    };

    // Fonts first: the CSS refers to their hashed names.
    let mut css = fs::read_to_string(web.join("styles.css")).unwrap();
    for font in FONTS {
        let bytes = fs::read(web.join("fonts").join(font)).unwrap();
        let name = emit(None, font, "font/woff2", &bytes);
        let placeholder = format!("fonts/{font}");
        assert!(css.contains(&placeholder), "styles.css does not use {font}");
        css = css.replace(&placeholder, &format!("/assets/{name}"));
    }

    emit(
        Some("CSS"),
        "styles.css",
        "text/css; charset=utf-8",
        css.as_bytes(),
    );
    for (constant, file) in [
        ("REACT", "react.production.min.js"),
        ("REACT_DOM", "react-dom.production.min.js"),
    ] {
        let bytes = fs::read(web.join("vendor").join(file)).unwrap();
        emit(
            Some(constant),
            file,
            "text/javascript; charset=utf-8",
            &bytes,
        );
    }
    let app = compile_jsx(&web.join("app.jsx"));
    emit(
        Some("APP"),
        "app.js",
        "text/javascript; charset=utf-8",
        app.as_bytes(),
    );

    let mut generated = String::new();
    generated.push_str("pub struct Asset {\n    pub name: &'static str,\n    pub content_type: &'static str,\n    pub bytes: &'static [u8],\n}\n\n");
    for asset in assets.iter().filter(|a| a.constant.is_some()) {
        writeln!(
            generated,
            "pub const {}_PATH: &str = \"/assets/{}\";",
            asset.constant.unwrap(),
            asset.name
        )
        .unwrap();
    }
    generated.push_str("\npub static ASSETS: &[Asset] = &[\n");
    for asset in &assets {
        writeln!(
            generated,
            "    Asset {{ name: {:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},",
            asset.name,
            asset.content_type,
            asset.file.display().to_string()
        )
        .unwrap();
    }
    generated.push_str("];\n");
    fs::write(out.join("assets_generated.rs"), generated).unwrap();
}
