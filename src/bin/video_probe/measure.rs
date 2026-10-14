//! What the probe measures that does not need a window: CPU time, the subsurface geometry and the
//! frame copy of the software way.

/// User plus system CPU time of this process, in clock ticks, read from a `/proc/<pid>/stat` line.
pub fn cpu_ticks(stat: &str) -> Option<u64> {
    // The command name is in parentheses and may hold anything, so the fields after it are read
    // from the last `)`: state is field 3, utime field 14 and stime field 15 (proc(5)).
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(user + system)
}

/// CPU load over an interval, in percent of one core (a fully busy core is 100).
pub fn cpu_percent(ticks: u64, seconds: f64, ticks_per_second: u64) -> f64 {
    ticks as f64 / ticks_per_second as f64 / seconds * 100.0
}

/// Where the video subsurface goes: its rectangle in the parent surface, in logical pixels, and the
/// size of its buffer, in physical pixels (a viewport maps one onto the other, so a fractional scale
/// still gets a sharp video).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub buffer_width: i32,
    pub buffer_height: i32,
}

/// The placement of a video drawn in `bounds` (logical pixels: origin and size) at `scale`.
pub fn placement(origin: (f32, f32), size: (f32, f32), scale: f32) -> Placement {
    Placement {
        x: origin.0.round() as i32,
        y: origin.1.round() as i32,
        width: (size.0.round() as i32).max(1),
        height: (size.1.round() as i32).max(1),
        buffer_width: ((size.0 * scale).round() as i32).max(1),
        buffer_height: ((size.1 * scale).round() as i32).max(1),
    }
}

/// Copies a `bgr0` frame rendered by mpv's software renderer into the tightly packed, opaque BGRA
/// buffer GPUI uploads: mpv leaves the fourth byte undefined, GPUI reads it as alpha.
pub fn opaque_bgra(src: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for line in src.chunks(stride).take(height) {
        for pixel in line[..width * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_ticks_adds_user_and_system_time() {
        let stat = "4242 (video_probe) S 1 4242 4242 0 -1 4194560 1 0 0 0 150 37 0 0 20 0 9 0";
        assert_eq!(cpu_ticks(stat), Some(187));
    }

    #[test]
    fn cpu_ticks_survives_a_command_name_with_spaces_and_parentheses() {
        let stat = "7 (a b) c)) R 1 7 7 0 -1 0 0 0 0 0 5 6 0 0 20 0 1 0";
        assert_eq!(cpu_ticks(stat), Some(11));
    }

    #[test]
    fn cpu_ticks_rejects_a_truncated_line() {
        assert_eq!(cpu_ticks("12 (x) S 1 2"), None);
    }

    #[test]
    fn cpu_percent_is_relative_to_one_core() {
        assert_eq!(cpu_percent(200, 2.0, 100), 100.0);
        assert_eq!(cpu_percent(50, 10.0, 100), 5.0);
    }

    #[test]
    fn placement_keeps_logical_position_and_scales_the_buffer() {
        let p = placement((10.4, 48.0), (1280.0, 720.0), 1.25);
        assert_eq!(
            p,
            Placement {
                x: 10,
                y: 48,
                width: 1280,
                height: 720,
                buffer_width: 1600,
                buffer_height: 900,
            }
        );
    }

    #[test]
    fn placement_never_makes_an_empty_buffer() {
        let p = placement((0.0, 0.0), (0.0, 0.2), 1.0);
        assert_eq!((p.width, p.height), (1, 1));
        assert_eq!((p.buffer_width, p.buffer_height), (1, 1));
    }

    #[test]
    fn opaque_bgra_drops_the_padding_and_sets_alpha() {
        // Two pixels per line, stride of three pixels, two lines.
        let src = [
            1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
            7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9,
        ];
        assert_eq!(
            opaque_bgra(&src, 12, 2, 2),
            vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }
}
