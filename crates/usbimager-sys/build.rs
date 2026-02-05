use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect_c_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_c_files(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("c") {
                out.push(path);
            }
        }
    }
}

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" {
        println!("cargo:warning=usbimager-sys only builds the C engine on Linux");
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("../../vendor/usbimager/src");

    let mut build = cc::Build::new();
    build
        .flag("-std=c99")
        .define("DISKS_TEST", "0")
        .define("_FILE_OFFSET_BITS", "64")
        .define("__USE_FILE_OFFSET64", "1")
        .define("__USE_LARGEFILE", "1")
        .define("ZSTD_LEGACY_SUPPORT", "0")
        .define("ZSTD_LIB_DICTBUILDER", "0")
        .define("ZSTD_LIB_DEPRECATED", "0")
        .define("ZSTD_LIB_MINIFY", "1")
        .define("ZSTD_STATIC_LINKING_ONLY", "1")
        .define("ZSTD_STRIP_ERROR_STRINGS", "1")
        .define("DEBUGLEVEL", "0")
        .define("XZ_USE_CRC64", None)
        .define("XZ_DEC_ANY_CHECK", None)
        .warnings(false)
        .include(&vendor)
        .include(vendor.join("zlib"))
        .include(vendor.join("bzip2"))
        .include(vendor.join("xz"))
        .include(vendor.join("zstd"))
        .include(manifest_dir.join("c"));

    build.file(manifest_dir.join("c/usbimager_core.c"));
    build.file(manifest_dir.join("c/xz_crc64.c"));
    build.file(vendor.join("stream.c"));
    build.file(vendor.join("lang.c"));
    build.file(vendor.join("disks_linux.c"));

    let mut files = Vec::new();
    collect_c_files(&vendor.join("zlib"), &mut files);
    collect_c_files(&vendor.join("bzip2"), &mut files);
    collect_c_files(&vendor.join("xz"), &mut files);
    collect_c_files(&vendor.join("zstd/common"), &mut files);
    collect_c_files(&vendor.join("zstd/compress"), &mut files);
    collect_c_files(&vendor.join("zstd/decompress"), &mut files);

    for file in files {
        build.file(file);
    }

    build.compile("usbimager_core");

    println!("cargo:rustc-link-lib=pthread");
}
