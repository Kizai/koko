use koko_input::scroll::{WheelToPixels, pixels_to_wheel};

// Calculated conversion cases. No recorded device or protocol samples are used.
#[test]
fn protocol_pixels_scale_to_signed_wheel_units_on_both_axes() {
    assert_eq!(pixels_to_wheel((-12, 12)), (120, 120));
    assert_eq!(pixels_to_wheel((1, -1)), (-10, -10));
    assert_eq!(pixels_to_wheel((0, 0)), (0, 0));
}

#[test]
fn the_full_protocol_range_fits_in_signed_wheel_units() {
    assert_eq!(pixels_to_wheel((i16::MIN, i16::MAX)), (327_680, 327_670));
    assert_eq!(pixels_to_wheel((i16::MAX, i16::MIN)), (-327_670, -327_680));
}

#[test]
fn whole_wheel_steps_round_trip_on_both_axes() {
    let mut converter = WheelToPixels::default();
    assert_eq!(converter.convert((120, 120)), (-12, 12));
    assert_eq!(converter.convert((-120, -120)), (12, -12));
}

#[test]
fn fine_wheel_units_accumulate_independently() {
    let mut converter = WheelToPixels::default();
    assert_eq!(converter.convert((6, -4)), (0, 0));
    assert_eq!(converter.convert((4, -4)), (-1, 0));
    assert_eq!(converter.convert((0, -3)), (0, -1));
    assert_eq!(converter.convert((0, -9)), (0, -1));
}

#[test]
fn reversing_direction_offsets_the_pending_remainder() {
    let mut converter = WheelToPixels::default();
    assert_eq!(converter.convert((8, -8)), (0, 0));
    assert_eq!(converter.convert((-10, 10)), (0, 0));
    assert_eq!(converter.convert((-8, 8)), (1, 1));
}

#[test]
fn oversized_deltas_saturate_without_carrying_whole_pixel_overflow() {
    let mut converter = WheelToPixels::default();
    assert_eq!(
        converter.convert((i32::MIN, i32::MAX)),
        (i16::MAX, i16::MAX)
    );
    assert_eq!(converter.convert((-2, 3)), (1, 1));
    assert_eq!(
        converter.convert((i32::MAX, i32::MIN)),
        (i16::MIN, i16::MIN)
    );
    assert_eq!(converter.convert((0, 0)), (0, 0));
}
