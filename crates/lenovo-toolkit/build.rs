use std::path::PathBuf;
use std::{env, fs};

/// Sinh `app.rc` từ phiên bản trong Cargo.toml: manifest quyền admin, icon, và thông tin phiên bản
/// (Properties > Details của file .exe: tên sản phẩm, FileVersion, ProductVersion).
fn resource_script(manifest: &str, icon: &str, version: &str) -> String {
    let mut nums = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0), nums.next().unwrap_or(0));
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
      VALUE "FileDescription", "Lenovo Toolkit"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "lenovo-toolkit"
      VALUE "OriginalFilename", "lenovo-toolkit.exe"
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
    slint_build::compile("ui/app.slint").expect("không biên dịch được ui/app.slint");

    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let path = |name: &str| dir.join(name).to_string_lossy().replace('\\', "/");

    // ICO chứa trực tiếp PNG 256×256; dùng chung ảnh với giao diện.
    let png = include_bytes!("ui/icon.png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "icon phải là PNG");
    assert_eq!(&png[16..24], &[0, 0, 1, 0, 0, 0, 1, 0], "icon phải có kích thước 256×256");
    let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&u32::try_from(png.len()).expect("icon quá lớn").to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    let icon = out.join("icon.ico");
    fs::write(&icon, ico).expect("không ghi được icon.ico");

    let rc = out.join("app.rc");
    fs::write(&rc, resource_script(&path("app.manifest"), &icon.to_string_lossy().replace('\\', "/"), &version)).expect("không ghi được app.rc");

    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=ui/icon.png");
    // Nhúng vào file chạy `lenovo-toolkit` (không áp cho test harness).
    embed_resource::compile_for(&rc, ["lenovo-toolkit"], embed_resource::NONE)
        .manifest_required()
        .expect("không nhúng được tài nguyên (cần Windows SDK rc.exe)");
}
