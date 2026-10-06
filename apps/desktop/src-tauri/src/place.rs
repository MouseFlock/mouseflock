//! Where the Undo popup goes (design D5): the corner of the work area nearest
//! the tray icon, or bottom-right when Windows reports no icon rectangle.
//! All values are physical pixels.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    const fn center(self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// The popup's top-left corner.
pub fn popup_position(tray: Option<Rect>, work: Rect, size: (i32, i32), margin: i32) -> (i32, i32) {
    let (right, bottom) = match tray {
        Some(t) => {
            let (tx, ty) = t.center();
            let (wx, wy) = work.center();
            (tx >= wx, ty >= wy)
        }
        None => (true, true),
    };
    let x = if right {
        work.x + work.width - size.0 - margin
    } else {
        work.x + margin
    };
    let y = if bottom {
        work.y + work.height - size.1 - margin
    } else {
        work.y + margin
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (i32, i32) = (360, 96);

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn taskbar_at_each_edge() {
        // 1920×1080 screen, 48 px taskbar; the tray sits at the taskbar's end.
        let bottom = popup_position(
            Some(rect(1800, 1040, 24, 24)),
            rect(0, 0, 1920, 1032),
            SIZE,
            12,
        );
        assert_eq!(bottom, (1920 - 360 - 12, 1032 - 96 - 12));

        let top = popup_position(
            Some(rect(1800, 12, 24, 24)),
            rect(0, 48, 1920, 1032),
            SIZE,
            12,
        );
        assert_eq!(top, (1920 - 360 - 12, 48 + 12));

        let left = popup_position(
            Some(rect(12, 1000, 24, 24)),
            rect(48, 0, 1872, 1080),
            SIZE,
            12,
        );
        assert_eq!(left, (48 + 12, 1080 - 96 - 12));

        let right = popup_position(
            Some(rect(1884, 1000, 24, 24)),
            rect(0, 0, 1872, 1080),
            SIZE,
            12,
        );
        assert_eq!(right, (1872 - 360 - 12, 1080 - 96 - 12));
    }

    #[test]
    fn no_tray_rectangle_means_bottom_right() {
        let p = popup_position(None, rect(-1920, 0, 1920, 1032), SIZE, 12);
        assert_eq!(p, (-360 - 12, 1032 - 96 - 12));
    }
}
