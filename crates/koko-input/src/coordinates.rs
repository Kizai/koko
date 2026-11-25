//! Coordinate conversions for physical desktop pixels.
//!
//! Supply monitor and virtual desktop bounds in physical pixels. For mixed-DPI
//! desktops, obtain those bounds in a per-monitor DPI-aware context.

const PROTOCOL_SCALE: i64 = 10_000;
const PROTOCOL_MAX: i64 = PROTOCOL_SCALE - 1;

/// A half-open rectangle in physical desktop pixels, with a signed origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRect {
    // Widen before subtracting origins, scaling offsets, or doubling centers.
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl PhysicalRect {
    /// Returns `None` for zero extents or if the last pixel cannot fit in `i32`.
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Option<Self> {
        let (x, y) = (i64::from(x), i64::from(y));
        let (width, height) = (i64::from(width), i64::from(height));
        let max_pixel = i64::from(i32::MAX);
        if width == 0 || height == 0 || x + width - 1 > max_pixel || y + height - 1 > max_pixel {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Converts a protocol position in 0–9999 to a pixel on this monitor.
    ///
    /// The conversion uses the protocol denominator of 10,000 and rounds the
    /// pixel offset down. Returns `None` if either position is out of range.
    pub fn protocol_to_pixel(&self, (x, y): (u16, u16)) -> Option<(i32, i32)> {
        let (x, y) = (i64::from(x), i64::from(y));
        if x > PROTOCOL_MAX || y > PROTOCOL_MAX {
            return None;
        }
        Some((
            (self.x + x * self.width / PROTOCOL_SCALE) as i32,
            (self.y + y * self.height / PROTOCOL_SCALE) as i32,
        ))
    }

    /// Converts a pixel on this monitor to protocol coordinates, rounding down.
    ///
    /// Returns `None` for a pixel outside the monitor. Round trips can lose
    /// precision because both representations are quantized.
    pub fn pixel_to_protocol(&self, pixel: (i32, i32)) -> Option<(u16, u16)> {
        let (x, y) = self.pixel_offset(pixel)?;
        Some((
            protocol_position(x, self.width),
            protocol_position(y, self.height),
        ))
    }

    /// Converts a pixel to virtual-desktop absolute coordinates in 0–65535.
    ///
    /// Use the entire virtual desktop as this rectangle. The first and last
    /// pixel on each axis map to the endpoints, with intermediate values rounded
    /// to nearest (half up). A single-pixel axis maps to zero. Returns `None`
    /// for a pixel outside the rectangle.
    pub fn pixel_to_absolute(&self, pixel: (i32, i32)) -> Option<(u16, u16)> {
        let (x, y) = self.pixel_offset(pixel)?;
        let absolute = |offset: i64, extent: i64| {
            let last = extent - 1;
            ((offset * i64::from(u16::MAX) + last / 2) / last.max(1)) as u16
        };
        Some((absolute(x, self.width), absolute(y, self.height)))
    }

    fn pixel_offset(&self, (x, y): (i32, i32)) -> Option<(i64, i64)> {
        let x = i64::from(x) - self.x;
        let y = i64::from(y) - self.y;
        ((0..self.width).contains(&x) && (0..self.height).contains(&y)).then_some((x, y))
    }
}

/// UI tree bounds in protocol units, with positions in 0–9999 and sizes in 1–10,000.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NormalizedFrame {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Normalizes a UI element frame relative to the first monitor containing its center.
///
/// Returns the monitor index and normalized frame, or `None` when no monitor's
/// half-open bounds contain the center. Uses the full frame size even when the
/// frame crosses a monitor edge.
pub fn normalize_frame(
    frame: PhysicalRect,
    monitors: &[PhysicalRect],
) -> Option<(usize, NormalizedFrame)> {
    // Compare doubled coordinates to retain half-pixel centers exactly.
    let center_x = 2 * frame.x + frame.width;
    let center_y = 2 * frame.y + frame.height;
    let (index, monitor) = monitors.iter().enumerate().find(|(_, monitor)| {
        (2 * monitor.x..2 * (monitor.x + monitor.width)).contains(&center_x)
            && (2 * monitor.y..2 * (monitor.y + monitor.height)).contains(&center_y)
    })?;

    let size = |pixels: i64, extent: i64| {
        (pixels * PROTOCOL_SCALE / extent).clamp(1, PROTOCOL_SCALE) as u16
    };
    Some((
        index,
        NormalizedFrame {
            x: protocol_position(frame.x - monitor.x, monitor.width),
            y: protocol_position(frame.y - monitor.y, monitor.height),
            width: size(frame.width, monitor.width),
            height: size(frame.height, monitor.height),
        },
    ))
}

fn protocol_position(offset: i64, extent: i64) -> u16 {
    (offset * PROTOCOL_SCALE / extent).clamp(0, PROTOCOL_MAX) as u16
}
