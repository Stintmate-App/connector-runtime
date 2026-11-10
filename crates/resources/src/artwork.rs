//! Decoding a connector's own artwork. The tray icon is the artwork with a badge of the
//! connector's level in its lower right corner; the window icon is the artwork alone.

use crate::{ConnectorIcon, level_rgb};
use std::fmt;

/// Size of the tray icon, in pixels.
pub const TRAY_SIZE: u32 = 32;
/// Size of the window icon, in pixels.
pub const WINDOW_SIZE: u32 = 64;

/// Why the artwork could not be turned into pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkError {
    /// The size is neither [`TRAY_SIZE`] nor [`WINDOW_SIZE`].
    UnsupportedSize(u32),
    /// The bytes are not a PNG that decodes (the message is the decoder's).
    Decode(String),
    /// The PNG decodes but is not `size` x `size`, 8-bit RGBA.
    Format {
        /// The size asked for.
        size: u32,
        /// What the image is, e.g. `16x16 Rgb Eight`.
        found: String,
    },
}

impl fmt::Display for ArtworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSize(s) => write!(f, "the artwork has no {s} px size"),
            Self::Decode(e) => write!(f, "the artwork is not a PNG that decodes: {e}"),
            Self::Format { size, found } => {
                write!(f, "the artwork must be {size}x{size} RGBA8, it is {found}")
            }
        }
    }
}

impl std::error::Error for ArtworkError {}

impl ConnectorIcon {
    /// RGBA pixels (row by row) of the artwork, `size` pixels square: [`TRAY_SIZE`] or
    /// [`WINDOW_SIZE`]. The runtime logs an error and falls back to the glyph; a connector's
    /// tests should call this for both sizes so a bad file fails the build.
    pub fn rgba(&self, size: u32) -> Result<Vec<u8>, ArtworkError> {
        let bytes = match size {
            TRAY_SIZE => self.png_32,
            WINDOW_SIZE => self.png_64,
            other => return Err(ArtworkError::UnsupportedSize(other)),
        };
        let decode = |e: png::DecodingError| ArtworkError::Decode(e.to_string());
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .map_err(decode)?;
        let len = reader
            .output_buffer_size()
            .ok_or_else(|| ArtworkError::Decode("the image is too large".into()))?;
        let mut buf = vec![0; len];
        let info = reader.next_frame(&mut buf).map_err(decode)?;
        if info.width != size
            || info.height != size
            || info.color_type != png::ColorType::Rgba
            || info.bit_depth != png::BitDepth::Eight
        {
            return Err(ArtworkError::Format {
                size,
                found: format!(
                    "{}x{} {:?} {:?}",
                    info.width, info.height, info.color_type, info.bit_depth
                ),
            });
        }
        buf.truncate(info.buffer_size());
        Ok(buf)
    }

    /// The artwork with the badge of `level` in the lower right corner (a disc of the level's
    /// colour, see [`level_rgb`], inside a dark ring, anti-aliased); `size` as in
    /// [`ConnectorIcon::rgba`].
    pub fn badged_rgba(&self, level: &str, size: u32) -> Result<Vec<u8>, ArtworkError> {
        let mut px = self.rgba(size)?;
        let [r, g, b] = level_rgb(level);
        let s = size as f32;
        let (cx, cy) = (0.74 * s, 0.74 * s);
        let disc = 0.21 * s;
        let ring = disc + (0.06 * s).max(1.0);
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let ring_a = (ring - d + 0.5).clamp(0.0, 1.0);
                if ring_a == 0.0 {
                    continue;
                }
                let disc_a = (disc - d + 0.5).clamp(0.0, 1.0);
                let ring_rgb = [0x1b_u8, 0x1b, 0x1b];
                let mix = |i: usize, c: u8| {
                    (f32::from(ring_rgb[i]) * (1.0 - disc_a) + f32::from(c) * disc_a).round()
                };
                let badge = [mix(0, r), mix(1, g), mix(2, b)];
                let i = ((y * size + x) * 4) as usize;
                let under = f32::from(px[i + 3]) / 255.0;
                let out_a = ring_a + under * (1.0 - ring_a);
                for k in 0..3 {
                    let c =
                        (badge[k] * ring_a + f32::from(px[i + k]) * under * (1.0 - ring_a)) / out_a;
                    px[i + k] = c.round() as u8;
                }
                px[i + 3] = (out_a * 255.0).round() as u8;
            }
        }
        Ok(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> &'static [u8] {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        Box::leak(out.into_boxed_slice())
    }

    /// A solid, opaque square in the artwork's own colour.
    fn square(size: u32) -> &'static [u8] {
        let px: Vec<u8> = (0..size * size).flat_map(|_| [10, 20, 200, 255]).collect();
        encode(size, size, png::ColorType::Rgba, &px)
    }

    fn icon() -> ConnectorIcon {
        ConnectorIcon {
            png_32: square(32),
            png_64: square(64),
        }
    }

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> Vec<u8> {
        px[((y * size + x) * 4) as usize..][..4].to_vec()
    }

    #[test]
    fn decodes_both_sizes() {
        for size in [TRAY_SIZE, WINDOW_SIZE] {
            let px = icon().rgba(size).unwrap();
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(at(&px, size, 3, 3), [10, 20, 200, 255]);
        }
    }

    #[test]
    fn the_badge_shows_the_level_and_leaves_the_rest_alone() {
        for size in [TRAY_SIZE, WINDOW_SIZE] {
            let base = icon().rgba(size).unwrap();
            let c = (0.74 * size as f32) as u32;
            for level in ["off", "ok", "warn", "error"] {
                let px = icon().badged_rgba(level, size).unwrap();
                let centre = at(&px, size, c, c);
                assert_eq!(&centre[..3], &level_rgb(level), "{level}: badge centre");
                assert_eq!(centre[3], 255);
                assert_eq!(at(&px, size, 1, 1), at(&base, size, 1, 1), "far corner");
            }
        }
    }

    #[test]
    fn an_unknown_level_is_off_and_the_levels_differ() {
        let a = icon().badged_rgba("nonsense", 32).unwrap();
        assert_eq!(a, icon().badged_rgba("off", 32).unwrap());
        assert_ne!(a, icon().badged_rgba("ok", 32).unwrap());
        assert_ne!(
            icon().badged_rgba("warn", 32).unwrap(),
            icon().badged_rgba("error", 32).unwrap()
        );
    }

    #[test]
    fn other_sizes_are_refused() {
        assert_eq!(icon().rgba(48), Err(ArtworkError::UnsupportedSize(48)));
    }

    #[test]
    fn bad_images_are_errors_not_panics() {
        let junk = ConnectorIcon {
            png_32: b"not a png",
            png_64: b"",
        };
        assert!(matches!(junk.rgba(32), Err(ArtworkError::Decode(_))));
        assert!(matches!(
            junk.badged_rgba("ok", 64),
            Err(ArtworkError::Decode(_))
        ));
        let wrong = ConnectorIcon {
            png_32: square(16),
            png_64: encode(64, 64, png::ColorType::Rgb, &[0; 64 * 64 * 3]),
        };
        let e = wrong.rgba(32).unwrap_err();
        assert!(e.to_string().contains("32x32 RGBA8"), "{e}");
        assert!(matches!(wrong.rgba(64), Err(ArtworkError::Format { .. })));
    }
}
