use windows::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
};

/// A window position and size in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The work area of the monitor showing `rect` and that monitor's scale
/// factor. Windows placed there are sized for this scale factor, not for the
/// one of the monitor they are on now.
pub fn monitor_work_area(rect: RECT) -> (RECT, f64) {
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };

    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (mut dpi_x, mut dpi_y) = (96, 96);
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
            dpi_x = 96;
        }
    }
    (monitor_info.rcWork, f64::from(dpi_x) / 96.0)
}

/// Places a window of `width` x `height` logical pixels below `caret`, or
/// above it when it does not fit below, `offset_x` logical pixels left of
/// the caret, and keeps it inside `work` (the work area of the caret's
/// monitor, whose scale factor is `scale`).
pub fn place_at_caret(
    caret: RECT,
    work: RECT,
    scale: f64,
    width: f64,
    height: f64,
    offset_x: f64,
) -> Placement {
    let to_physical = |value: f64| (value * scale).round() as i32;
    let width = to_physical(width).clamp(1, (work.right - work.left).max(1));
    let height = to_physical(height).clamp(1, (work.bottom - work.top).max(1));

    let below = caret.bottom;
    let above = caret.top - height;
    let y = if below + height <= work.bottom {
        below
    } else if above >= work.top {
        above
    } else {
        // Fits on neither side: keep it on the screen, over the caret.
        work.bottom - height
    };

    let x = (caret.left - to_physical(offset_x))
        .min(work.right - width)
        .max(work.left);

    Placement {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::{place_at_caret, Placement};
    use windows::Win32::Foundation::RECT;

    const WORK: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    fn caret(left: i32, top: i32) -> RECT {
        RECT {
            left,
            top,
            right: left + 2,
            bottom: top + 20,
        }
    }

    #[test]
    fn window_goes_below_the_caret_in_scaled_size() {
        assert_eq!(
            place_at_caret(caret(500, 300), WORK, 1.5, 225.0, 200.0, 15.0),
            Placement {
                x: 477,
                y: 320,
                width: 338,
                height: 300,
            }
        );
    }

    #[test]
    fn window_flips_above_the_caret_near_the_bottom() {
        let placement = place_at_caret(caret(500, 900), WORK, 1.0, 225.0, 200.0, 15.0);
        assert_eq!(placement.y, 700);
    }

    #[test]
    fn window_stays_on_screen_when_it_fits_on_neither_side() {
        let work = RECT {
            bottom: 400,
            ..WORK
        };
        let placement = place_at_caret(caret(500, 150), work, 1.0, 225.0, 300.0, 15.0);
        assert_eq!(placement.y, 100);
    }

    #[test]
    fn wide_window_is_limited_to_the_work_area() {
        let placement = place_at_caret(caret(1800, 300), WORK, 2.0, 2000.0, 200.0, 15.0);
        assert_eq!(placement.width, 1920);
        assert_eq!(placement.x, 0);
    }

    #[test]
    fn window_is_kept_inside_the_left_and_right_edges() {
        assert_eq!(
            place_at_caret(caret(5, 300), WORK, 1.0, 225.0, 200.0, 15.0).x,
            0
        );
        assert_eq!(
            place_at_caret(caret(1900, 300), WORK, 1.0, 225.0, 200.0, 15.0).x,
            1695
        );
    }
}
