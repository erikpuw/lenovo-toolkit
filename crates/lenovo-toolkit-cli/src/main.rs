use std::process::ExitCode;

use lenovo_toolkit_core::{
    ai, games,
    battery::{self, BatteryState},
    compat, custom,
    disabler::{self, Software},
    energy::{self, AlwaysOnUsb},
    events::{EventListener, Source},
    fan,
    gamezone::{self, Feature},
    keys::KeyListener,
    lighting::{self, Light},
    wmi_util::Wmi,
    power::{self, PowerMode},
    threshold::{self, ChargeThreshold},
};

const USAGE: &str = "dùng:
  lenovo-toolkit-cli battery get
  lenovo-toolkit-cli battery set <normal|rapid|conservation>   (cần admin)
  lenovo-toolkit-cli power get
  lenovo-toolkit-cli power set <quiet|balance|performance|extreme|godmode>
  lenovo-toolkit-cli gamezone <winkey|gsync|overdrive|touchpad|igpu> <get|supported|set <n>>
  lenovo-toolkit-cli <fnlock|nightcharge|onelevel> <get|set <on|off>>
  lenovo-toolkit-cli fan <sensors|max|fullspeed <get|set <on|off>>>   (cần admin: WMI)
  lenovo-toolkit-cli light <ports|logo|kbd> <get|supported|set <n>>    (cần admin: WMI)
  lenovo-toolkit-cli info                                              (thông tin máy và khả năng hỗ trợ; cần admin)
  lenovo-toolkit-cli ai <status|refresh|set <on|off>>                  (AI Engine ở chế độ Cân bằng; cần admin)
  lenovo-toolkit-cli custom show                                       (chế độ Custom: khoảng giá trị, mặc định Lenovo, giá trị hiện tại; chỉ đọc; cần admin)
  lenovo-toolkit-cli games                                             (game đang chạy theo danh sách GameConfigStore)
  lenovo-toolkit-cli software status                                   (phần mềm Lenovo gốc: fnkeys, vantage, legionzone, legionspace; cần admin)
  lenovo-toolkit-cli software <fnkeys|vantage|legionzone|legionspace> <disable|enable>
  lenovo-toolkit-cli keys                                              (in phím Fn đặc biệt, Ctrl+C để thoát)
  lenovo-toolkit-cli events                                            (in sự kiện đổi chế độ nguồn, ví dụ Fn+Q; cần admin)
  lenovo-toolkit-cli usb <get|set <off|sleeping|always>>
  lenovo-toolkit-cli kbd <get|max|set <level>>
  lenovo-toolkit-cli threshold get
  lenovo-toolkit-cli threshold set <start> <stop>              (cần admin)
  lenovo-toolkit-cli threshold off                             (cần admin)";

fn parse_state(s: &str) -> Option<BatteryState> {
    match s {
        "normal" => Some(BatteryState::Normal),
        "rapid" => Some(BatteryState::RapidCharge),
        "conservation" => Some(BatteryState::Conservation),
        _ => None,
    }
}

fn parse_power(s: &str) -> Option<PowerMode> {
    match s {
        "quiet" => Some(PowerMode::Quiet),
        "balance" => Some(PowerMode::Balance),
        "performance" => Some(PowerMode::Performance),
        "extreme" => Some(PowerMode::Extreme),
        "godmode" => Some(PowerMode::GodMode),
        _ => None,
    }
}

fn parse_onoff(s: &str) -> Result<bool, &'static str> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(USAGE),
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();

    // Máy không thuộc nhóm gaming (ITS: ThinkBook, IdeaPad, Slim, Yoga...) chưa được hỗ trợ: không ghi phần cứng.
    // `software ...` (chặn phần mềm Lenovo) không phải ghi phần cứng nên không bị chặn.
    let writes = a.iter().any(|t| matches!(*t, "set" | "off" | "disable" | "enable" | "refresh"));
    if writes && a.first() != Some(&"software") {
        let info = compat::detect()?;
        if info.class() == compat::DeviceClass::NonGaming {
            return Err(format!(
                "{}: thiết bị dùng ITS, không thuộc nhóm gaming nên chưa hỗ trợ ghi phần cứng. Hỗ trợ: Legion, LOQ, IdeaPad Gaming.",
                info.model
            )
            .into());
        }
    }

    match a.as_slice() {
        ["battery", "get"] => println!("{:?}", battery::get(&battery::open()?)?),
        ["battery", "set", s] => {
            let state = parse_state(s).ok_or(USAGE)?;
            let actual = battery::set(&battery::open()?, state)?;
            println!("{actual:?}");
        }
        ["power", "get"] => println!("{:?}", power::get()?),
        ["power", "set", s] => {
            let w = Wmi::open()?;
            let info = compat::detect_with(&w).ok();
            power::switch_with(&w, info.as_ref(), parse_power(s).ok_or(USAGE)?)?;
            println!("{:?}", power::get()?);
        }
        ["threshold", "get"] => println!("{:?}", threshold::get()?),
        ["threshold", "set", start, stop] => {
            let t = ChargeThreshold::new(true, start.parse()?, stop.parse()?)?;
            threshold::set(&threshold::open()?, t)?;
            println!("{:?}", threshold::get()?);
        }
        ["threshold", "off"] => {
            let cur = threshold::get()?;
            let t = ChargeThreshold { enabled: false, ..cur };
            threshold::set(&threshold::open()?, t)?;
            println!("{:?}", threshold::get()?);
        }
        ["gamezone", f, rest @ ..] => {
            let feature = Feature::parse(f).ok_or(USAGE)?;
            match rest {
                ["get"] => println!("{}", gamezone::get(feature)?),
                ["supported"] => println!("{}", gamezone::is_supported(feature)?),
                ["set", v] => {
                    gamezone::set(feature, v.parse()?)?;
                    println!("{}", gamezone::get(feature)?);
                }
                _ => return Err(USAGE.into()),
            }
        }
        ["fan", "sensors"] => {
            let w = Wmi::open()?;
            println!("{:?}", fan::read(&w, fan::detect(&w)?)?);
        }
        ["fan", "max"] => {
            let w = Wmi::open()?;
            let (cpu, gpu) = fan::max_speeds(&w, fan::detect(&w)?)?;
            println!("cpu={cpu} gpu={gpu}");
        }
        ["fan", "fullspeed", "get"] => println!("{}", fan::full_speed_get(&Wmi::open()?)?),
        ["fan", "fullspeed", "set", v] => fan::full_speed_set(&Wmi::open()?, parse_onoff(v)?)?,
        ["light", l, rest @ ..] => {
            let light = Light::parse(l).ok_or(USAGE)?;
            let w = Wmi::open()?;
            match rest {
                ["get"] => println!("{}", lighting::get(&w, light)?),
                ["supported"] => println!("{}", lighting::is_supported(&w, light)?),
                ["set", v] => {
                    lighting::set(&w, light, v.parse()?)?;
                    println!("{}", lighting::get(&w, light)?);
                }
                _ => return Err(USAGE.into()),
            }
        }
        ["software", "status"] => {
            for sw in Software::ALL {
                println!("{:<12} {:?}", sw.name(), disabler::status(sw)?);
            }
        }
        ["software", name, action @ ("disable" | "enable")] => {
            let sw = Software::parse(name).ok_or(USAGE)?;
            if *action == "disable" {
                let r = disabler::disable(sw)?;
                println!("{:?}", r);
                if !r.not_stopped.is_empty() || r.status == disabler::Status::Enabled {
                    eprintln!("Chưa tắt hẳn được: có thể cần khởi động lại máy.");
                }
            } else {
                disabler::enable(sw)?;
                println!("{:?}", disabler::status(sw)?);
            }
        }
        ["custom", "show"] => {
            let w = Wmi::open()?;
            let info = compat::detect()?;
            let ranges = custom::read_ranges(&w)?;
            let defaults = custom::read_mode_defaults(&w).unwrap_or_default();
            let lenovo = custom::lenovo_default(&ranges, &defaults, info.properties.supports_extreme_mode);
            println!("{:<42} {:>7} {:>8} {:>6} {:>6} {:>5}  bước", "capability", "hiện tại", "Lenovo", "min", "max", "step");
            for r in &ranges {
                let t = custom::cap_info(r.id).map_or("?", |c| c.title);
                let cur = custom::get_value(&w, r.id).map_or("-".into(), |v| v.to_string());
                let steps = if r.steps.is_empty() { String::new() } else { format!("{:?}", r.steps) };
                println!("{t:<42} {cur:>7} {:>8} {:>6} {:>6} {:>5}  {steps}", lenovo.values[&r.id], r.min, r.max, r.step);
            }
            let fan = custom::get_value(&w, custom::FAN_FULL_SPEED_ID).map_or("-".into(), |v| v.to_string());
            println!("{:<42} {fan:>7} {:>8}", custom::FAN_MAX_TITLE, 0);
            println!("mode defaults (bảng firmware): {} dòng", defaults.len());
        }
        ["games"] => {
            for p in games::running()? {
                println!("{} (pid {})", p.name, p.pid);
            }
        }
        ["ai", "status"] => {
            println!("enabled     = {}", ai::is_enabled());
            println!("sub_mode    = {}", ai::sub_mode_get()?);
            println!("power_mode  = {:?}", power::get().ok());
            println!("adapter     = {:?}", power::adapter_status());
            println!("games       = {}", games::running()?.len());
            println!("op_list     = {} mục", ai::op_list()?.len());
        }
        ["ai", "refresh"] => println!("{:?}", ai::refresh()?),
        ["ai", "set", v] => {
            let on = parse_onoff(v)?;
            ai::set_enabled(on)?;
            if on {
                println!("{:?}", ai::refresh()?);
            } else {
                println!("đã ghi sub_mode 0: {}", ai::disable_if_needed()?);
            }
        }
        ["info"] => {
            let info = compat::detect()?;
            println!("{info:#?}");
            println!("class = {:?}", info.class());
            println!("compatible = {}", compat::is_compatible(&info)?);
        }
        ["events"] => {
            let _smart = EventListener::start(Source::SmartFanMode, |raw, m| println!("smartfan raw={raw} mode={m:?}"))?;
            let _thermal = EventListener::start(Source::ThermalMode, |raw, m| println!("thermal raw={raw} mode={m:?}"))?;
            eprintln!("đang nghe sự kiện đổi chế độ nguồn... (Ctrl+C để thoát)");
            loop {
                std::thread::park();
            }
        }
        ["keys"] => {
            let _listener = KeyListener::start(|raw, keys| println!("raw={raw} keys={keys:?}"))?;
            eprintln!("đang nghe phím Fn... (Ctrl+C để thoát)");
            loop {
                std::thread::park();
            }
        }
        ["fnlock", "get"] => {
            let series = compat::detect()?.series;
            let on = energy::fn_lock_translate(series, energy::fn_lock_get(&battery::open()?)?);
            println!("{on}");
        }
        ["fnlock", "set", v] => {
            let series = compat::detect()?.series;
            // phép đảo là đối xứng nên dùng lại để đổi trạng thái mong muốn sang phần cứng
            let hw = energy::fn_lock_translate(series, parse_onoff(v)?);
            energy::fn_lock_set(&battery::open()?, hw)?;
        }
        ["nightcharge", "get"] => println!("{}", energy::night_charge_get(&battery::open()?)?),
        ["nightcharge", "set", v] => energy::night_charge_set(&battery::open()?, parse_onoff(v)?)?,
        ["onelevel", "get"] => println!("{}", energy::one_level_get(&battery::open()?)?),
        ["onelevel", "set", v] => energy::one_level_set(&battery::open()?, parse_onoff(v)?)?,
        ["usb", "get"] => println!("{:?}", energy::usb_get(&battery::open()?)?),
        ["usb", "set", s] => energy::usb_set(&battery::open()?, AlwaysOnUsb::parse(s).ok_or(USAGE)?)?,
        ["kbd", "get"] => println!("{}", energy::kbd_get(&battery::open()?)?),
        ["kbd", "max"] => println!("{}", energy::kbd_max_level(&battery::open()?)?),
        ["kbd", "set", n] => energy::kbd_set(&battery::open()?, n.parse()?)?,
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
