use crate::effects::{self, BgMotion, ExportFormat, Poly, RenderCfg, VizKind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use base64::{engine::general_purpose::STANDARD, Engine};
use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::window;
use dioxus::html::geometry::WheelDelta;
use dioxus::html::HasFileData;
use dioxus::html::Key;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use image::RgbaImage;
use rand::{rngs::StdRng, Rng, SeedableRng};

static CSS: &str = include_str!("../assets/main.css");

/// Timeline lane width in px — fixed so click-to-seek maps without JS measuring.
const LANE_W: f64 = 560.0;
/// Preview is rendered at most this wide; polygons are stored full-res.
const PREVIEW_W: f32 = 820.0;
const TICK_MS: u64 = 66; // ~15 fps preview

/// Fixed shortcut cheat-sheet: (group, [(keys, action)]).
/// " / " separates alternate combos, "+" separates keys within one combo.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Files & Export",
        &[
            ("Ctrl+O", "Open image"),
            ("Ctrl+B", "Load background image"),
            ("Ctrl+A", "Load audio (lip-sync)"),
            ("Ctrl+E", "Export video (.webm)"),
            ("Ctrl+Shift+E", "Export PNG sequence"),
            ("Ctrl+S", "Export current frame"),
        ],
    ),
    (
        "Playback & Layers",
        &[
            ("Space", "Play / Pause"),
            ("Home", "Rewind to start"),
            ("1 – 5", "Select layer"),
        ],
    ),
    (
        "Tracing",
        &[
            ("Enter", "Finish eye"),
            ("Esc", "Cancel pending trace"),
            ("Delete", "Remove last point"),
            ("Right-click", "Delete nearest point"),
            ("Drag", "Move a point"),
        ],
    ),
    (
        "View",
        &[
            ("Ctrl+Scroll", "Zoom at cursor"),
            ("Middle-drag", "Pan"),
            ("Ctrl+= / Ctrl+-", "Zoom in / out"),
            ("Ctrl+0", "Reset zoom"),
            ("F1", "Keyboard shortcuts"),
        ],
    ),
];

/// Which polygon vertex is being dragged.
#[derive(Clone, Copy, PartialEq)]
enum DragTarget {
    CurEye(usize),
    Eye(usize, usize),
    Mouth(usize),
}

fn clamp_pt(p: (f32, f32), dims: Option<(f32, f32)>) -> (f32, f32) {
    match dims {
        Some((w, h)) => (p.0.clamp(0.0, w - 1.0), p.1.clamp(0.0, h - 1.0)),
        None => p,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Bg,
    BgImg,
    Blink,
    Talk,
    Viz,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Layer::Bg => "BG Removal",
            Layer::BgImg => "BG Image",
            Layer::Blink => "Blink",
            Layer::Talk => "Talk",
            Layer::Viz => "Visualizer",
        }
    }
}

fn quit() {
    std::process::exit(0);
}

/// Human meaning of a VP9 CRF value (lower = better).
fn crf_label(crf: u32) -> &'static str {
    match crf {
        0..=15 => "Near-lossless — very large file",
        16..=25 => "High quality — large file",
        26..=34 => "Balanced — recommended",
        35..=44 => "Compact — visible compression",
        _ => "Smallest — rough",
    }
}

/// Await a blocking export while mirroring its atomic frame counter into the
/// progress signal (signals aren't Send, so the worker can't set it directly).
async fn track_progress<T>(
    mut handle: tokio::task::JoinHandle<T>,
    counter: Arc<AtomicU32>,
    total: u32,
    mut progress: Signal<Option<(u32, u32)>>,
) -> Result<T, tokio::task::JoinError> {
    progress.set(Some((0, total)));
    let res = loop {
        match tokio::time::timeout(std::time::Duration::from_millis(120), &mut handle).await {
            Ok(r) => break r,
            Err(_) => progress.set(Some((counter.load(Ordering::Relaxed), total))),
        }
    };
    progress.set(None);
    res
}

fn png_data_uri(img: &RgbaImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut buf, image::ImageFormat::Png);
    format!("data:image/png;base64,{}", STANDARD.encode(buf.into_inner()))
}

fn hex_rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        [0, 2, 4].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0))
    } else {
        [0, 0, 0]
    }
}

fn pts_attr(poly: &Poly, s: f32) -> String {
    poly.iter()
        .map(|(x, y)| format!("{:.1},{:.1}", x * s, y * s))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn App() -> Element {
    // ---------- state ----------
    let mut base = use_signal(|| None::<RgbaImage>);
    let mut img_name = use_signal(String::new);
    let mut status = use_signal(|| "Open an image to begin (File → Open Image…)".to_string());
    let mut time = use_signal(|| 0.0f32);
    let mut playing = use_signal(|| false);
    let mut duration = use_signal(|| 6.0f32);
    let mut fps = use_signal(|| 25.0f32);
    let mut selected = use_signal(|| Layer::Blink);

    let mut bg_on = use_signal(|| false);
    let mut tolerance = use_signal(|| 30.0f32);

    let mut bgimg_on = use_signal(|| false);
    let mut bg_img = use_signal(|| None::<RgbaImage>);
    let mut bg_name = use_signal(String::new);

    let mut blink_on = use_signal(|| true);
    let mut eye_polys = use_signal(Vec::<Poly>::new);
    let mut cur_eye = use_signal(Poly::new);
    let mut blink_period = use_signal(|| 3.0f32);
    let mut blink_random = use_signal(|| false);
    let mut min_gap = use_signal(|| 2.0f32);
    let mut max_gap = use_signal(|| 5.0f32);
    let mut blink_seed = use_signal(|| 1u64);

    // preview navigation (ctrl+wheel zoom, middle-drag pan) + vertex dragging
    let mut zoom = use_signal(|| 1.0f64);
    let mut pan = use_signal(|| (0.0f64, 0.0f64));
    let mut pan_drag = use_signal(|| None::<(f64, f64)>);
    let mut vertex_drag = use_signal(|| None::<DragTarget>);
    let mut shortcuts_open = use_signal(|| false);
    // Some((done, total)) while an export is running.
    let export_progress = use_signal(|| None::<(u32, u32)>);
    let mut export_dialog = use_signal(|| false);
    let mut crf = use_signal(|| 30u32);
    let mut fmt = use_signal(|| ExportFormat::WebmAlpha);

    let mut talk_on = use_signal(|| true);
    let mut mouth_poly = use_signal(Poly::new);
    let mut jaw_pct = use_signal(|| 0.55f32);
    let mut talk_amt = use_signal(|| 0.85f32);
    let mut inner_hex = use_signal(|| "#1a0d0d".to_string());
    // (name, path for muxing, 44.1 kHz mono samples)
    let mut talk_audio = use_signal(|| None::<(String, std::path::PathBuf, Arc<Vec<f64>>)>);
    let mut sensitivity = use_signal(|| 1.75f32);
    let mut gate_v = use_signal(|| 0.015f32);
    let mut smooth_v = use_signal(|| 0.12f32);

    let mut bg_motion = use_signal(|| BgMotion::Static);
    let mut bg_mo_speed = use_signal(|| 1.0f32);
    let mut bg_mo_amount = use_signal(|| 0.5f32);

    let mut viz_on = use_signal(|| false);
    let mut viz_kind = use_signal(|| VizKind::Bars);
    let mut viz_speed = use_signal(|| 1.0f32);
    let mut viz_hue = use_signal(|| 210.0f32);

    // ---------- derived ----------
    // Full-res image after (optional) background removal, plus removed-pixel count.
    let processed = use_memo(move || {
        base.read().as_ref().map(|img| {
            if bg_on() {
                effects::remove_background(img, tolerance())
            } else {
                (img.clone(), 0)
            }
        })
    });

    // Downscaled copy for live preview + the scale factor (display px / image px).
    let preview = use_memo(move || {
        processed.read().as_ref().map(|(img, _)| {
            let s = (PREVIEW_W / img.width() as f32).min(1.0);
            if s < 1.0 {
                let nw = (img.width() as f32 * s).round() as u32;
                let nh = (img.height() as f32 * s).round() as u32;
                (
                    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle),
                    s,
                )
            } else {
                (img.clone(), 1.0)
            }
        })
    });

    // Blink start times over the clip. Regular = fixed period; random = seeded
    // min/max gaps (reproducible, so exports match the preview).
    let blink_starts = use_memo(move || {
        let d = duration().max(0.1);
        let mut v = Vec::new();
        if blink_random() {
            let lo = min_gap().max(0.5);
            let hi = max_gap().max(lo + 0.1);
            let mut rng = StdRng::seed_from_u64(blink_seed());
            let mut t = rng.gen_range(lo..hi) * 0.5;
            while t < d {
                v.push(t);
                t += rng.gen_range(lo..hi);
            }
        } else {
            let p = blink_period().max(0.5);
            let mut t = 0.0;
            while t < d {
                v.push(t);
                t += p;
            }
        }
        v
    });

    // Per-frame mouth amplitudes from the loaded audio, at the current fps.
    let talk_env = use_memo(move || {
        talk_audio.read().as_ref().map(|(_, _, samples)| {
            effects::get_envelope(samples, fps(), sensitivity(), gate_v(), smooth_v())
        })
    });

    let make_cfg = move || RenderCfg {
        blink_on: blink_on(),
        eye_polys: eye_polys(),
        blink_starts: blink_starts(),
        talk_on: talk_on(),
        mouth_poly: mouth_poly(),
        jaw_pct: jaw_pct(),
        talk_amount: talk_amt(),
        inner_color: hex_rgb(&inner_hex()),
        talk_env: talk_env(),
        fps: fps(),
        viz_on: viz_on(),
        viz_kind: viz_kind(),
        viz_speed: viz_speed(),
        viz_hue: viz_hue(),
        bg_motion: bg_motion(),
        bg_motion_speed: bg_mo_speed(),
        bg_motion_amount: bg_mo_amount(),
    };

    // Background image pre-fitted to the preview canvas, so the per-tick render
    // doesn't re-scale it.
    let bg_prev = use_memo(move || {
        if !bgimg_on() {
            return None;
        }
        let (w, h) = preview.read().as_ref().map(|(img, _)| img.dimensions())?;
        bg_img.read().as_ref().map(|b| effects::cover(b, w, h))
    });

    let frame_uri = use_memo(move || {
        preview
            .read()
            .as_ref()
            .map(|(img, s)| {
                let cfg = make_cfg().scaled(*s);
                png_data_uri(&effects::render_frame(img, &cfg, time(), bg_prev.read().as_ref()))
            })
            .unwrap_or_default()
    });

    // ---------- playback ticker ----------
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(TICK_MS)).await;
            if playing() {
                let d = duration().max(0.1);
                time.set((time() + TICK_MS as f32 / 1000.0) % d);
            }
        }
    });

    // ---------- actions ----------
    let do_open = move || {
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
                .pick_file()
                .await
            else {
                return;
            };
            let name = f.file_name();
            match image::load_from_memory(&f.read().await) {
                Ok(d) => {
                    let img = d.to_rgba8();
                    status.set(format!("Loaded {name} ({}×{})", img.width(), img.height()));
                    img_name.set(name);
                    base.set(Some(img));
                    eye_polys.set(Vec::new());
                    cur_eye.set(Vec::new());
                    mouth_poly.set(Vec::new());
                    time.set(0.0);
                    zoom.set(1.0);
                    pan.set((0.0, 0.0));
                }
                Err(e) => status.set(format!("Failed to load {name}: {e}")),
            }
        });
    };

    let full_bg = move || {
        if *bgimg_on.peek() {
            bg_img.peek().clone()
        } else {
            None
        }
    };

    let do_load_bg = move || {
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
                .pick_file()
                .await
            else {
                return;
            };
            let name = f.file_name();
            match image::load_from_memory(&f.read().await) {
                Ok(d) => {
                    bg_img.set(Some(d.to_rgba8()));
                    bg_name.set(name);
                    bgimg_on.set(true);
                    status.set("Background image loaded.".into());
                }
                Err(e) => status.set(format!("Failed to load {name}: {e}")),
            }
        });
    };

    let mut do_export_frame = move || {
        let Some((img, _)) = processed.peek().clone() else {
            status.set("Load an image first".into());
            return;
        };
        let cfg = make_cfg();
        let t = *time.peek();
        let bg = full_bg();
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new()
                .set_file_name("moranima_frame.png")
                .save_file()
                .await
            else {
                return;
            };
            let path = f.path().to_path_buf();
            let res = tokio::task::spawn_blocking(move || {
                let bg = bg.map(|b| effects::cover(&b, img.width(), img.height()));
                effects::render_frame(&img, &cfg, t, bg.as_ref()).save(&path)
            })
            .await;
            status.set(match res {
                Ok(Ok(())) => "Frame exported.".into(),
                Ok(Err(e)) => format!("Export failed: {e}"),
                Err(_) => "Export task crashed".into(),
            });
        });
    };

    let full_dims = move || {
        processed
            .peek()
            .as_ref()
            .map(|(i, _)| (i.width() as f32, i.height() as f32))
    };
    // ~10 screen px expressed in image px at the current preview scale and zoom.
    let hit_tol = move || {
        let s = preview.peek().as_ref().map(|(_, s)| *s).unwrap_or(1.0);
        10.0 / (s * (*zoom.peek() as f32).max(0.05))
    };
    let d2 = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2);

    // Cursor-anchored zoom: keep the local point `p` fixed on screen.
    let mut zoom_at = move |factor: f64, p: (f64, f64)| {
        let s = zoom();
        let ns = (s * factor).clamp(0.25, 8.0);
        if (ns - s).abs() < 1e-9 {
            return;
        }
        let (tx, ty) = pan();
        pan.set((tx + (s - ns) * p.0, ty + (s - ns) * p.1));
        zoom.set(ns);
    };
    let stage_center = move || {
        preview
            .peek()
            .as_ref()
            .map(|(img, _)| (img.width() as f64 / 2.0, img.height() as f64 / 2.0))
            .unwrap_or((0.0, 0.0))
    };
    let mut reset_view = move || {
        zoom.set(1.0);
        pan.set((0.0, 0.0));
    };

    let mut finish_eye = move || {
        if cur_eye.read().len() >= 3 {
            let p = cur_eye();
            eye_polys.write().push(p);
            cur_eye.set(Vec::new());
            status.set(format!("Eye added — {} traced.", eye_polys.read().len()));
        } else {
            status.set("Need at least 3 points to finish an eye.".into());
        }
    };

    let on_stage_down = move |evt: MouseEvent| {
        if evt.trigger_button() == Some(MouseButton::Auxiliary) {
            evt.prevent_default();
            let c = evt.client_coordinates();
            pan_drag.set(Some((c.x, c.y)));
            return;
        }
        if evt.trigger_button() != Some(MouseButton::Primary) {
            return;
        }
        let Some(scale) = preview.peek().as_ref().map(|(_, s)| *s) else {
            return;
        };
        let c = evt.element_coordinates();
        let p = (c.x as f32 / scale, c.y as f32 / scale);
        let tol = hit_tol();
        match *selected.peek() {
            Layer::Blink => {
                // snap-to-close: clicking the first pending point finishes the eye
                let snap = {
                    let cur = cur_eye.read();
                    cur.len() >= 3 && d2(p, cur[0]) <= (tol * 1.5).powi(2)
                };
                if snap {
                    finish_eye();
                    return;
                }
                // grab an existing vertex — pending trace first, then finished eyes
                let mut best: Option<(f32, DragTarget)> = None;
                for (i, q) in cur_eye.read().iter().enumerate() {
                    let d = d2(p, *q);
                    if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, DragTarget::CurEye(i)));
                    }
                }
                for (pi, poly) in eye_polys.read().iter().enumerate() {
                    for (vi, q) in poly.iter().enumerate() {
                        let d = d2(p, *q);
                        if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, DragTarget::Eye(pi, vi)));
                        }
                    }
                }
                if let Some((_, t)) = best {
                    vertex_drag.set(Some(t));
                    return;
                }
                cur_eye.write().push(clamp_pt(p, full_dims()));
                status.set(format!(
                    "Eye trace: {} points — click the first point to close",
                    cur_eye.read().len()
                ));
            }
            Layer::Talk => {
                let mut best: Option<(f32, usize)> = None;
                for (i, q) in mouth_poly.read().iter().enumerate() {
                    let d = d2(p, *q);
                    if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
                if let Some((_, i)) = best {
                    vertex_drag.set(Some(DragTarget::Mouth(i)));
                    return;
                }
                mouth_poly.write().push(clamp_pt(p, full_dims()));
                status.set(format!("Mouth polygon: {} points", mouth_poly.read().len()));
            }
            _ => {}
        }
    };

    let on_stage_move = move |evt: MouseEvent| {
        if let Some((lx, ly)) = pan_drag() {
            let c = evt.client_coordinates();
            let (tx, ty) = pan();
            pan.set((tx + c.x - lx, ty + c.y - ly));
            pan_drag.set(Some((c.x, c.y)));
            return;
        }
        if let Some(t) = vertex_drag() {
            let Some(scale) = preview.peek().as_ref().map(|(_, s)| *s) else {
                return;
            };
            let c = evt.element_coordinates();
            let p = clamp_pt((c.x as f32 / scale, c.y as f32 / scale), full_dims());
            match t {
                DragTarget::CurEye(i) => {
                    if let Some(q) = cur_eye.write().get_mut(i) {
                        *q = p;
                    }
                }
                DragTarget::Eye(pi, vi) => {
                    if let Some(q) = eye_polys.write().get_mut(pi).and_then(|pl| pl.get_mut(vi)) {
                        *q = p;
                    }
                }
                DragTarget::Mouth(i) => {
                    if let Some(q) = mouth_poly.write().get_mut(i) {
                        *q = p;
                    }
                }
            }
        }
    };

    let mut end_drags = move || {
        pan_drag.set(None);
        vertex_drag.set(None);
    };

    // Right-click deletes the nearest vertex; with none nearby, undoes the
    // last point of the active trace (like Ventriloquism Studio).
    let on_stage_ctx = move |evt: Event<MouseData>| {
        evt.prevent_default();
        let Some(scale) = preview.peek().as_ref().map(|(_, s)| *s) else {
            return;
        };
        let c = evt.element_coordinates();
        let p = (c.x as f32 / scale, c.y as f32 / scale);
        let tol = hit_tol();
        match *selected.peek() {
            Layer::Blink => {
                let mut best: Option<(f32, DragTarget)> = None;
                for (i, q) in cur_eye.read().iter().enumerate() {
                    let d = d2(p, *q);
                    if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, DragTarget::CurEye(i)));
                    }
                }
                for (pi, poly) in eye_polys.read().iter().enumerate() {
                    for (vi, q) in poly.iter().enumerate() {
                        let d = d2(p, *q);
                        if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, DragTarget::Eye(pi, vi)));
                        }
                    }
                }
                match best {
                    Some((_, DragTarget::CurEye(i))) => {
                        cur_eye.write().remove(i);
                    }
                    Some((_, DragTarget::Eye(pi, vi))) => {
                        let mut polys = eye_polys.write();
                        polys[pi].remove(vi);
                        if polys[pi].len() < 3 {
                            polys.remove(pi);
                            drop(polys);
                            status.set("Eye removed (fewer than 3 points left).".into());
                        }
                    }
                    _ => {
                        cur_eye.write().pop();
                    }
                }
            }
            Layer::Talk => {
                let mut best: Option<(f32, usize)> = None;
                for (i, q) in mouth_poly.read().iter().enumerate() {
                    let d = d2(p, *q);
                    if d <= tol * tol && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
                match best {
                    Some((_, i)) => {
                        mouth_poly.write().remove(i);
                    }
                    None => {
                        mouth_poly.write().pop();
                    }
                }
            }
            _ => {}
        }
    };

    // Decode (via ffmpeg) and hook up an audio file: the mouth follows its
    // loudness and the clip duration snaps to the audio length.
    let load_audio_path = move |name: String, path: std::path::PathBuf| {
        spawn(async move {
            status.set(format!("Decoding {name}…"));
            let p = path.clone();
            let res = tokio::task::spawn_blocking(move || effects::decode_audio(&p)).await;
            match res {
                Ok(Ok(samples)) => {
                    let secs = samples.len() as f32 / effects::AUDIO_SR as f32;
                    duration.set(secs.clamp(1.0, 120.0));
                    talk_audio.set(Some((name, path, Arc::new(samples))));
                    talk_on.set(true);
                    status.set(format!(
                        "Audio loaded ({secs:.1} s) — the mouth now follows it; duration matched."
                    ));
                }
                Ok(Err(e)) => status.set(format!("Audio load failed: {e}")),
                Err(_) => status.set("Audio decode crashed".into()),
            }
        });
    };

    let do_load_audio = move || {
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new()
                .add_filter("Audio", &["wav", "mp3", "ogg", "flac", "m4a", "aac", "opus"])
                .pick_file()
                .await
            else {
                return;
            };
            load_audio_path(f.file_name(), f.path().to_path_buf());
        });
    };

    let mut open_export_dialog = move |preset: Option<ExportFormat>| {
        if processed.peek().is_none() {
            status.set("Load an image first".into());
            return;
        }
        if let Some(p) = preset {
            fmt.set(p);
        }
        export_dialog.set(true);
    };

    // Run the export for the currently chosen format. Video formats pipe to
    // ffmpeg; PNG sequence writes numbered files to a folder.
    let mut do_export = move || {
        export_dialog.set(false);
        if export_progress.peek().is_some() {
            status.set("An export is already running.".into());
            return;
        }
        let Some((img, _)) = processed.peek().clone() else {
            status.set("Load an image first".into());
            return;
        };
        let cfg = make_cfg();
        let (f, d) = (*fps.peek(), *duration.peek());
        let bg = full_bg();
        let audio_path = if *talk_on.peek() {
            talk_audio.peek().as_ref().map(|(_, p, _)| p.clone())
        } else {
            None
        };
        let q = *crf.peek();
        let format = *fmt.peek();
        let total = (f * d).round().max(1.0) as u32;

        if format.is_sequence() {
            spawn(async move {
                let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else {
                    return;
                };
                let dir = dir.path().to_path_buf();
                status.set(format!("Exporting {total} frames at full resolution…"));
                let counter = Arc::new(AtomicU32::new(0));
                let c2 = counter.clone();
                let handle = tokio::task::spawn_blocking(move || {
                    let bg = bg.map(|b| effects::cover(&b, img.width(), img.height()));
                    for i in 0..total {
                        let frame = effects::render_frame(&img, &cfg, i as f32 / f, bg.as_ref());
                        frame.save(dir.join(format!("moranima_{:04}.png", i + 1)))?;
                        c2.store(i + 1, Ordering::Relaxed);
                    }
                    Ok::<_, image::ImageError>(())
                });
                let res = track_progress(handle, counter, total, export_progress).await;
                status.set(match res {
                    Ok(Ok(())) => format!("Exported {total} PNG frames — import as an image sequence in Kdenlive."),
                    Ok(Err(e)) => format!("Export failed: {e}"),
                    Err(_) => "Export task crashed".into(),
                });
            });
            return;
        }

        let ext = format.ext();
        spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new()
                .set_file_name(&format!("moranima.{ext}"))
                .add_filter(format.label(), &[ext])
                .save_file()
                .await
            else {
                return;
            };
            let mut path = file.path().to_path_buf();
            if path.extension().is_none_or(|e| e != ext) {
                path.set_extension(ext);
            }
            status.set(format!("Encoding {total} frames — {}…", format.label()));
            let counter = Arc::new(AtomicU32::new(0));
            let c2 = counter.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let bg = bg.map(|b| effects::cover(&b, img.width(), img.height()));
                effects::export_video(&img, &cfg, bg.as_ref(), f, d, audio_path.as_deref(), q, format, &path, |i| {
                    c2.store(i, Ordering::Relaxed)
                })
            });
            let res = track_progress(handle, counter, total, export_progress).await;
            status.set(match res {
                Ok(Ok(n)) => format!("Exported {n} frames — {}.", format.label()),
                Ok(Err(e)) => format!("Export failed: {e}"),
                Err(_) => "Export task crashed".into(),
            });
        });
    };

    let on_key = move |evt: Event<KeyboardData>| {
        let mut combo = String::new();
        if evt.modifiers().ctrl() {
            combo.push_str("CTRL+");
        }
        if evt.modifiers().shift() {
            combo.push_str("SHIFT+");
        }
        if evt.modifiers().alt() {
            combo.push_str("ALT+");
        }
        match evt.key() {
            Key::Character(c) => combo.push_str(&c.to_uppercase()),
            other => combo.push_str(&other.to_string().to_uppercase()),
        }
        let mut handled = true;
        match combo.as_str() {
            "CTRL+O" => do_open(),
            "CTRL+B" => do_load_bg(),
            "CTRL+A" => do_load_audio(),
            "CTRL+E" => open_export_dialog(None),
            "CTRL+SHIFT+E" | "SHIFT+CTRL+E" => open_export_dialog(Some(ExportFormat::PngSequence)),
            "CTRL+S" => do_export_frame(),
            " " => playing.set(!playing()),
            "HOME" => time.set(0.0),
            "CTRL+=" | "CTRL++" | "CTRL+SHIFT+=" | "CTRL+SHIFT++" => zoom_at(1.25, stage_center()),
            "CTRL+-" => zoom_at(1.0 / 1.25, stage_center()),
            "CTRL+0" => reset_view(),
            "ENTER" => {
                if *selected.peek() == Layer::Blink {
                    finish_eye();
                } else {
                    handled = false;
                }
            }
            "ESCAPE" => {
                if export_dialog() {
                    export_dialog.set(false);
                } else if shortcuts_open() {
                    shortcuts_open.set(false);
                } else {
                    cur_eye.set(Vec::new());
                }
            }
            "DELETE" | "BACKSPACE" => match *selected.peek() {
                Layer::Blink => {
                    cur_eye.write().pop();
                }
                Layer::Talk => {
                    mouth_poly.write().pop();
                }
                _ => handled = false,
            },
            "F1" => shortcuts_open.set(!shortcuts_open()),
            "1" => selected.set(Layer::Blink),
            "2" => selected.set(Layer::Talk),
            "3" => selected.set(Layer::Bg),
            "4" => selected.set(Layer::BgImg),
            "5" => selected.set(Layer::Viz),
            _ => handled = false,
        }
        if handled {
            evt.prevent_default();
            evt.stop_propagation();
        }
    };

    let on_stage_wheel = move |evt: Event<WheelData>| {
        if !evt.modifiers().ctrl() {
            return;
        }
        evt.prevent_default();
        let dy = match evt.delta() {
            WheelDelta::Pixels(v) => v.y,
            WheelDelta::Lines(v) => v.y * 40.0,
            WheelDelta::Pages(v) => v.y * 400.0,
        };
        let f = if dy < 0.0 { 1.1 } else { 1.0 / 1.1 };
        let p = evt.element_coordinates();
        zoom_at(f, (p.x, p.y));
    };

    // Drop a file anywhere on the stage: audio drives the mouth; an image
    // loads the character, or the background when the BG Image layer is selected.
    let on_drop = move |evt: Event<DragData>| {
        evt.prevent_default();
        let Some(file) = evt.files().into_iter().next() else {
            return;
        };
        let ext = file.name().rsplit('.').next().unwrap_or_default().to_lowercase();
        if ["wav", "mp3", "ogg", "flac", "m4a", "aac", "opus"].contains(&ext.as_str()) {
            load_audio_path(file.name(), file.path());
            return;
        }
        let to_bg = *selected.peek() == Layer::BgImg;
        spawn(async move {
            let name = file.name();
            let bytes = match file.read_bytes().await {
                Ok(b) => b,
                Err(e) => {
                    status.set(format!("Could not read dropped file {name}: {e}"));
                    return;
                }
            };
            match image::load_from_memory(&bytes) {
                Ok(d) => {
                    let img = d.to_rgba8();
                    if to_bg {
                        bg_img.set(Some(img));
                        bg_name.set(name);
                        bgimg_on.set(true);
                        status.set("Background image loaded.".into());
                    } else {
                        status.set(format!("Loaded {name} ({}×{})", img.width(), img.height()));
                        img_name.set(name);
                        base.set(Some(img));
                        eye_polys.set(Vec::new());
                        cur_eye.set(Vec::new());
                        mouth_poly.set(Vec::new());
                        time.set(0.0);
                        reset_view();
                    }
                }
                Err(e) => status.set(format!("Failed to load {name}: {e}")),
            }
        });
    };

    let seek = move |frac: f32| {
        playing.set(false);
        time.set(frac.clamp(0.0, 1.0) * duration().max(0.1));
    };

    // ---------- timeline block geometry (fractions of duration) ----------
    let dur = duration().max(0.1);
    let blink_blocks: Vec<(f32, f32)> = blink_starts
        .read()
        .iter()
        .map(|s0| (s0 / dur, effects::BLINK_LEN.min(dur - s0).max(0.0) / dur))
        .collect();
    let talk_segs: Vec<(f32, f32)> = {
        let env = talk_env.read();
        let f = fps();
        let active = |t: f32| match env.as_ref() {
            Some(e) => e.get((t * f) as usize).copied().unwrap_or(0.0) > 0.05,
            None => effects::talk_gate(t) > 0.1,
        };
        let mut v = Vec::new();
        let mut start = None;
        let n = (dur / 0.05) as usize;
        for i in 0..=n {
            let t = i as f32 * 0.05;
            let on = active(t) && i < n;
            match (on, start) {
                (true, None) => start = Some(t),
                (false, Some(s0)) => {
                    v.push((s0 / dur, (t - s0) / dur));
                    start = None;
                }
                _ => {}
            }
        }
        v
    };
    let playhead_px = 148.0 + (time() / dur) as f64 * LANE_W;

    // ---------- window chrome handlers ----------
    let mut last_click =
        use_signal(|| std::time::Instant::now() - std::time::Duration::from_secs(10));
    let handle_drag = move |_| {
        let now = std::time::Instant::now();
        if now.duration_since(last_click()) < std::time::Duration::from_millis(400) {
            window().toggle_maximized();
            last_click.set(now - std::time::Duration::from_secs(10));
        } else {
            last_click.set(now);
            window().drag();
        }
    };

    rsx! {
        style { "{CSS}" }
        div { class: "mor-root",
            tabindex: "-1",
            autofocus: true,
            style: "outline: none;",
            onkeydown: on_key,
            // frameless resize edges
            div { class: "mor-resize-edge top", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::North); } }
            div { class: "mor-resize-edge bottom", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::South); } }
            div { class: "mor-resize-edge left", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::West); } }
            div { class: "mor-resize-edge right", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::East); } }
            div { class: "mor-resize-edge top-left", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::NorthWest); } }
            div { class: "mor-resize-edge top-right", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::NorthEast); } }
            div { class: "mor-resize-edge bottom-left", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::SouthWest); } }
            div { class: "mor-resize-edge bottom-right", onmousedown: move |_| { let _ = window().drag_resize_window(ResizeDirection::SouthEast); } }

            // ---------- titlebar ----------
            div { class: "mor-headerbar", onmousedown: handle_drag,
                div { class: "hb-start" }
                div { class: "hb-center",
                    span { class: "mor-window-title", "MorAnima" }
                    span { class: "mor-window-subtitle", "Layered Effects Studio" }
                }
                div { class: "hb-end",
                    div { class: "mor-window-controls", onmousedown: |e| e.stop_propagation(),
                        button { class: "window-btn", onclick: move |_| window().set_minimized(true), "—" }
                        button { class: "window-btn", onclick: move |_| window().toggle_maximized(), "□" }
                        button { class: "window-btn close", onclick: move |_| window().close(), "×" }
                    }
                }
            }

            // ---------- menu bar ----------
            nav { class: "mor-menu-bar",
                // system menu on the app icon, classic titlebar-icon behavior
                div { class: "mor-menu-item", style: "padding: 0 6px;",
                    span { class: "mor-menu-logo", "🎭" }
                    div { class: "mor-menu-dropdown",
                        MItem { label: "Minimize", on: move |_| window().set_minimized(true) }
                        MItem { label: "Maximize / Restore", on: move |_| window().toggle_maximized() }
                        div { class: "mor-menu-divider" }
                        MItem { label: "Close", on: move |_| window().close() }
                    }
                }
                div { class: "mor-menu-item", "File"
                    div { class: "mor-menu-dropdown",
                        MItem { label: "Open Image…", shortcut: "Ctrl+O", on: move |_| do_open() }
                        MItem { label: "Load Background Image…", shortcut: "Ctrl+B", on: move |_| do_load_bg() }
                        MItem { label: "Load Audio (lip-sync)…", shortcut: "Ctrl+A", on: move |_| do_load_audio() }
                        div { class: "mor-menu-divider" }
                        MItem { label: "Export…", shortcut: "Ctrl+E", on: move |_| open_export_dialog(None) }
                        MItem { label: "Export PNG Sequence…", shortcut: "Ctrl+Shift+E", on: move |_| open_export_dialog(Some(ExportFormat::PngSequence)) }
                        MItem { label: "Export Current Frame…", shortcut: "Ctrl+S", on: move |_| do_export_frame() }
                        div { class: "mor-menu-divider" }
                        MItem { label: "Quit", on: move |_| quit() }
                    }
                }
                div { class: "mor-menu-item", "View"
                    div { class: "mor-menu-dropdown",
                        MItem { label: "Zoom In", shortcut: "Ctrl+=", on: move |_| zoom_at(1.25, stage_center()) }
                        MItem { label: "Zoom Out", shortcut: "Ctrl+-", on: move |_| zoom_at(1.0 / 1.25, stage_center()) }
                        MItem { label: "Reset Zoom", shortcut: "Ctrl+0", on: move |_| reset_view() }
                    }
                }
                div { class: "mor-menu-item", "Playback"
                    div { class: "mor-menu-dropdown",
                        MItem { label: if playing() { "Pause" } else { "Play" }, shortcut: "Space", on: move |_| playing.set(!playing()) }
                        MItem { label: "Rewind", shortcut: "Home", on: move |_| time.set(0.0) }
                    }
                }
                div { class: "mor-menu-item", "Help"
                    div { class: "mor-menu-dropdown",
                        MItem { label: "Keyboard Shortcuts", shortcut: "F1", on: move |_| shortcuts_open.set(true) }
                        div { class: "mor-menu-divider" }
                        MItem {
                            label: "About MorAnima",
                            on: move |_| status.set("MorAnima 0.1 — blink, talk & background FX layers for still characters. Effects ported from morblink and Ventriloquism Studio.".into()),
                        }
                    }
                }
            }

            // ---------- main ----------
            div { class: "main-row",
                div { class: "side-panel",
                    // keep typing in inputs from triggering global shortcuts
                    onkeydown: |e| e.stop_propagation(),
                    h3 { class: "panel-title", "{selected().name()} — settings" }
                    match selected() {
                        Layer::Bg => rsx! {
                            Slider { label: "Tolerance", min: 0.0, max: 100.0, step: 1.0, value: tolerance(), on: move |v| tolerance.set(v) }
                            if bg_on() {
                                if let Some((_, n)) = processed.read().as_ref() {
                                    p { class: "hint ok", "✓ {n} pixels made transparent." }
                                }
                            } else {
                                p { class: "hint", "Layer is disabled — tick its checkbox in the timeline." }
                            }
                            p { class: "hint",
                                "Optional — skip it if your PNG already has real transparency. Detects the two most common border colors (solid or fake checkerboard) and gives matching pixels real alpha — fixes AI-generated PNGs."
                            }
                        },
                        Layer::BgImg => rsx! {
                            div {
                                button { class: "btn primary", onclick: move |_| do_load_bg(), "Load Background…" }
                                button { class: "btn", onclick: move |_| {
                                    bg_img.set(None);
                                    bg_name.set(String::new());
                                }, "Clear" }
                            }
                            if bg_img.read().is_some() {
                                p { class: "hint ok", "✓ {bg_name} — scaled to fill and cropped to the character canvas." }
                            } else {
                                p { class: "hint", "No background loaded." }
                            }
                            label { class: "param", "Motion "
                                select {
                                    onchange: move |e| bg_motion.set(match e.value().as_str() {
                                        "zoom" => BgMotion::Zoom,
                                        "drift" => BgMotion::Drift,
                                        "sway" => BgMotion::Sway,
                                        "tilt" => BgMotion::Tilt,
                                        _ => BgMotion::Static,
                                    }),
                                    option { value: "static", selected: bg_motion() == BgMotion::Static, "Static" }
                                    option { value: "zoom", selected: bg_motion() == BgMotion::Zoom, "Slow Zoom" }
                                    option { value: "drift", selected: bg_motion() == BgMotion::Drift, "Drift" }
                                    option { value: "sway", selected: bg_motion() == BgMotion::Sway, "Sway" }
                                    option { value: "tilt", selected: bg_motion() == BgMotion::Tilt, "Perspective Tilt" }
                                }
                            }
                            if bg_motion() != BgMotion::Static {
                                Slider { label: "Motion amount", min: 0.0, max: 1.0, step: 0.05, value: bg_mo_amount(), on: move |v| bg_mo_amount.set(v) }
                                Slider { label: "Motion speed", min: 0.1, max: 4.0, step: 0.1, value: bg_mo_speed(), on: move |v| bg_mo_speed.set(v) }
                            }
                            p { class: "hint",
                                "Optional. Shows behind the character wherever it's transparent. Sits above the Visualizer and below the character; Motion adds a gentle camera move (zoom, drift, sway, or a perspective wobble)."
                            }
                        },
                        Layer::Blink => rsx! {
                            label { class: "param",
                                input { r#type: "checkbox", checked: blink_random(), onchange: move |e| blink_random.set(e.checked()) }
                                " Random spacing"
                            }
                            if blink_random() {
                                Slider { label: "Min gap (s)", min: 0.5, max: 10.0, step: 0.5, value: min_gap(), on: move |v| min_gap.set(v) }
                                Slider { label: "Max gap (s)", min: 1.0, max: 12.0, step: 0.5, value: max_gap(), on: move |v| max_gap.set(v) }
                                label { class: "param", "Seed "
                                    input { r#type: "number", min: "0", value: "{blink_seed}",
                                        onchange: move |e| if let Ok(v) = e.value().parse::<u64>() { blink_seed.set(v); } }
                                    button { class: "btn", onclick: move |_| blink_seed.set(rand::random::<u64>() % 100_000), "🎲 Reroll" }
                                }
                                p { class: "hint", "Seeded, so exports reproduce the preview exactly." }
                            } else {
                                Slider { label: "Blink every (s)", min: 0.5, max: 8.0, step: 0.1, value: blink_period(), on: move |v| blink_period.set(v) }
                            }
                            p { class: "hint", "Click the preview to trace around one eye — click the first point again to close it. Drag points to adjust, right-click to delete." }
                            div {
                                button { class: "btn primary", onclick: move |_| finish_eye(), "Finish Eye" }
                                button { class: "btn", onclick: move |_| { cur_eye.write().pop(); }, "Undo Point" }
                                button { class: "btn", onclick: move |_| {
                                    eye_polys.set(Vec::new());
                                    cur_eye.set(Vec::new());
                                }, "Clear Eyes" }
                            }
                            p { class: "hint", "{eye_polys.read().len()} eye(s) traced, {cur_eye.read().len()} point(s) pending." }
                        },
                        Layer::Talk => rsx! {
                            if let Some((name, _, samples)) = talk_audio.read().as_ref() {
                                p { class: "hint ok", "♪ {name} · {samples.len() as f32 / effects::AUDIO_SR as f32:.1} s — driving the mouth" }
                                div {
                                    button { class: "btn", onclick: move |_| do_load_audio(), "Replace Audio…" }
                                    button { class: "btn", onclick: move |_| {
                                        talk_audio.set(None);
                                        status.set("Audio removed — mouth uses the built-in rhythm again.".into());
                                    }, "Remove Audio" }
                                }
                                Slider { label: "Sensitivity", min: 0.25, max: 8.0, step: 0.05, value: sensitivity(), on: move |v| sensitivity.set(v) }
                                Slider { label: "Silence gate", min: 0.0, max: 0.5, step: 0.005, value: gate_v(), on: move |v| gate_v.set(v) }
                                Slider { label: "Smoothing", min: 0.0, max: 2.0, step: 0.02, value: smooth_v(), on: move |v| smooth_v.set(v) }
                            } else {
                                button { class: "btn primary", style: "width: 100%;", onclick: move |_| do_load_audio(),
                                    "♪ Load Audio — lip-sync the mouth…"
                                }
                                p { class: "hint", "Or drop an audio file (WAV, MP3, OGG, FLAC…) anywhere on the stage. Without audio, the jaw moves to a built-in speech rhythm." }
                            }
                            Slider { label: "Jaw drop", min: 0.05, max: 1.5, step: 0.05, value: jaw_pct(), on: move |v| jaw_pct.set(v) }
                            Slider { label: "Talk amount", min: 0.0, max: 2.0, step: 0.05, value: talk_amt(), on: move |v| talk_amt.set(v) }
                            label { class: "param", "Inner mouth color "
                                input { r#type: "color", value: "{inner_hex}", oninput: move |e| inner_hex.set(e.value()) }
                            }
                            p { class: "hint", "Click the preview to outline the mouth — drag points to adjust, right-click to delete." }
                            div {
                                button { class: "btn", onclick: move |_| { mouth_poly.write().pop(); }, "Undo Point" }
                                button { class: "btn", onclick: move |_| mouth_poly.set(Vec::new()), "Clear Mouth" }
                            }
                            p { class: "hint", "{mouth_poly.read().len()} point(s) in mouth polygon." }
                        },
                        Layer::Viz => rsx! {
                            label { class: "param", "Style "
                                select {
                                    onchange: move |e| viz_kind.set(match e.value().as_str() {
                                        "waves" => VizKind::Waves,
                                        "stars" => VizKind::Starfield,
                                        "fireflies" => VizKind::Fireflies,
                                        "snow" => VizKind::Snow,
                                        "embers" => VizKind::Embers,
                                        _ => VizKind::Bars,
                                    }),
                                    option { value: "bars", selected: viz_kind() == VizKind::Bars, "Audio Bars" }
                                    option { value: "waves", selected: viz_kind() == VizKind::Waves, "Plasma Waves" }
                                    option { value: "stars", selected: viz_kind() == VizKind::Starfield, "Starfield" }
                                    option { value: "fireflies", selected: viz_kind() == VizKind::Fireflies, "Fireflies ✦" }
                                    option { value: "snow", selected: viz_kind() == VizKind::Snow, "Snowfall ✦" }
                                    option { value: "embers", selected: viz_kind() == VizKind::Embers, "Embers ✦" }
                                }
                            }
                            Slider { label: "Speed", min: 0.1, max: 4.0, step: 0.1, value: viz_speed(), on: move |v| viz_speed.set(v) }
                            Slider { label: "Hue", min: 0.0, max: 360.0, step: 1.0, value: viz_hue(), on: move |v| viz_hue.set(v) }
                            p { class: "hint", "Bars, Waves and Starfield are opaque backdrops behind everything. The ✦ particle kinds (Fireflies, Snowfall, Embers) float over the background image, tinted by Hue." }
                        },
                    }
                }

                div { class: "preview-wrap",
                    ondragover: move |e| e.prevent_default(),
                    ondrop: on_drop,
                    if frame_uri().is_empty() {
                        div { class: "empty-state",
                            h2 { class: "empty-title", "Summon a character" }
                            p { "Open a still image to animate — or drop one anywhere here. PNGs with transparency work best; the optional BG Removal layer can fix AI images with fake backgrounds." }
                            button { class: "btn primary", onclick: move |_| do_open(), "Open Image…" }
                        }
                    } else {
                        div { class: "preview-stage",
                            style: "transform: translate({pan().0}px, {pan().1}px) scale({zoom()}); transform-origin: 0 0;",
                            onmousedown: on_stage_down,
                            onmousemove: on_stage_move,
                            onmouseup: move |_| end_drags(),
                            onmouseleave: move |_| end_drags(),
                            oncontextmenu: on_stage_ctx,
                            onwheel: on_stage_wheel,
                            img { src: "{frame_uri}", draggable: false }
                            if let Some((pimg, s)) = preview.read().as_ref() {
                                svg { width: "{pimg.width()}", height: "{pimg.height()}",
                                    for poly in eye_polys.read().iter() {
                                        polygon { points: "{pts_attr(poly, *s)}", fill: "rgba(88,166,255,0.22)", stroke: "#58a6ff", stroke_width: "1", vector_effect: "non-scaling-stroke" }
                                    }
                                    if cur_eye.read().len() >= 2 {
                                        polygon { points: "{pts_attr(&cur_eye.read(), *s)}", fill: "none", stroke: "#58a6ff", stroke_width: "1", stroke_dasharray: "4 3", vector_effect: "non-scaling-stroke" }
                                    }
                                    for (i, (x, y)) in cur_eye.read().iter().enumerate() {
                                        circle { cx: "{x * s}", cy: "{y * s}", r: "3", fill: if i == 0 { "#e3cd8a" } else { "#58a6ff" } }
                                    }
                                    if mouth_poly.read().len() >= 2 {
                                        polygon { points: "{pts_attr(&mouth_poly.read(), *s)}", fill: "rgba(240,80,107,0.22)", stroke: "#f0506b", stroke_width: "1", vector_effect: "non-scaling-stroke" }
                                    }
                                    for (x, y) in mouth_poly.read().iter() {
                                        circle { cx: "{x * s}", cy: "{y * s}", r: "3", fill: "#f0506b" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ---------- timeline ----------
            div { class: "timeline",
                div { class: "transport",
                    onkeydown: |e| e.stop_propagation(),
                    button { class: "btn", onclick: move |_| time.set(0.0), "⏮" }
                    button { class: "btn primary", onclick: move |_| playing.set(!playing()),
                        if playing() { "⏸ Pause" } else { "▶ Play" }
                    }
                    span { "{time():.2} s" }
                    label { "Duration (s) "
                        input { r#type: "number", min: "1", max: "120", step: "0.5", value: "{duration}",
                            onchange: move |e| if let Ok(v) = e.value().parse::<f32>() { duration.set(v.clamp(1.0, 120.0)); } }
                    }
                    label { "FPS "
                        input { r#type: "number", min: "1", max: "60", value: "{fps}",
                            onchange: move |e| if let Ok(v) = e.value().parse::<f32>() { fps.set(v.clamp(1.0, 60.0)); } }
                    }
                    button { class: "btn", title: "Ctrl+scroll zooms toward the cursor, middle-drag pans",
                        onclick: move |_| reset_view(),
                        "🔍 {(zoom() * 100.0).round()}% · Reset"
                    }
                }
                div { class: "timeline-body",
                    div { class: "playhead", style: "left: {playhead_px}px;" }
                    Track { name: "Blink", active: selected() == Layer::Blink, enabled: blink_on(),
                        on_toggle: move |v| blink_on.set(v), on_select: move |_| selected.set(Layer::Blink), on_seek: seek,
                        if blink_on() {
                            for (l, w) in blink_blocks {
                                div { class: "blk", style: "left: {l * 100.0}%; width: {(w * 100.0).max(0.8)}%;" }
                            }
                        }
                    }
                    Track { name: "Talk", active: selected() == Layer::Talk, enabled: talk_on(),
                        on_toggle: move |v| talk_on.set(v), on_select: move |_| selected.set(Layer::Talk), on_seek: seek,
                        if talk_on() {
                            for (l, w) in talk_segs {
                                div { class: "blk talk", style: "left: {l * 100.0}%; width: {w * 100.0}%;" }
                            }
                        }
                    }
                    Track { name: "BG Removal", active: selected() == Layer::Bg, enabled: bg_on(),
                        on_toggle: move |v| bg_on.set(v), on_select: move |_| selected.set(Layer::Bg), on_seek: seek,
                        if bg_on() { div { class: "blk bg", style: "left: 0%; width: 100%;" } }
                    }
                    Track { name: "BG Image", active: selected() == Layer::BgImg, enabled: bgimg_on(),
                        on_toggle: move |v| bgimg_on.set(v), on_select: move |_| selected.set(Layer::BgImg), on_seek: seek,
                        if bgimg_on() && bg_img.read().is_some() { div { class: "blk img", style: "left: 0%; width: 100%;" } }
                    }
                    Track { name: "Visualizer", active: selected() == Layer::Viz, enabled: viz_on(),
                        on_toggle: move |v| viz_on.set(v), on_select: move |_| selected.set(Layer::Viz), on_seek: seek,
                        if viz_on() { div { class: "blk viz", style: "left: 0%; width: 100%;" } }
                    }
                }
            }

            // ---------- status bar ----------
            div { class: "statusbar",
                if base.read().is_some() {
                    span { "{img_name} · " }
                    if let Some((img, n)) = processed.read().as_ref() {
                        span { "{img.width()}×{img.height()}" }
                        if bg_on() {
                            span { class: "ok", "✓ {n} px transparent" }
                        }
                    }
                } else {
                    span { "no image" }
                }
                if let Some((n, _, _)) = talk_audio.read().as_ref() {
                    span { class: "ok", "♪ {n}" }
                }
                div { class: "grow",
                    if let Some((done, total)) = export_progress() {
                        div { class: "progress-row",
                            div { class: "progress-track",
                                div { class: "progress-fill", style: "width: {done * 100 / total.max(1)}%;" }
                            }
                            span { "Rendering… {done}/{total} frames ({done * 100 / total.max(1)}%)" }
                        }
                    } else {
                        "{status}"
                    }
                }
                span { "{time():.2} / {duration():.1} s · {fps():.0} fps · {(zoom() * 100.0).round()}% · {selected().name()}" }
            }

            ShortcutsDialog { open: shortcuts_open }

            // ---------- export dialog: pick a format, see exactly what you'll get ----------
            if export_dialog() {
                div { class: "mor-modal-backdrop", onclick: move |_| export_dialog.set(false),
                    div { class: "mor-modal", style: "min-width: 560px; max-width: 640px;",
                        onclick: |e| e.stop_propagation(),
                        onkeydown: |e| e.stop_propagation(),
                        div { class: "mor-modal-header",
                            span { "Export" }
                            div { class: "mor-modal-close", onclick: move |_| export_dialog.set(false), "×" }
                        }
                        div { class: "mor-modal-body",
                            div { class: "export-cols",
                                // ---- preset list (Kdenlive-style) ----
                                div { class: "preset-list",
                                    for preset in [ExportFormat::WebmAlpha, ExportFormat::Mp4, ExportFormat::Gif, ExportFormat::PngSequence] {
                                        button {
                                            class: if fmt() == preset { "preset-item active" } else { "preset-item" },
                                            onclick: move |_| fmt.set(preset),
                                            span { class: "preset-name", "{preset.label()}" }
                                        }
                                    }
                                }
                                // ---- what you'll get ----
                                div { class: "export-detail",
                                    p { class: "hint", style: "margin: 0 0 10px;", "{fmt().blurb()}" }
                                    if let Some((img, _)) = processed.read().as_ref() {
                                        {
                                            let (w, h) = img.dimensions();
                                            let (pw, ph) = (w.div_ceil(2) * 2, h.div_ceil(2) * 2);
                                            let total = (fps() * duration()).round().max(1.0) as u32;
                                            let opaque_bg = (viz_on() && !viz_kind().is_overlay())
                                                || (bgimg_on() && bg_img.read().is_some());
                                            let f = fmt();
                                            rsx! {
                                                div { class: "kv-row", span { "Output" }
                                                    span {
                                                        if f.is_sequence() { "moranima_0001.png … ({total} files)" }
                                                        else { "moranima.{f.ext()}" }
                                                    }
                                                }
                                                div { class: "kv-row", span { "Resolution" }
                                                    span {
                                                        if f.is_sequence() || (pw, ph) == (w, h) { "{w} × {h}" }
                                                        else { "{w} × {h} → {pw} × {ph}" }
                                                    }
                                                }
                                                div { class: "kv-row", span { "Length" } span { "{duration():.1} s · {fps():.0} fps · {total} frames" } }
                                                div { class: "kv-row", span { "Audio" }
                                                    if !f.has_audio() {
                                                        span { "not supported by this format" }
                                                    } else if let Some((n, _, _)) = talk_audio.read().as_ref() {
                                                        if talk_on() {
                                                            span { class: "ok", "♪ {n} — muxed in" }
                                                        } else {
                                                            span { "none (Talk layer disabled)" }
                                                        }
                                                    } else {
                                                        span { "none — silent" }
                                                    }
                                                }
                                                div { class: "kv-row", span { "Background" }
                                                    if f.alpha() && !opaque_bg {
                                                        span { class: "ok", "transparent — alpha preserved" }
                                                    } else if f.alpha() {
                                                        span { "opaque (visualizer / image fills it)" }
                                                    } else if opaque_bg {
                                                        span { "opaque (visualizer / image)" }
                                                    } else {
                                                        span { "flattened onto black" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if fmt().uses_crf() {
                                        div { style: "margin-top: 14px;",
                                            Slider { label: "Quality (CRF {crf()} — lower is better)", min: 0.0, max: 55.0, step: 1.0,
                                                value: crf() as f32, on: move |v: f32| crf.set(v as u32) }
                                            p { class: "hint", style: "text-align: right; margin-top: -4px;", "{crf_label(crf())}" }
                                        }
                                    }
                                }
                            }
                            div { style: "display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px;",
                                button { class: "btn", onclick: move |_| export_dialog.set(false), "Cancel" }
                                button { class: "btn primary", onclick: move |_| do_export(),
                                    if fmt().is_sequence() { "Choose Folder & Export…" } else { "Choose File & Export…" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Render "Ctrl+Shift+E" as keycap chips; " / " separates alternate combos.
#[component]
fn KeyCaps(combo: String) -> Element {
    rsx! {
        for (i, alt) in combo.split(" / ").enumerate() {
            if i > 0 {
                span { class: "mor-keycap-plus", "/" }
            }
            for (j, k) in alt.split('+').enumerate() {
                if j > 0 {
                    span { class: "mor-keycap-plus", "+" }
                }
                span { class: "mor-keycap", "{k}" }
            }
        }
    }
}

#[component]
fn ShortcutsDialog(open: Signal<bool>) -> Element {
    let mut open = open;
    if !open() {
        return rsx! {};
    }
    rsx! {
        div { class: "mor-modal-backdrop", onclick: move |_| open.set(false),
            div { class: "mor-modal", onclick: |e| e.stop_propagation(),
                div { class: "mor-modal-header",
                    span { "Keyboard Shortcuts" }
                    div { class: "mor-modal-close", onclick: move |_| open.set(false), "×" }
                }
                div { class: "mor-modal-body",
                    div { class: "mor-shortcuts-grid",
                        for (group, items) in SHORTCUTS {
                            div { class: "mor-shortcut-group",
                                h4 { class: "mor-shortcut-group-title", "{group}" }
                                for (combo, label) in items.iter() {
                                    div { class: "mor-shortcut-row",
                                        span { class: "mor-shortcut-keys",
                                            KeyCaps { combo: combo.to_string() }
                                        }
                                        div { class: "mor-action-label", "{label}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MItem(label: String, on: EventHandler<()>, #[props(default)] shortcut: Option<String>) -> Element {
    rsx! {
        button {
            class: "mor-menu-item",
            onmousedown: |e| e.stop_propagation(),
            onclick: move |e| {
                e.stop_propagation();
                on.call(());
            },
            span { "{label}" }
            if let Some(sc) = shortcut {
                span { class: "shortcut", "{sc}" }
            }
        }
    }
}

#[component]
fn Slider(label: String, min: f32, max: f32, step: f32, value: f32, on: EventHandler<f32>) -> Element {
    rsx! {
        label { class: "param",
            span { "{label}" }
            span { class: "val", "{value:.2}" }
            input {
                r#type: "range",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                oninput: move |e| if let Ok(v) = e.value().parse::<f32>() { on.call(v) },
            }
        }
    }
}

#[component]
fn Track(
    name: String,
    active: bool,
    enabled: bool,
    on_toggle: EventHandler<bool>,
    on_select: EventHandler<()>,
    on_seek: EventHandler<f32>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "track",
            input { r#type: "checkbox", checked: enabled, onchange: move |e| on_toggle.call(e.checked()) }
            button { class: if active { "track-name active" } else { "track-name" },
                onclick: move |_| on_select.call(()), "{name}"
            }
            div { class: "lane",
                onmousedown: move |e| on_seek.call((e.element_coordinates().x / LANE_W) as f32),
                {children}
            }
        }
    }
}
