//! The connector's icons and Windows resource scripts, drawn and written in code (no image
//! files): a glyph on a disc whose colour is the level of the connector (off = grey, ok =
//! green, warn = amber, error = red). They are our own artwork; nothing is copied from a third
//! party.
//!
//! Pure standard library, so a connector's `build.rs` can use it as a build dependency to
//! write the executables' `.ico` and `.rc` files ([`ico_bytes`], [`version_rc`]) before it
//! compiles them.
#![deny(missing_docs)]

use std::path::Path;

/// One line of a glyph, in the unit square: from point to point.
pub type Stroke = ((f32, f32), (f32, f32));

/// The mark drawn on the disc: a few strokes in the unit square (`0.0..=1.0`, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph(pub &'static [Stroke]);

impl Glyph {
    /// A sans-serif capital A.
    pub const A: Glyph = Glyph(&[
        ((0.5, 0.2), (0.29, 0.8)),
        ((0.5, 0.2), (0.71, 0.8)),
        ((0.375, 0.6), (0.625, 0.6)),
    ]);
    /// A serif capital I.
    pub const I: Glyph = Glyph(&[
        ((0.5, 0.25), (0.5, 0.75)),
        ((0.36, 0.25), (0.64, 0.25)),
        ((0.36, 0.75), (0.64, 0.75)),
    ]);
}

/// The colour of a level word (`off`, `ok`, `warn`, `error`; anything else is `off`).
pub fn level_rgb(level: &str) -> [u8; 3] {
    match level {
        "ok" => [0x2e, 0x9d, 0x48],
        "warn" => [0xe0, 0xa1, 0x00],
        "error" => [0xd1, 0x34, 0x38],
        _ => [0x8a, 0x8a, 0x8a],
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * abx, a.1 + t * aby);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Whether a point of the unit square is on the glyph, and whether it is on the disc.
fn shape_at(glyph: Glyph, p: (f32, f32)) -> (bool, bool) {
    let (dx, dy) = (p.0 - 0.5, p.1 - 0.5);
    let disc = (dx * dx + dy * dy).sqrt() <= 0.47;
    let half = 0.07;
    let on_glyph = glyph
        .0
        .iter()
        .any(|&(a, b)| segment_distance(p, a, b) <= half);
    (on_glyph, disc)
}

/// RGBA pixels (row by row) of the icon of `level`, `size` pixels square, 4x4 supersampled.
pub fn render_rgba(glyph: Glyph, level: &str, size: u32) -> Vec<u8> {
    let [r, g, b] = level_rgb(level);
    let n = 4u32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut disc_sum, mut glyph_sum) = (0.0f32, 0.0f32);
            for sy in 0..n {
                for sx in 0..n {
                    let p = (
                        (x as f32 + (sx as f32 + 0.5) / n as f32) / size as f32,
                        (y as f32 + (sy as f32 + 0.5) / n as f32) / size as f32,
                    );
                    let (on_glyph, disc) = shape_at(glyph, p);
                    if disc {
                        disc_sum += 1.0;
                        if on_glyph {
                            glyph_sum += 1.0;
                        }
                    }
                }
            }
            let alpha = disc_sum / (n * n) as f32;
            let white = if disc_sum > 0.0 {
                glyph_sum / disc_sum
            } else {
                0.0
            };
            let mix = |c: u8| (f32::from(c) * (1.0 - white) + 255.0 * white).round() as u8;
            out.extend_from_slice(&[mix(r), mix(g), mix(b), (alpha * 255.0).round() as u8]);
        }
    }
    out
}

/// A Windows `.ico` file with 32-bit images of `sizes` (each at most 255).
pub fn ico_bytes(glyph: Glyph, level: &str, sizes: &[u32]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend_from_slice(&[0, 0, 1, 0]);
    file.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut images: Vec<Vec<u8>> = Vec::new();
    for &s in sizes {
        let rgba = render_rgba(glyph, level, s);
        let mut dib = Vec::new();
        // BITMAPINFOHEADER: the height counts the XOR and the AND masks.
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(s as i32).to_le_bytes());
        dib.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        let row = (s.div_ceil(32) * 4) as usize;
        dib.extend(std::iter::repeat_n(0u8, row * s as usize));
        images.push(dib);
    }
    let mut offset = 6 + 16 * sizes.len();
    for (&s, img) in sizes.iter().zip(&images) {
        file.extend_from_slice(&[s as u8, s as u8, 0, 0, 1, 0, 32, 0]);
        file.extend_from_slice(&(img.len() as u32).to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        file.extend_from_slice(&img);
    }
    file
}

/// What the version information of an executable says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo<'a> {
    /// `FileDescription`, e.g. "Example connector recorder".
    pub description: &'a str,
    /// `InternalName`, e.g. "example-recorder".
    pub internal_name: &'a str,
    /// `OriginalFilename`, e.g. "example-recorder.exe".
    pub original_filename: &'a str,
    /// `ProductName`, e.g. "Stintmate Example connector".
    pub product_name: &'a str,
    /// The version text, e.g. `1.2.3` or `0.1.0-dev`; the numeric `FILEVERSION` is made from
    /// its first three digit groups.
    pub version: &'a str,
}

/// A string for a resource script: backslashes escaped, quotes doubled.
fn rc_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\"\"")
}

/// The resource script of an executable: its icon (`ico_path`, written by [`ico_bytes`]) and
/// its version information. The caller writes it to a file and compiles it (for example with
/// `embed_resource`).
pub fn version_rc(info: &FileInfo<'_>, ico_path: &Path) -> String {
    let mut n = info
        .version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse::<u16>().ok())
        .chain(std::iter::repeat(0));
    let (a, b, c) = (
        n.next().unwrap_or(0),
        n.next().unwrap_or(0),
        n.next().unwrap_or(0),
    );
    let version = rc_escape(info.version);
    let ico = rc_escape(&ico_path.display().to_string().replace('\\', "/"));
    format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Stintmate\0"
      VALUE "FileDescription", "{description}\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "{internal}\0"
      VALUE "LegalCopyright", "Apache-2.0\0"
      VALUE "OriginalFilename", "{original}\0"
      VALUE "ProductName", "{product}\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#,
        description = rc_escape(info.description),
        internal = rc_escape(info.internal_name),
        original = rc_escape(info.original_filename),
        product = rc_escape(info.product_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_disc_with_a_white_glyph() {
        let px = render_rgba(Glyph::A, "ok", 32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "outside the disc");
        assert_eq!(&at(5, 16)[..3], &level_rgb("ok"), "on the disc");
        assert_eq!(at(5, 16)[3], 255);
        assert_eq!(&at(16, 19)[..3], &[255, 255, 255], "the crossbar of the A");
    }

    #[test]
    fn the_serif_i_has_its_stem_in_the_middle() {
        let px = render_rgba(Glyph::I, "ok", 32);
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(&at(16, 16)[..3], &[255, 255, 255], "the stem");
        assert_eq!(&at(8, 16)[..3], &level_rgb("ok"), "beside the stem");
        assert_ne!(
            render_rgba(Glyph::A, "ok", 32),
            px,
            "the glyphs draw differently"
        );
    }

    #[test]
    fn the_levels_have_different_colours() {
        let all: Vec<_> = ["off", "ok", "warn", "error"].map(level_rgb).to_vec();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(level_rgb("nonsense"), level_rgb("off"));
    }

    #[test]
    fn the_ico_has_a_directory_and_its_images() {
        let ico = ico_bytes(Glyph::A, "ok", &[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        let size = u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize;
        let off = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
        assert_eq!(off, 6 + 32);
        assert_eq!(size, 40 + 16 * 16 * 4 + 4 * 16);
    }

    fn info(version: &str) -> FileInfo<'_> {
        FileInfo {
            description: "Example connector recorder",
            internal_name: "example-recorder",
            original_filename: "example-recorder.exe",
            product_name: "Stintmate Example connector",
            version,
        }
    }

    #[test]
    fn version_rc_parses_the_numeric_version() {
        let rc = version_rc(&info("1.2.3"), Path::new(r"C:\out\app.ico"));
        assert!(rc.contains("FILEVERSION 1,2,3,0"));
        assert!(rc.contains("PRODUCTVERSION 1,2,3,0"));
        assert!(rc.contains(r#"VALUE "FileVersion", "1.2.3\0""#));
        assert!(rc.starts_with("1 ICON \"C:/out/app.ico\"\n"));
        assert!(rc.contains(r#"VALUE "InternalName", "example-recorder\0""#));
    }

    #[test]
    fn version_rc_ignores_a_pre_release_suffix() {
        let rc = version_rc(&info("0.1.0-dev"), Path::new("app.ico"));
        assert!(rc.contains("FILEVERSION 0,1,0,0"));
        assert!(rc.contains(r#"VALUE "ProductVersion", "0.1.0-dev\0""#));
        let short = version_rc(&info("7"), Path::new("app.ico"));
        assert!(short.contains("FILEVERSION 7,0,0,0"));
    }

    #[test]
    fn version_rc_escapes_quotes_and_backslashes() {
        let mut i = info("1.0.0");
        i.description = r#"The "Example" \ connector"#;
        let rc = version_rc(&i, Path::new("app.ico"));
        assert!(rc.contains(r#"VALUE "FileDescription", "The ""Example"" \\ connector\0""#));
    }
}
