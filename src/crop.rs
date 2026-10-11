use std::time::{Duration, Instant};

pub const CROP_PADDING: u32 = 8;
pub const SHRINK_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRect {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn right(self) -> u32 {
        self.x + self.width
    }
    fn bottom(self) -> u32 {
        self.y + self.height
    }

    fn contains(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: self.right().max(other.right()) - x,
            height: self.bottom().max(other.bottom()) - y,
        }
    }
}

/// Window offsets are rounded to physical pixels; use the same offset for
/// composition and hit testing so changing the crop does not move the artwork.
#[derive(Clone, Copy, Debug)]
pub struct CropLayout {
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl CropLayout {
    pub fn new(rect: CropRect, scale: f32) -> Self {
        let offset_x = (rect.x as f64 * scale as f64).round() as i32;
        let offset_y = (rect.y as f64 * scale as f64).round() as i32;
        Self {
            offset_x,
            offset_y,
            width: ((rect.right() as f64 * scale as f64).ceil() as i64 - offset_x as i64)
                .clamp(1, 16_384) as u32,
            height: ((rect.bottom() as f64 * scale as f64).ceil() as i64 - offset_y as i64)
                .clamp(1, 16_384) as u32,
            scale,
        }
    }

    pub fn source_point(self, x: u32, y: u32) -> (f32, f32) {
        (
            (x as f32 + 0.5 + self.offset_x as f32) / self.scale - 0.5,
            (y as f32 + 0.5 + self.offset_y as f32) / self.scale - 0.5,
        )
    }
}

/// Detect every nonzero alpha, including faint shadows. The click threshold is
/// deliberately unrelated to the visibility threshold used for cropping.
pub fn visible_bounds(alpha: &[u8], width: u32, height: u32) -> Option<CropRect> {
    let count = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || alpha.len() < count {
        return None;
    }
    let mut left = width;
    let mut right = 0;
    let mut top = height;
    let mut bottom = 0;
    for (y, row) in alpha[..count].chunks_exact(width as usize).enumerate() {
        if let Some(first) = row.iter().position(|a| *a != 0) {
            let last = row.iter().rposition(|a| *a != 0).unwrap();
            left = left.min(first as u32);
            right = right.max(last as u32 + 1);
            top = top.min(y as u32);
            bottom = y as u32 + 1;
        }
    }
    if left == width {
        return None;
    }
    let x = left.saturating_sub(CROP_PADDING);
    let y = top.saturating_sub(CROP_PADDING);
    right = right.saturating_add(CROP_PADDING).min(width);
    bottom = bottom.saturating_add(CROP_PADDING).min(height);
    Some(CropRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

pub struct AutoCrop {
    rect: CropRect,
    shrink: Option<(CropRect, Instant)>,
}

impl AutoCrop {
    pub fn next_deadline(&self) -> Option<Instant> {
        self.shrink.map(|(_, deadline)| deadline)
    }
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            rect: CropRect::full(width, height),
            shrink: None,
        }
    }

    pub fn observe(&mut self, bounds: Option<CropRect>, now: Instant) {
        let Some(bounds) = bounds else {
            // Preserve the previous extent during blank animation frames. Keep
            // receiving so later content can expand the window again.
            self.shrink = None;
            return;
        };
        if !self.rect.contains(bounds) {
            self.rect = self.rect.union(bounds);
            self.shrink = None;
        } else if bounds == self.rect {
            self.shrink = None;
        } else if self.shrink.is_none_or(|(candidate, _)| candidate != bounds) {
            self.shrink = Some((bounds, now + SHRINK_DELAY));
        }
    }

    pub fn rect(&mut self, now: Instant) -> CropRect {
        if let Some((candidate, deadline)) = self.shrink {
            if now >= deadline {
                self.rect = candidate;
                self.shrink = None;
            }
        }
        self.rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_preserve_faint_pixels_and_clamp_padding() {
        let mut alpha = vec![0; 100 * 80];
        alpha[20 * 100 + 30] = 1;
        alpha[50 * 100 + 70] = 255;
        assert_eq!(
            visible_bounds(&alpha, 100, 80),
            Some(CropRect {
                x: 22,
                y: 12,
                width: 57,
                height: 47
            })
        );
        alpha[0] = 1;
        alpha[7999] = 1;
        assert_eq!(
            visible_bounds(&alpha, 100, 80),
            Some(CropRect::full(100, 80))
        );
        assert_eq!(visible_bounds(&[0; 4], 2, 2), None);
        assert_eq!(visible_bounds(&[255], 2, 2), None);
    }

    #[test]
    fn shrink_waits_but_growth_is_immediate() {
        let now = Instant::now();
        let mut crop = AutoCrop::new(100, 100);
        let small = CropRect {
            x: 20,
            y: 20,
            width: 30,
            height: 30,
        };
        crop.observe(Some(small), now);
        assert_eq!(crop.rect(now + SHRINK_DELAY / 2), CropRect::full(100, 100));
        // A static sender need not send another frame to finish the delay.
        assert_eq!(crop.rect(now + SHRINK_DELAY), small);
        let larger = CropRect {
            x: 10,
            y: 10,
            width: 70,
            height: 60,
        };
        crop.observe(Some(larger), now + SHRINK_DELAY);
        assert_eq!(crop.rect(now + SHRINK_DELAY), larger);
    }

    #[test]
    fn changed_or_empty_samples_cancel_pending_shrink() {
        let now = Instant::now();
        let mut crop = AutoCrop::new(100, 100);
        let a = CropRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        let b = CropRect { x: 11, ..a };
        crop.observe(Some(a), now);
        crop.observe(Some(b), now + Duration::from_millis(400));
        assert_eq!(crop.rect(now + SHRINK_DELAY), CropRect::full(100, 100));
        crop.observe(None, now + SHRINK_DELAY);
        assert_eq!(
            crop.rect(now + Duration::from_secs(2)),
            CropRect::full(100, 100)
        );
    }

    #[test]
    fn quantized_crop_offset_keeps_desktop_coordinates_stable() {
        let rect = CropRect {
            x: 13,
            y: 17,
            width: 21,
            height: 19,
        };
        for scale in [0.1, 1.0, 1.05, 2.0, 5.0] {
            let layout = CropLayout::new(rect, scale);
            let (sx, sy) = layout.source_point(0, 0);
            assert!(((sx + 0.5) * scale - layout.offset_x as f32 - 0.5).abs() < 0.0001);
            assert!(((sy + 0.5) * scale - layout.offset_y as f32 - 0.5).abs() < 0.0001);
            assert!(layout.width >= 1 && layout.height >= 1);
        }
    }
}
