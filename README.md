# Lenovo Toolkit (v1.0.1)

Bản port Rust của LenovoLegionToolkit. Mục tiêu: nói chuyện trực tiếp với driver
Lenovo, giải phóng handle bằng RAII, giữ RAM thấp và chạy ổn định cùng Windows.

## Thiết bị được hỗ trợ

Mã nguồn phân hai nhóm theo `compat::device_class` (dựa trên tên model):

| Nhóm | Dòng máy | Trạng thái |
|---|---|---|
| **Gaming** | Legion, LOQ, IdeaPad Gaming | Hỗ trợ đầy đủ: chế độ nguồn SmartFan (Quiet/Balance/Performance/Extreme/Custom), quạt và cảm biến WMI, Custom Mode, pin, đèn, phím Fn |
| **Không gaming** | ThinkBook, IdeaPad, Lenovo Slim, Yoga, XiaoXin, Motobook | **Chưa hỗ trợ.** Các máy này dùng ITS (Intelligent Thermal Solution, DYTC qua driver) với chế độ Auto/Cool/Performance/Geek, bảng capability Custom Mode khác và không chắc có WMI GameZone |

Với máy không gaming, ứng dụng chỉ đọc cảm biến: giao diện hiện cảnh báo và **ẩn mọi điều khiển ghi phần cứng**, tray và CLI từ chối lệnh ghi
(`set`, `off`, `disable`, `enable`, `refresh`; riêng `lenovo-toolkit-cli software ...` vẫn dùng được), và **không** tự chặn phần mềm Lenovo khi khởi động
(vì app chưa xử lý được phím Fn trên các máy này). Chưa thử trên máy không gaming nào nên không ghi ITS khi chưa có phần cứng kiểm chứng.

## Cấu trúc

- `crates/lenovo-toolkit-core`: driver (`EnergyDrv`, `IBMPmDrv`), chế độ sạc pin, ngưỡng sạc.
- `crates/lenovo-toolkit-cli`: file chạy `lenovo-toolkit-cli.exe`.

## Lệnh

```
lenovo-toolkit-cli battery get
lenovo-toolkit-cli battery set <normal|rapid|conservation>   (cần admin)
lenovo-toolkit-cli power get                                 (cần admin: WMI root\WMI)
lenovo-toolkit-cli power set <quiet|balance|performance|extreme|godmode>   (cần admin)
lenovo-toolkit-cli gamezone <winkey|gsync|overdrive|touchpad|igpu> <get|supported|set <n>>   (cần admin: WMI)
lenovo-toolkit-cli <fnlock|nightcharge|onelevel> <get|set <on|off>>
lenovo-toolkit-cli fan <sensors|max|fullspeed <get|set <on|off>>>   (cần admin: WMI)
lenovo-toolkit-cli light <ports|logo|kbd> <get|supported|set <n>>    (cần admin: WMI)
lenovo-toolkit-cli info                                      (thông tin máy, khả năng hỗ trợ; cần admin)
lenovo-toolkit-cli keys                                      (in phím Fn đặc biệt; đóng bản C# trước khi thử)
lenovo-toolkit-cli events                                    (in sự kiện đổi chế độ nguồn, ví dụ Fn+Q; cần admin)
lenovo-toolkit-cli usb <get|set <off|sleeping|always>>
lenovo-toolkit-cli kbd <get|max|set <level>>
lenovo-toolkit-cli threshold get
lenovo-toolkit-cli threshold set <start> <stop>              (cần admin)
lenovo-toolkit-cli threshold off                             (cần admin)
```

`threshold` cần key registry `HKLM\SOFTWARE\WOW6432Node\Lenovo\PWRMGRV` do phần mềm Lenovo tạo;
máy không có key này sẽ báo "không hỗ trợ".

## Giao diện (Slint)

`crates/lenovo-toolkit` (file chạy `lenovo-toolkit.exe`, tên hiển thị **Lenovo Toolkit v1.0.1**): giao diện tinh gọn kèm System Tray Win32 và chống phình RAM:
- **Trang Dashboard (Home)**: Bảng chỉ số gộp gọn trên đầu (CPU: `%`, `°C`, `RPM` · GPU: `%` qua NVAPI, `°C`, `RPM` · System: `RAM GB/%`, `PCH °C`, `Pin %`), bỏ toàn bộ các dòng `-` thừa. Tên CPU, GPU và dòng máy (`Legion Y7000 IRX9`) được đọc trực tiếp từ Registry (`< 0.5ms`) giúp khởi động tức thì. Bên dưới là các nhóm điều khiển xếp 1 cột dọc từ trên xuống: **Power** → **Graphics** (`GPU Working Mode`, `Discrete GPU`, `Overclock GPU`) → **Display** → **Other**, tự động giãn chiều cao thẻ không cắt chữ.
- **Trang Battery**: Đọc trực tiếp qua IOCTL (`GUID_DEVCLASS_BATTERY` + `EnergyDrv` `0x83102138`) hiển thị đầy đủ như bản gốc: phần trăm, trạng thái, cảnh báo pin yếu/sạc công suất thấp, nhiệt độ pin, tốc độ sạc/xả hiện tại/min/max (W), dung lượng hiện tại/sạc đầy/thiết kế (Wh), sức khỏe pin (%), số chu kỳ sạc, ngày sản xuất và ngày dùng đầu tiên.
- **Trang Settings**: Căn đỉnh chuẩn, gồm **App Behavior** (`Language: English / Tiếng Việt`, `Autorun`, `Minimize to tray`, `Minimize on close`), **Power** (`Switch to Custom Mode with Fn+Q`, `Power Mode Synchronization`, `Reset "On battery since" at startup`), và **Software Control** (`Lenovo Vantage`, `Legion Zone`, `Legion Space`, `Lenovo Hotkeys`).
- **System Tray & Tối ưu hiệu năng / RAM**:
  - Biểu tượng dưới khay hệ thống (`Shell_NotifyIconW`): click trái mở lại cửa sổ, click phải đổi nhanh `Quiet / Balance / Performance` hoặc `Quit`.
  - **Tooltip khi rê chuột** (như bản gốc, Win32 + GDI, không dựng thêm cửa sổ Slint): hiện chế độ nguồn, CPU (% · °C · RPM), GPU (% · °C · RPM), RAM và pin. Số liệu chỉ được đọc đúng lúc bạn rê chuột vào (sự kiện `NIN_POPUPOPEN`, `NOTIFYICON_VERSION_4`) nên lúc nhàn rỗi không tốn CPU hay RAM; tự đóng khi rời chuột, bấm chuột hoặc sau 8 giây.
  - **Thông báo phím** (một dòng chữ giữa đáy màn hình, 1,8 giây, Win32 + GDI): đổi chế độ nguồn (Fn+Q), Fn Lock bật/tắt, camera bật/tắt, đèn bàn phím trắng (Tắt/Thấp/Cao, Fn+Space), **Fn+F4 tắt/bật mic** (app đổi mute của mọi thiết bị thu rồi báo kết quả) và **Fn+F10 touchpad** (đọc trạng thái khóa qua WMI GameZone `GetTPStatus` như bản gốc). Fn+F4/F10/Space đến từ driver `EnergyDrv` (`keys.rs`) như `DriverKeyListener` của bản gốc; Fn Lock/camera/đèn qua WMI `LENOVO_UTILITY_EVENT`. Mỗi thông báo có icon Segoe Fluent Icons (xanh khi bật, xám khi tắt). Thông báo bo góc nhẹ, chữ đậm. Bật/tắt trong mục riêng **Settings → Thông báo**: công tắc chung "Thông báo phím" và từng loại (chế độ nguồn, Fn Lock/Camera, mic, touchpad, đèn bàn phím). Tự bỏ qua khi có game toàn màn hình, trình chiếu hoặc "không làm phiền" (`SHQueryUserNotificationState`). Chưa có: AC adapter, Num/Caps Lock, chế độ máy bay (Fn+F8), đèn LED trên phím F4, tùy chỉnh vị trí/thời lượng/từng loại thông báo như bản gốc.
  - Khi chạy với `--minimized` (Autorun) hoặc bấm đóng/thu nhỏ xuống Tray: cửa sổ ẩn hoàn toàn khỏi Taskbar, luồng worker tạm dừng hoàn toàn việc đọc cảm biến WMI/UI, và gọi `EmptyWorkingSet` để thu hồi bộ nhớ đệm RAM về mức tối thiểu.
  - Luồng worker tái sử dụng kết nối WMI (`Option<Wmi>`) thay vì mở mới mỗi 2 giây, chỉ đọc cảm biến khi đang ở tab Dashboard và chỉ đọc chi tiết pin khi đang ở tab Battery.
- Phiên bản lấy từ một nguồn duy nhất là `version` trong `Cargo.toml` (hiện 1.0.1): `build.rs` sinh tài nguyên phiên bản (ProductName "Lenovo Toolkit", FileVersion, ProductVersion) nhúng vào file `.exe`, và `lenovo_toolkit::app_title()` dùng cho tiêu đề cửa sổ và tooltip khay hệ thống. Khi đổi phiên bản chỉ cần sửa `version` ở các `Cargo.toml`.
- File chạy `lenovo-toolkit.exe` nhúng manifest `requireAdministrator` và icon chính thức nên Windows tự hỏi UAC khi mở. Nếu đã bật Autorun từ bản cũ (`lenovo-toolkit.exe`), tắt rồi bật lại Autorun trong Settings để task trỏ tới `lenovo-toolkit.exe`. Vì vậy `cargo run` phải chạy từ terminal admin; test của bin bị tắt (`test = false`), logic thuần nằm ở `src/lib.rs`.

```
cargo run --release -p lenovo-toolkit
```

## Phạm vi (bản ổn định, nhẹ)

Mục tiêu: app chạy ổn định cùng Windows, ít RAM, điều khiển quạt/pin/đèn/chế độ nguồn và chặn phần mềm
quản lý phím gốc của Lenovo. **Cố ý không port**: Macro, Automation (action theo app), Downloads, Donate,
About, Spectrum RGB, OSD, cập nhật, tích hợp, phím tắt mở app / phím tắt tần số quét.

- **Custom Mode** (GodMode, nền tảng Legion): `custom.rs`. Hai cấu hình: *Lenovo default (recommended)* =
  mặc định của hãng, **bị khóa**; *Custom* = người dùng chỉnh theo bước của firmware, có cảnh báo. Mặc định
  chọn Lenovo default và tự áp dụng khi vào chế độ Custom (Fn+Q). Quạt tối đa mặc định tắt, có cảnh báo
  mòn quạt, và luôn được ghi lại (tắt) khi áp dụng để không bị kẹt ở 100%. Chưa có: biểu đồ quạt,
  ép xung AMD, NVAPI.
- **Chặn phần mềm Lenovo** (`disabler.rs`, trang Settings > Software Control): FnKeys, Vantage, Legion Zone,
  Legion Space; áp dụng lại khi khởi động (mặc định chặn Lenovo Hotkeys).
- **Autorun** (`autorun.rs`, Settings > App Behavior): Scheduled Task đăng nhập quyền cao nhất, tên riêng
  `LegionToolkitRust_Autorun`, khởi động với `--minimized`. Nếu bản C# cũng bật autorun thì tắt một trong hai.

## Build / test

```
cargo test --workspace
cargo build --release
```

## Đã port / chưa port

Đã: chế độ sạc pin, ngưỡng sạc, power mode (get/set qua WMI, chưa kiểm tra AC và chưa có
workaround lỗi chuyển Quiet→Performance / GodMode→khác của bản C#).
Đã thêm: `energy` (khóa Fn thô, Always-on USB, sạc đêm, đèn bàn phím 1 mức và nhiều mức),
`gamezone` (phím Win, G-Sync, Overdrive, khóa touchpad, chế độ iGPU), helper WMI chung `wmi_util`.
Đã kiểm chứng trên máy: các lệnh đọc của `energy`. Lệnh `gamezone` chưa thử (cần admin).
Đã thêm: `fan` (nhiệt độ CPU/GPU/PCH, tốc độ quạt, tốc độ tối đa, full speed; chọn 1 trong 6 profile
theo `LENOVO_FAN_TABLE_DATA`), `lighting` (đèn cổng, logo, bàn phím trắng qua WMI). Chưa kiểm chứng
trên máy (cần admin). Chưa: profile FanTestData, bảng quạt, Spectrum RGB (HID), LampArray, NVAPI.
Đã thêm: `keys` (listener phím Fn đặc biệt FnQ/F4/F8/F10/Space qua EnergyDrv, event Windows + IOCTL
`KEY_WAIT_HANDLE`/`KEY_VALUE`). Chưa kiểm chứng trên máy; hành động theo phím (tắt mic, thông báo) nằm ở
`audio` và `events::UtilityEventListener`; chưa có chế độ máy bay và chưa bỏ qua phím khi dịch vụ FnKeys của Lenovo còn bật.
Đã thêm: `events` (sự kiện đổi chế độ nguồn do firmware báo qua WMI `LENOVO_GAMEZONE_SMART_FAN_MODE_EVENT`
và `..._THERMAL_MODE_EVENT`, ví dụ Fn+Q). `keys` đã kiểm chứng trên máy (FnF4/F8/F10/Space); FnQ không đi qua
driver mà qua sự kiện WMI này. Giới hạn: luồng nền chỉ thoát sau sự kiện kế tiếp khi listener bị drop.
Đã thêm: `compat` (nhận diện dòng máy qua machine type/model, thế hệ, BIOS, chế độ nguồn hỗ trợ,
nền tảng GodMode, các cờ lỗi theo BIOS, loại trừ đèn); `fnlock` đã áp dụng đảo trạng thái theo dòng máy;
giao diện hiện nút Extreme/GodMode khi máy hỗ trợ và tự làm mới khi bấm Fn+Q.
Chưa trong compat: fake_mi.json, FeatureData, SupportsAlwaysOnAc.
Chưa: quạt, đèn bàn phím, listener phím Fn (`IOCTL_KEY_WAIT_HANDLE`), disabler
phần mềm Lenovo, Automation, UI.
Bỏ qua có chủ đích: đồng bộ registry của Vantage khi đổi chế độ sạc.
