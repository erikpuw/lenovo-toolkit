//! Icon của thông báo phím, tự vẽ bằng nét đơn giản trên lưới 24×24 (không dùng font icon có sẵn).
//! Phần này chỉ mô tả hình; `tray.rs` vẽ bằng GDI (phóng to rồi thu nhỏ để mịn cạnh).

/// Lưới tọa độ của mọi icon.
pub const GRID: f32 = 24.0;
/// Độ dày nét trên lưới.
pub const STROKE: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Power,
    Keyboard,
    Camera,
    Mic,
    Touchpad,
    Backlight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// (x1, y1, x2, y2)
    Line(f32, f32, f32, f32),
    /// Hộp bao (left, top, right, bottom), bán kính góc.
    RoundRect(f32, f32, f32, f32, f32),
    /// Hộp bao (left, top, right, bottom).
    Ellipse(f32, f32, f32, f32),
    /// Hộp bao (left, top, right, bottom), góc đầu và cuối theo độ, ngược chiều kim đồng hồ từ trục +x.
    Arc(f32, f32, f32, f32, f32, f32),
    /// Chấm đặc (tâm x, tâm y, bán kính).
    Dot(f32, f32, f32),
}

/// Các nét của icon; `off` thêm một nét gạch chéo (tắt).
pub fn shapes(kind: IconKind, off: bool) -> Vec<Shape> {
    use Shape::*;
    let mut s = match kind {
        // Đồng hồ tốc độ: nửa vòng, kim và trục.
        IconKind::Power => vec![Arc(3.0, 5.0, 21.0, 23.0, 0.0, 180.0), Line(12.0, 14.0, 17.0, 9.0), Dot(12.0, 14.0, 1.6)],
        IconKind::Keyboard => vec![
            RoundRect(2.0, 6.0, 22.0, 18.0, 3.0),
            Dot(7.0, 10.0, 1.1),
            Dot(12.0, 10.0, 1.1),
            Dot(17.0, 10.0, 1.1),
            Line(8.0, 14.5, 16.0, 14.5),
        ],
        IconKind::Camera => vec![
            RoundRect(2.0, 7.0, 22.0, 20.0, 3.0),
            Ellipse(8.0, 10.0, 16.0, 18.0),
            Line(8.0, 7.0, 9.5, 4.5),
            Line(9.5, 4.5, 14.5, 4.5),
            Line(14.5, 4.5, 16.0, 7.0),
        ],
        IconKind::Mic => vec![
            RoundRect(8.5, 2.0, 15.5, 14.0, 3.5),
            Arc(5.0, 6.0, 19.0, 20.0, 180.0, 360.0),
            Line(12.0, 20.0, 12.0, 22.5),
            Line(8.5, 22.5, 15.5, 22.5),
        ],
        IconKind::Touchpad => vec![
            RoundRect(2.0, 4.5, 22.0, 19.5, 3.0),
            Line(2.0, 14.5, 22.0, 14.5),
            Line(12.0, 14.5, 12.0, 19.5),
        ],
        IconKind::Backlight => {
            let mut v = vec![Ellipse(8.0, 8.0, 16.0, 16.0)];
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                let (c, s) = (a.cos(), a.sin());
                v.push(Line(12.0 + 7.0 * c, 12.0 - 7.0 * s, 12.0 + 10.0 * c, 12.0 - 10.0 * s));
            }
            v
        }
    };
    if off {
        s.push(Line(4.0, 20.0, 20.0, 4.0));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [IconKind; 6] =
        [IconKind::Power, IconKind::Keyboard, IconKind::Camera, IconKind::Mic, IconKind::Touchpad, IconKind::Backlight];

    fn points(s: &Shape) -> Vec<f32> {
        match *s {
            Shape::Line(a, b, c, d) | Shape::RoundRect(a, b, c, d, _) | Shape::Ellipse(a, b, c, d) => vec![a, b, c, d],
            Shape::Arc(a, b, c, d, ..) => vec![a, b, c, d],
            Shape::Dot(x, y, _) => vec![x, y],
        }
    }

    #[test]
    fn every_icon_has_strokes_inside_the_grid() {
        for kind in ALL {
            let s = shapes(kind, false);
            assert!(!s.is_empty(), "{kind:?} trống");
            for sh in &s {
                for p in points(sh) {
                    assert!((0.0..=GRID).contains(&p), "{kind:?}: {sh:?} vượt lưới");
                }
            }
        }
    }

    #[test]
    fn off_adds_one_slash_only() {
        for kind in ALL {
            assert_eq!(shapes(kind, true).len(), shapes(kind, false).len() + 1);
        }
    }

    #[test]
    fn icons_differ_from_each_other() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(shapes(*a, false), shapes(*b, false), "{a:?} trùng {b:?}");
            }
        }
    }
}
