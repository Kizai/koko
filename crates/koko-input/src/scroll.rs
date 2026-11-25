//! Integer conversion between Koko protocol pixels and Windows wheel units.

const WHEEL_UNITS_PER_PIXEL: i32 = 10;

/// Converts protocol `(dx, dy)` to Windows `(horizontal, vertical)` wheel units.
///
/// Koko maps 12 protocol pixels to one WHEEL_DELTA (120 units). Windows horizontal
/// wheel deltas reverse the protocol X sign under the adapter's direction convention.
pub fn pixels_to_wheel((dx, dy): (i16, i16)) -> (i32, i32) {
    (
        -i32::from(dx) * WHEEL_UNITS_PER_PIXEL,
        i32::from(dy) * WHEEL_UNITS_PER_PIXEL,
    )
}

/// Accumulates subpixel wheel deltas for one input stream.
///
/// Use a fresh default value at the start of a stream to clear both remainders.
#[derive(Debug, Default)]
pub struct WheelToPixels {
    remainder_x: i64,
    remainder_y: i64,
}

impl WheelToPixels {
    /// Converts Windows `(horizontal, vertical)` wheel units to protocol `(dx, dy)`.
    ///
    /// Each axis truncates toward zero and retains its signed subpixel remainder.
    /// Whole pixels outside the protocol's i16 range are discarded by saturation.
    pub fn convert(&mut self, (horizontal, vertical): (i32, i32)) -> (i16, i16) {
        (
            wheel_axis_to_pixels(-i64::from(horizontal), &mut self.remainder_x),
            wheel_axis_to_pixels(i64::from(vertical), &mut self.remainder_y),
        )
    }
}

fn wheel_axis_to_pixels(wheel_units: i64, remainder: &mut i64) -> i16 {
    let total = wheel_units + *remainder;
    let units_per_pixel = i64::from(WHEEL_UNITS_PER_PIXEL);
    *remainder = total % units_per_pixel;
    (total / units_per_pixel).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}
