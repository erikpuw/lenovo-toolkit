use flate2::Compression;
use flate2::write::GzEncoder;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

fn compress_if_exists(src: &Path, dst: &Path) {
    let mut encoder = GzEncoder::new(
        File::create(dst).expect("không tạo được file payload nén"),
        Compression::best(),
    );
    if src.exists() {
        let bytes = fs::read(src).expect("không đọc được file nhị phân release");
        encoder.write_all(&bytes).expect("không nén được payload");
    }
    encoder.finish().expect("không hoàn tất được payload gzip");
}

fn resource_script(manifest: &str, icon: &str, version: &str) -> String {
    let mut nums = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
    );
    format!(
        r#"1 24 "{manifest}"
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x4
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "erikpuw"
      VALUE "FileDescription", "Lenovo Toolkit Setup"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "lenovo-toolkit-setup"
      VALUE "OriginalFilename", "lenovo-toolkit-setup.exe"
      VALUE "ProductName", "Lenovo Toolkit"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");

    let app_exe = root.join("target").join("release").join("lenovo-toolkit.exe");
    let cli_exe = root.join("target").join("release").join("lenovo-toolkit-cli.exe");

    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=../lenovo-toolkit/ui/icon.png");
    println!("cargo:rerun-if-changed={}", app_exe.display());
    println!("cargo:rerun-if-changed={}", cli_exe.display());

    // 1. Nén lenovo-toolkit.exe và lenovo-toolkit-cli.exe vào OUT_DIR
    compress_if_exists(&app_exe, &out_dir.join("lenovo_toolkit_payload.gz"));
    compress_if_exists(&cli_exe, &out_dir.join("lenovo_toolkit_cli_payload.gz"));

    // 2. Tạo icon.ico từ PNG 256×256 dùng chung với ứng dụng chính
    let png = include_bytes!("../lenovo-toolkit/ui/icon.png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "icon phải là PNG");
    assert_eq!(&png[16..24], &[0, 0, 1, 0, 0, 0, 1, 0], "icon phải có kích thước 256×256");
    let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&u32::try_from(png.len()).expect("icon quá lớn").to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    let icon_path = out_dir.join("icon.ico");
    fs::write(&icon_path, ico).expect("không ghi được icon.ico");

    // 3. Nhúng manifest (asInvoker + PerMonitorV2), icon và thông tin phiên bản
    let rc_path = out_dir.join("setup.rc");
    let manifest_str = manifest_dir.join("app.manifest").to_string_lossy().replace('\\', "/");
    let icon_str = icon_path.to_string_lossy().replace('\\', "/");
    fs::write(&rc_path, resource_script(&manifest_str, &icon_str, &version))
        .expect("không ghi được setup.rc");

    embed_resource::compile_for(&rc_path, ["lenovo-toolkit-setup"], embed_resource::NONE)
        .manifest_required()
        .expect("không nhúng được tài nguyên (cần Windows SDK rc.exe)");
}
