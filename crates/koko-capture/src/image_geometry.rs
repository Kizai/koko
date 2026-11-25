pub(super) fn scaled_size(width: u32, height: u32, edge: u16) -> (u32, u32) {
    let longest = width.max(height);
    let edge = u32::from(edge);
    if longest <= edge {
        return (width, height);
    }
    let scale = |value| {
        let numerator = u64::from(value) * u64::from(edge) + u64::from(longest / 2);
        ((numerator / u64::from(longest)) as u32).max(1)
    };
    (scale(width), scale(height))
}

pub(super) fn display_id(path: &str) -> u32 {
    // FNV-1a is specified here so toolchain upgrades cannot change the wire ID.
    path.bytes()
        .fold(2_166_136_261_u32, |hash, byte| {
            (hash ^ u32::from(byte.to_ascii_lowercase())).wrapping_mul(16_777_619)
        })
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_edge_scales_without_upscaling_or_losing_the_short_edge() {
        assert_eq!(scaled_size(3840, 2160, 1600), (1600, 900));
        assert_eq!(scaled_size(1080, 1920, 1280), (720, 1280));
        assert_eq!(scaled_size(640, 480, 1600), (640, 480));
        assert_eq!(scaled_size(1, 1000, 16), (1, 16));
    }

    #[test]
    fn display_id_is_case_insensitive_and_stable() {
        assert_eq!(display_id("MONITOR"), display_id("monitor"));
        assert_eq!(display_id("monitor"), 0x763e0219);
        assert_ne!(display_id("monitor"), display_id("monitor2"));
    }
}
