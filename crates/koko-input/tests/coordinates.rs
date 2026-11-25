use koko_input::coordinates::{NormalizedFrame, PhysicalRect, normalize_frame};

// Calculated geometry cases. No recorded device or protocol samples are used.
#[test]
fn protocol_coordinates_use_the_selected_monitors_physical_extent() {
    let primary_at_100_percent = PhysicalRect::new(0, 0, 1920, 1080).unwrap();
    // A 2560 × 1440 display at 150% scaling still has 2560 × 1440 physical pixels.
    let left_at_150_percent = PhysicalRect::new(-2560, 0, 2560, 1440).unwrap();
    let above_at_150_percent = PhysicalRect::new(0, -1440, 2560, 1440).unwrap();

    for (monitor, center) in [
        (primary_at_100_percent, (960, 540)),
        (left_at_150_percent, (-1280, 720)),
        (above_at_150_percent, (1280, -720)),
    ] {
        assert_eq!(monitor.protocol_to_pixel((5000, 5000)), Some(center));
        assert_eq!(monitor.pixel_to_protocol(center), Some((5000, 5000)));
    }

    assert_eq!(
        left_at_150_percent.protocol_to_pixel((9999, 9999)),
        Some((-1, 1439))
    );
    assert_eq!(
        left_at_150_percent.pixel_to_protocol((-1, 1439)),
        Some((9996, 9993))
    );
    assert_eq!(left_at_150_percent.protocol_to_pixel((10000, 0)), None);
    assert_eq!(left_at_150_percent.pixel_to_protocol((0, 0)), None);
}

#[test]
fn absolute_coordinates_span_the_entire_virtual_desktop() {
    let desktop = PhysicalRect::new(-2560, -1440, 5121, 2881).unwrap();
    for (pixel, absolute) in [
        ((-2560, -1440), (0, 0)),
        ((2560, -1440), (65535, 0)),
        ((-2560, 1440), (0, 65535)),
        ((2560, 1440), (65535, 65535)),
        ((-1280, 720), (16384, 49151)),
        ((1280, -720), (49151, 16384)),
        ((0, 0), (32768, 32768)),
    ] {
        assert_eq!(desktop.pixel_to_absolute(pixel), Some(absolute));
    }
    assert_eq!(desktop.pixel_to_absolute((2561, 1440)), None);
    let single_pixel = PhysicalRect::new(-3, 7, 1, 1).unwrap();
    assert_eq!(single_pixel.pixel_to_absolute((-3, 7)), Some((0, 0)));
}

#[test]
fn crossing_frame_uses_its_center_and_clamps_position() {
    let primary = PhysicalRect::new(0, 0, 1920, 1080).unwrap();
    let left = PhysicalRect::new(-2560, 0, 2560, 1440).unwrap();
    let above = PhysicalRect::new(0, -1440, 2560, 1440).unwrap();
    let displays = [left, above, primary];

    let frame = PhysicalRect::new(-30, 180, 100, 108).unwrap();
    assert_eq!(
        normalize_frame(frame, &displays),
        Some((
            2,
            NormalizedFrame {
                x: 0,
                y: 1666,
                width: 520,
                height: 1000
            }
        ))
    );

    let frame = PhysicalRect::new(640, -120, 256, 200).unwrap();
    assert_eq!(
        normalize_frame(frame, &displays),
        Some((
            1,
            NormalizedFrame {
                x: 2500,
                y: 9166,
                width: 1000,
                height: 1388
            }
        ))
    );
}

#[test]
fn frames_use_half_open_monitor_edges_and_order() {
    let left = PhysicalRect::new(-10, 0, 10, 10).unwrap();
    let right = PhysicalRect::new(0, 0, 10, 10).unwrap();
    let overlap = PhysicalRect::new(-3, 0, 10, 10).unwrap();
    let frame = PhysicalRect::new(-1, 0, 2, 1).unwrap();

    assert_eq!(normalize_frame(frame, &[left, right]).unwrap().0, 1);
    assert_eq!(
        normalize_frame(frame, &[left, right, overlap]).unwrap().1.x,
        0
    );
    assert_eq!(
        normalize_frame(frame, &[left, overlap, right]).unwrap().1.x,
        2000
    );
    let frame_in_gap = PhysicalRect::new(100, 0, 1, 1).unwrap();
    assert_eq!(normalize_frame(frame_in_gap, &[left, right]), None);
    assert_eq!(normalize_frame(frame, &[]), None);
}

#[test]
fn normalized_frame_dimensions_are_bounded() {
    let display = PhysicalRect::new(0, 0, 20_000, 20_000).unwrap();
    let frame = PhysicalRect::new(-5_000, 19_999, 30_000, 1).unwrap();
    assert_eq!(
        normalize_frame(frame, &[display]),
        Some((
            0,
            NormalizedFrame {
                x: 0,
                y: 9999,
                width: 10000,
                height: 1
            }
        ))
    );
}

#[test]
fn physical_rectangles_validate_extent_and_handle_integer_limits() {
    assert_eq!(PhysicalRect::new(0, 0, 0, 10), None);
    assert_eq!(PhysicalRect::new(0, 0, 10, 0), None);
    assert_eq!(PhysicalRect::new(i32::MAX, 0, 2, 1), None);
    assert_eq!(PhysicalRect::new(0, i32::MAX, 1, 2), None);
    let wide = PhysicalRect::new(i32::MIN, i32::MAX, u32::MAX, 1).unwrap();
    assert_eq!(
        wide.pixel_to_absolute((i32::MAX - 1, i32::MAX)),
        Some((65535, 0))
    );
    assert_eq!(
        wide.pixel_to_protocol((i32::MAX - 1, i32::MAX)),
        Some((9999, 0))
    );
}
