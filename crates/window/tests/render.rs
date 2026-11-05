//! Offscreen renders of the three pages: egui lays each page out on a headless
//! context and a small CPU rasteriser draws the tessellated meshes into a PNG under
//! `target/tmp/`, so the layout can be looked at without a window, a GPU or a screenshot.

use eframe::egui::{self, Color32, epaint};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use stintmate_connector_control::api::Snapshot;
use stintmate_connector_control::texts::Texts;
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_testing::EXAMPLE_SPEC;
use stintmate_connector_window::gui::{Page, Window};
use stintmate_connector_window::theme;

/// The example connector with its UI catalog.
static SPEC: ConnectorSpec = ConnectorSpec {
    ui_catalog: include_str!("fixtures/en.example.json"),
    ..EXAMPLE_SPEC
};

const W: f32 = 680.0;
const H: f32 = 460.0;
const PPP: f32 = 2.0;

struct Tex {
    size: [usize; 2],
    px: Vec<Color32>,
}

fn apply_delta(
    textures: &mut HashMap<egui::TextureId, Tex>,
    id: egui::TextureId,
    d: &epaint::ImageDelta,
) {
    let epaint::ImageData::Color(img) = &d.image;
    match d.pos {
        None => {
            textures.insert(
                id,
                Tex {
                    size: img.size,
                    px: img.pixels.clone(),
                },
            );
        }
        Some([x0, y0]) => {
            let t = textures.get_mut(&id).expect("patch of an unknown texture");
            for y in 0..img.size[1] {
                for x in 0..img.size[0] {
                    t.px[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                }
            }
        }
    }
}

fn sample(t: &Tex, u: f32, v: f32) -> [f32; 4] {
    let x = (u * t.size[0] as f32 - 0.5).clamp(0.0, t.size[0] as f32 - 1.0);
    let y = (v * t.size[1] as f32 - 0.5).clamp(0.0, t.size[1] as f32 - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(t.size[0] - 1), (y0 + 1).min(t.size[1] - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| t.px[y * t.size[0] + x].to_array().map(f32::from);
    let mut out = [0.0; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0)[i] * (1.0 - fx) + at(x1, y0)[i] * fx;
        let bottom = at(x0, y1)[i] * (1.0 - fx) + at(x1, y1)[i] * fx;
        *o = top * (1.0 - fy) + bottom * fy;
    }
    out
}

#[allow(clippy::chunks_exact_to_as_chunks)]
/// Draws premultiplied-alpha meshes the way egui's shader does (gamma space blending).
fn rasterise(
    prims: &[epaint::ClippedPrimitive],
    textures: &HashMap<egui::TextureId, Tex>,
    w: usize,
    h: usize,
) -> Vec<[f32; 3]> {
    let mut fb = vec![[0.0f32; 3]; w * h];
    for prim in prims {
        let epaint::Primitive::Mesh(mesh) = &prim.primitive else {
            continue;
        };
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        };
        let clip = prim.clip_rect;
        let (cx0, cy0) = (
            (clip.min.x * PPP).max(0.0) as usize,
            (clip.min.y * PPP).max(0.0) as usize,
        );
        let (cx1, cy1) = (
            ((clip.max.x * PPP).ceil() as usize).min(w),
            ((clip.max.y * PPP).ceil() as usize).min(h),
        );
        for tri in mesh.indices.chunks_exact(3) {
            let v: Vec<&epaint::Vertex> = tri.iter().map(|i| &mesh.vertices[*i as usize]).collect();
            let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * PPP, v.pos.y * PPP)).collect();
            let area =
                (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = (p
                .iter()
                .map(|q| q.0)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(cx0 as f32)) as usize;
            let maxx = (p
                .iter()
                .map(|q| q.0)
                .fold(f32::MIN, f32::max)
                .ceil()
                .min(cx1 as f32)) as usize;
            let miny = (p
                .iter()
                .map(|q| q.1)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(cy0 as f32)) as usize;
            let maxy = (p
                .iter()
                .map(|q| q.1)
                .fold(f32::MIN, f32::max)
                .ceil()
                .min(cy1 as f32)) as usize;
            for y in miny..maxy {
                for x in minx..maxx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                    let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = sample(tex, u, vv);
                    let col: Vec<f32> = (0..4)
                        .map(|i| {
                            let c = |k: usize| f32::from(v[k].color.to_array()[i]);
                            w0 * c(0) + w1 * c(1) + w2 * c(2)
                        })
                        .collect();
                    let a = t[3] / 255.0 * col[3] / 255.0;
                    let dst = &mut fb[y * w + x];
                    for k in 0..3 {
                        let src = t[k] / 255.0 * col[k] / 255.0;
                        dst[k] = src + dst[k] * (1.0 - a);
                    }
                }
            }
        }
    }
    fb
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                0xedb8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend(data);
    out.extend(&body);
    out.extend(crc32(&body).to_be_bytes());
}

/// An RGB PNG with stored (uncompressed) deflate blocks: no dependency for a test image.
fn png(w: usize, h: usize, fb: &[[f32; 3]]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for row in fb.chunks(w) {
        raw.push(0);
        for px in row {
            raw.extend(px.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend(*b);
    }
    let (mut a, mut s) = (1u32, 0u32);
    for &b in &raw {
        a = (a + u32::from(b)) % 65521;
        s = (s + a) % 65521;
    }
    z.extend(((s << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = (w as u32).to_be_bytes().to_vec();
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn snapshot(data_dir: &Path, managed: bool) -> Snapshot {
    Snapshot(json!({
        "services": [{
            "id": "recorder", "name": "Recorder", "state": "waiting",
            "label": "Waiting for Example Simulator · Connected to Pitwall",
            "level": "warn", "running": true, "pid": 41416, "default": "on",
        }],
        "level": "warn", "tooltip": "", "managed": managed, "parent_pid": null,
        "window": "shown", "window_request": {"seq": 0, "action": null},
        "exiting": false, "autostart": false,
        "config": {
            "start_minimized": false, "recorder_hz": 50.0, "logs_default": "recorder",
            "start_with_app": {"recorder": true},
        },
        "data_dir": data_dir.display().to_string(),
        "game_running": false, "pitwall": "connected",
        "outbox": {"pending": 0, "rejected": 0},
    }))
}

const LOG: &str = "\
2026-10-25 01:10:41,102 INFO example_recorder: Recorder started (PID 41416)
2026-10-25 01:10:41,310 INFO example_recorder::link: Connected to Pitwall 0.1.0
2026-10-25 01:10:41,311 INFO example_recorder::link: Warning catalog declared: 28 codes
2026-10-25 01:10:41,640 INFO example_recorder::link: Guide declared: 2 · actions: 2
2026-10-25 01:11:19,008 WARNING example_recorder::link: Connection to Pitwall lost; reconnecting
2026-10-25 01:11:19,500 ERROR example_recorder: Shared memory read failed: access denied
2026-10-25 01:11:19,900 INFO example_recorder: Stopping (stop requested)
2026-10-25 01:11:25,214 INFO example_recorder: Recorder started (PID 27708)
2026-10-25 01:11:26,020 INFO example_recorder::link: Connected to Pitwall 0.1.0
2026-10-25 01:11:26,021 INFO example_recorder::link: Warning catalog declared: 28 codes
";

fn render(page: Page, managed: bool, name: &str, dir: &Path) -> PathBuf {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    let mut window = Window::preview(Texts::english(&SPEC), page, "recorder", 0.1);
    let snap = snapshot(dir, managed);
    let mut textures: HashMap<egui::TextureId, Tex> = HashMap::new();
    let mut last = None;
    for frame in 0..4 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W, H),
            )),
            time: Some(f64::from(frame) * 0.1),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(PPP),
                    ..Default::default()
                },
            ))
            .collect(),
            ..Default::default()
        };
        ctx.set_pixels_per_point(PPP);
        let mut out = ctx.run_ui(input, |ui| window.show(ui, &snap));
        for (id, ds) in &out.textures_delta.set {
            for d in ds {
                apply_delta(&mut textures, *id, d);
            }
        }
        out.textures_delta.clear();
        last = Some(out);
    }
    let out = last.unwrap();
    let prims = ctx.tessellate(out.shapes, PPP);
    let (w, h) = ((W * PPP) as usize, (H * PPP) as usize);
    let mut fb = rasterise(&prims, &textures, w, h);
    // Anything the window did not paint is the clear colour.
    let bg = theme::BG.to_array().map(|c| f32::from(c) / 255.0);
    for px in &mut fb {
        if *px == [0.0; 3] {
            *px = [bg[0], bg[1], bg[2]];
        }
    }
    let path = renders_dir().join(format!("connector-window-{name}.png"));
    std::fs::write(&path, png(w, h, &fb)).unwrap();
    // The page must have painted its accent (a switch) or its text, not only the background.
    let distinct: std::collections::HashSet<[u8; 3]> = fb
        .iter()
        .map(|p| p.map(|c| (c * 255.0).round() as u8))
        .collect();
    assert!(distinct.len() > 12, "{name}: the render looks empty");
    path
}

#[test]
fn the_three_pages_render() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("recorder.log"), LOG).unwrap();
    eprintln!("renders in {}", renders_dir().display());
    for (page, managed, name) in [
        (Page::Services, true, "services"),
        (Page::Options, true, "options"),
        (Page::Logs, true, "logs"),
        (Page::Services, false, "services-unmanaged"),
        (Page::Options, false, "options-unmanaged"),
    ] {
        let path = render(page, managed, name, dir.path());
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        eprintln!("render: {}", path.display());
    }
}

/// Where the PNGs go: `target/tmp/window-renders`, created on first use.
fn renders_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("window-renders");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
