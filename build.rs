use image::imageops::FilterType;
use std::env;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

const ICON_SOURCE: &str = "assets/nocaps-icon.png";
const WINDOW_ICON_SIZE: u32 = 64;

fn main() {
    println!("cargo:rerun-if-changed={ICON_SOURCE}");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let source = image::ImageReader::open(ICON_SOURCE)
        .expect("open nocaps icon")
        .decode()
        .expect("decode nocaps icon");
    let window_icon = source
        .resize_exact(WINDOW_ICON_SIZE, WINDOW_ICON_SIZE, FilterType::Nearest)
        .into_rgba8();
    fs::write(out_dir.join("nocaps.rgba"), window_icon.as_raw())
        .expect("write generated window icon");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon(&out_dir, &source);
    }
}

fn embed_windows_icon(out_dir: &Path, source: &image::DynamicImage) {
    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let rgba = source
            .resize_exact(size, size, FilterType::Nearest)
            .into_rgba8()
            .into_raw();
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        directory.add_entry(ico::IconDirEntry::encode(&image).expect("encode application icon"));
    }

    let mut bytes = Cursor::new(Vec::new());
    directory
        .write(&mut bytes)
        .expect("serialize application icon");
    let icon_path = out_dir.join("nocaps.ico");
    fs::write(&icon_path, bytes.into_inner()).expect("write application icon");

    let rc_path = out_dir.join("nocaps.rc");
    fs::write(&rc_path, format!("1 ICON \"{}\"\n", icon_path.display()))
        .expect("write resource script");
    embed_resource::compile(rc_path);
}
