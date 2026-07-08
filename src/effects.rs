//! All pixel work. Pure functions — no UI state.
//! Blink warp ported from morblink, jaw drop from ventriloquism-studio,
//! background removal from the PNG Transparency Fixer.

use image::{Rgba, RgbaImage};
use std::collections::HashMap;

pub type Poly = Vec<(f32, f32)>;

const TAU: f32 = std::f32::consts::TAU;
/// Where the lids meet, as a fraction of eye height from the top.
const MEET: f32 = 0.72;

// ---------- geometry / sampling helpers ----------

/// Even-odd ray cast against pixel centers.
fn point_in_poly(px: f32, py: f32, pts: &Poly) -> bool {
    let n = pts.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Polygon bounding box clamped to the image; None if degenerate/off-image.
fn bbox(pts: &Poly, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let x1 = (pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().max(0.0) as u32)
        .min(w.saturating_sub(1));
    let y1 = (pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().max(0.0) as u32)
        .min(h.saturating_sub(1));
    (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
}

/// Bilinear sample with edge clamping.
fn bilinear(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx, yy| img.get_pixel(xx, yy).0;
    let (p00, p10, p01, p11) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
        let bot = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
        out[c] = (top * (1.0 - fy) + bot * fy).round() as u8;
    }
    Rgba(out)
}

/// Smoothstep easing.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Multiply a pixel's RGB by `k`, leaving alpha alone.
fn shade_px(p: &mut Rgba<u8>, k: f32) {
    for ch in 0..3 {
        p.0[ch] = (p.0[ch] as f32 * k) as u8;
    }
}

/// Flatten alpha onto a solid `color`, making every pixel opaque. Used for
/// export formats that can't carry transparency (MP4, GIF).
pub fn flatten(img: &RgbaImage, color: [u8; 3]) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p.0[3] as f32 / 255.0;
        for c in 0..3 {
            p.0[c] = (p.0[c] as f32 * a + color[c] as f32 * (1.0 - a)).round() as u8;
        }
        p.0[3] = 255;
    }
    out
}

/// Source-over composite `fg` onto opaque `bg`.
pub fn over(bg: &mut RgbaImage, fg: &RgbaImage) {
    for (b, f) in bg.pixels_mut().zip(fg.pixels()) {
        let fa = f.0[3] as f32 / 255.0;
        for c in 0..3 {
            b.0[c] = (f.0[c] as f32 * fa + b.0[c] as f32 * (1.0 - fa)).round() as u8;
        }
    }
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0].map(|f| f as u8)
}

fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(2654435761);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    (x % 10000) as f32 / 10000.0
}

/// Scale-to-fill and center-crop `img` to exactly w×h (CSS `background-size: cover`).
pub fn cover(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let s = (w as f32 / iw as f32).max(h as f32 / ih as f32);
    let nw = ((iw as f32 * s).ceil() as u32).max(w);
    let nh = ((ih as f32 * s).ceil() as u32).max(h);
    let scaled = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
    image::imageops::crop_imm(&scaled, (nw - w) / 2, (nh - h) / 2, w, h).to_image()
}

// ---------- background removal (PNG Transparency Fixer port) ----------

/// Detect the two most common border colors and zero the alpha of every pixel
/// within `tolerance` (Euclidean RGB distance) of either. Returns the fixed
/// image and the number of pixels made transparent.
pub fn remove_background(src: &RgbaImage, tolerance: f32) -> (RgbaImage, u64) {
    let (w, h) = src.dimensions();
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    {
        let mut sample = |x: u32, y: u32| {
            let p = src.get_pixel(x, y);
            *counts.entry([p.0[0], p.0[1], p.0[2]]).or_insert(0) += 1;
        };
        for x in 0..w {
            sample(x, 0);
            sample(x, h - 1);
        }
        for y in 0..h {
            sample(0, y);
            sample(w - 1, y);
        }
    }
    let mut top: Vec<_> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let bg_colors: Vec<[u8; 3]> = top.into_iter().take(2).map(|(c, _)| c).collect();

    let thr = tolerance * tolerance;
    let mut out = src.clone();
    let mut fixed = 0u64;
    for p in out.pixels_mut() {
        for bg in &bg_colors {
            let dr = p.0[0] as f32 - bg[0] as f32;
            let dg = p.0[1] as f32 - bg[1] as f32;
            let db = p.0[2] as f32 - bg[2] as f32;
            if dr * dr + dg * dg + db * db <= thr {
                p.0[3] = 0;
                fixed += 1;
                break;
            }
        }
    }
    (out, fixed)
}

// ---------- blink (morblink liquify warp) ----------

/// Liquify one traced eye shut by `c` (0 = open, 1 = closed).
/// Column-wise squash: upper lid descends with stretched skin from above,
/// lower lid rises a little, eye content compresses into the slit, soft lash
/// shadow along the lid edge, feathered back into the original at the trace
/// boundary. Alpha is always preserved.
pub fn warp_eye(frame: &mut RgbaImage, open: &RgbaImage, pts: &Poly, c: f32) {
    if pts.len() < 3 || c <= 0.0 {
        return;
    }
    let (iw, ih) = open.dimensions();
    let Some((bx0, by0, bx1, by1)) = bbox(pts, iw, ih) else {
        return;
    };
    let ce = ease(c);

    // Per-column vertical extent of the polygon (NAN = column not in the eye).
    let cols = (bx1 - bx0 + 1) as usize;
    let mut top = vec![f32::NAN; cols];
    let mut bot = vec![f32::NAN; cols];
    for (i, x) in (bx0..=bx1).enumerate() {
        for y in by0..=by1 {
            if point_in_poly(x as f32 + 0.5, y as f32 + 0.5, pts) {
                if top[i].is_nan() {
                    top[i] = y as f32;
                }
                bot[i] = y as f32;
            }
        }
    }
    // 1-2-1 smoothing so the lid edge doesn't stair-step along the trace.
    let smooth = |v: &[f32]| -> Vec<f32> {
        (0..v.len())
            .map(|i| {
                let l = if i > 0 { v[i - 1] } else { v[i] };
                let r = if i + 1 < v.len() { v[i + 1] } else { v[i] };
                if l.is_nan() || r.is_nan() {
                    v[i]
                } else {
                    (l + 2.0 * v[i] + r) / 4.0
                }
            })
            .collect()
    };
    let (top, bot) = (smooth(&top), smooth(&bot));

    let maxh = top
        .iter()
        .zip(&bot)
        .map(|(t, b)| b - t)
        .fold(0.0f32, f32::max);
    if maxh <= 0.0 {
        return;
    }
    let feather = 1.5f32; // ponytail: calibration knob (px)
    // Distance in columns to the nearest end of the eye, for corner feathering.
    let mut dist_x = vec![0.0f32; cols];
    let mut run = 0.0;
    for i in 0..cols {
        run = if top[i].is_nan() { 0.0 } else { run + 1.0 };
        dist_x[i] = run;
    }
    run = 0.0;
    for i in (0..cols).rev() {
        run = if top[i].is_nan() { 0.0 } else { run + 1.0 };
        dist_x[i] = dist_x[i].min(run);
    }

    for (i, x) in (bx0..=bx1).enumerate() {
        let (t, b) = (top[i], bot[i]);
        if t.is_nan() || b <= t {
            continue;
        }
        let meet = t + MEET * (b - t);
        let u = t + ce * (meet - t); // upper lid edge, descending
        let l = b - ce * (b - meet); // lower lid edge, rising
        let corner = ((b - t) / (0.35 * maxh)).clamp(0.0, 1.0);
        for y in by0..=by1 {
            let orig = *open.get_pixel(x, y);
            let a = orig.0[3];
            if a <= 8 || !point_in_poly(x as f32 + 0.5, y as f32 + 0.5, pts) {
                continue;
            }
            let yf = y as f32;
            let mut px = if yf < u {
                let mut p = bilinear(open, x as f32, t - 1.5 - (u - yf) * 0.3);
                let depth = if u > t {
                    ((yf - t) / (u - t)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                shade_px(&mut p, 1.0 - 0.14 * depth * ce);
                p
            } else if yf > l {
                let mut p = bilinear(open, x as f32, b + 1.5 + (yf - l) * 0.3);
                shade_px(&mut p, 1.0 - 0.06 * ce);
                p
            } else {
                let src = t + (yf - u) / (l - u).max(0.01) * (b - t);
                bilinear(open, x as f32, src)
            };
            let d = (yf - u).abs();
            if d < 1.8 {
                shade_px(&mut px, 1.0 - 0.45 * ce * corner * (1.0 - d / 1.8));
            }
            let fd = (yf - t).min(dist_x[i]).max(0.0);
            let w = ease((fd / feather).min(1.0));
            for ch in 0..3 {
                px.0[ch] = (orig.0[ch] as f32 * (1.0 - w) + px.0[ch] as f32 * w).round() as u8;
            }
            px.0[3] = a;
            frame.put_pixel(x, y, px);
        }
    }
}

/// A blink lasts this many seconds: fast close, short hold, slower reopen.
pub const BLINK_LEN: f32 = 0.35;

/// One blink's closeness at `x` seconds after its start (0 outside the blink).
pub fn blink_curve(x: f32) -> f32 {
    if !(0.0..BLINK_LEN).contains(&x) {
        return 0.0;
    }
    let x = x / BLINK_LEN;
    if x < 0.4 {
        ease(x / 0.4)
    } else if x < 0.55 {
        1.0
    } else {
        ease(1.0 - (x - 0.55) / 0.45)
    }
}

/// Combined closeness at time `t` for a set of blink start times.
/// Overlapping blinks combine via `max`, like morblink's schedule stamps.
pub fn closeness_at(t: f32, starts: &[f32]) -> f32 {
    starts.iter().map(|s| blink_curve(t - s)).fold(0.0, f32::max)
}

// ---------- audio-driven talk (ventriloquism-studio envelope port) ----------

pub const AUDIO_SR: u32 = 44100;

/// Decode any audio file to 44.1 kHz mono samples via the system ffmpeg.
pub fn decode_audio(path: &std::path::Path) -> std::io::Result<Vec<f64>> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "s16le", "-ac", "1", "-ar", "44100", "-"])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                std::io::Error::other("ffmpeg not found — install it to use audio")
            }
            _ => e,
        })?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "ffmpeg could not decode the audio: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        )));
    }
    Ok(out
        .stdout
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0)
        .collect())
}

struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn bandpass(fs: f64, f_low: f64, f_high: f64) -> Self {
        use std::f64::consts::PI;
        let nyq = fs / 2.0;
        let f_low = f_low.clamp(20.0, nyq * 0.95);
        let f_high = f_high.clamp(f_low + 10.0, nyq * 0.99);
        let f0 = (f_low * f_high).sqrt();
        let bw = (f_high - f_low) / f0;
        let w0 = 2.0 * PI * f0 / fs;
        let alpha = w0.sin() * (2.0f64.ln() / 2.0 * bw * w0 / w0.sin()).sinh();
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: -2.0 * w0.cos() / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn gaussian_blur_1d(data: &[f64], sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 {
        return data.to_vec();
    }
    let radius = (sigma * 3.0).ceil() as i32;
    let mut kernel = Vec::new();
    let mut sum = 0.0;
    for i in -radius..=radius {
        let x = i as f64;
        let val = (-x * x / (2.0 * sigma * sigma)).exp();
        kernel.push(val);
        sum += val;
    }
    for k in &mut kernel {
        *k /= sum;
    }
    let mut out = vec![0.0; data.len()];
    for i in 0..data.len() {
        let mut v = 0.0;
        for j in -radius..=radius {
            let idx = (i as i32 + j).clamp(0, data.len() as i32 - 1) as usize;
            v += data[idx] * kernel[(j + radius) as usize];
        }
        out[i] = v;
    }
    out
}

/// Per-frame mouth amplitudes from audio: zero-phase 60–5000 Hz bandpass →
/// per-frame RMS → normalize to peak → sensitivity → noise gate → gamma →
/// optional Gaussian smoothing. Ported from Ventriloquism Studio.
// ponytail: filter band / gamma / offset fixed at Ventriloquism's defaults —
// expose them if a voice ever needs different tuning.
pub fn get_envelope(audio: &[f64], fps: f32, sensitivity: f32, gate: f32, smoothing: f32) -> Vec<f32> {
    let sr = AUDIO_SR as f64;
    let mut bq = Biquad::bandpass(sr, 60.0, 5000.0);
    let mut filtered: Vec<f64> = audio.iter().map(|&x| bq.process(x)).collect();
    filtered.reverse();
    let mut bq_rev = Biquad::bandpass(sr, 60.0, 5000.0);
    filtered = filtered.into_iter().map(|x| bq_rev.process(x)).collect();
    filtered.reverse();

    let spf = sr / fps as f64;
    let n_frames = (audio.len() as f64 / spf).ceil() as usize;
    let mut env = vec![0.0f64; n_frames];
    for (i, e) in env.iter_mut().enumerate() {
        let start = (i as f64 * spf) as usize;
        let end = ((start as f64 + spf).min(filtered.len() as f64)) as usize;
        if start < end {
            let sum_sq: f64 = filtered[start..end].iter().map(|x| x * x).sum();
            *e = (sum_sq / (end - start) as f64).sqrt();
        }
    }

    let mx = env.iter().cloned().fold(0.0, f64::max);
    if mx > 0.0 {
        for v in &mut env {
            *v /= mx;
        }
    }
    let (sensitivity, gate) = (sensitivity as f64, gate as f64);
    for v in &mut env {
        *v = (*v * sensitivity).clamp(0.0, 1.0);
        if *v < gate {
            *v = 0.0;
        } else {
            *v = (*v - gate) / (1.0 - gate + 1e-6);
        }
        *v = v.clamp(0.0, 1.0).powf(1.1);
    }
    if smoothing > 0.0 {
        env = gaussian_blur_1d(&env, smoothing as f64 * 8.0);
        let mx2 = env.iter().cloned().fold(0.0, f64::max);
        if mx2 > 0.0 {
            for v in &mut env {
                *v /= mx2;
            }
        }
    }
    env.into_iter().map(|v| v as f32).collect()
}

// ---------- talk (ventriloquism-studio jaw drop) ----------

/// Drop the jaw: fill the mouth polygon with `inner`, then shift the original
/// polygon content down by `mask_height * jaw_pct * amplitude` pixels.
pub fn jaw_drop(base: &RgbaImage, pts: &Poly, amplitude: f32, jaw_pct: f32, inner: [u8; 3]) -> RgbaImage {
    let mut out = base.clone();
    if pts.len() < 3 || amplitude <= 0.001 {
        return out;
    }
    let (iw, ih) = base.dimensions();
    let Some((bx0, by0, bx1, by1)) = bbox(pts, iw, ih) else {
        return out;
    };
    let min_y = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let max_y = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    let drop = ((max_y - min_y).max(1.0) * jaw_pct * amplitude).round() as u32;
    if drop < 1 {
        return out;
    }
    let inner_px = Rgba([inner[0], inner[1], inner[2], 255]);
    for y in by0..=by1 {
        for x in bx0..=bx1 {
            if point_in_poly(x as f32 + 0.5, y as f32 + 0.5, pts) {
                out.put_pixel(x, y, inner_px);
            }
        }
    }
    // Bottom-to-top so shifted pixels don't overwrite unread source rows.
    for y in (by0..=by1).rev() {
        for x in bx0..=bx1 {
            if point_in_poly(x as f32 + 0.5, y as f32 + 0.5, pts) {
                let sy = y + drop;
                if sy < ih {
                    let p = *base.get_pixel(x, y);
                    if p.0[3] > 0 {
                        out.put_pixel(x, sy, p);
                    }
                }
            }
        }
    }
    out
}

/// Phrase gate: 0 between phrases, 1 mid-phrase. Deterministic, so exports
/// reproduce the preview exactly.
pub fn talk_gate(t: f32) -> f32 {
    let phrase = 0.5 + 0.5 * (TAU * 0.35 * t + 1.3).sin();
    ((phrase - 0.35) / 0.15).clamp(0.0, 1.0)
}

/// Built-in speech rhythm: syllable oscillation gated into phrases.
// ponytail: synthetic rhythm, not audio-driven — port ventriloquism's
// bandpass+RMS envelope here if real lip-sync to a WAV is ever needed.
pub fn talk_amplitude(t: f32) -> f32 {
    let syll = (TAU * 4.2 * t).sin().abs();
    (syll * talk_gate(t)).powf(0.8)
}

// ---------- background motion (zoom / drift / sway / perspective) ----------

#[derive(Clone, Copy, PartialEq)]
pub enum BgMotion {
    Static,
    Zoom,
    Drift,
    Sway,
    Tilt,
}

/// Animate a canvas-sized background with a gentle camera move. Inverse-maps
/// every output pixel through scale/rotate/offset (+ a trapezoid squeeze for
/// the perspective tilt) and bilinear-samples the source. Motions with base
/// zoom > 1 keep the edges from showing.
pub fn animate_bg(img: &RgbaImage, t: f32, kind: BgMotion, speed: f32, amount: f32) -> RgbaImage {
    if kind == BgMotion::Static || amount <= 0.0 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let ph = TAU * 0.05 * speed * t;
    let (z, mut rot, mut dx, mut dy, mut tilt);
    (rot, dx, dy, tilt) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    match kind {
        BgMotion::Static => unreachable!(),
        BgMotion::Zoom => z = 1.0 + amount * 0.12 * (0.5 + 0.5 * (ph * 2.0).sin()),
        BgMotion::Drift => {
            z = 1.0 + amount * 0.08;
            dx = amount * 0.05 * w as f32 * (ph * 2.0).sin();
            dy = amount * 0.03 * h as f32 * (ph * 1.3).cos();
        }
        BgMotion::Sway => {
            z = 1.0 + amount * 0.06;
            rot = amount * 0.035 * (ph * 2.0).sin();
        }
        BgMotion::Tilt => {
            z = 1.0 + amount * 0.1;
            tilt = amount * 0.25 * (ph * 2.0).sin();
        }
    }
    let (sinr, cosr) = rot.sin_cos();
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        let yr = y as f32 - cy;
        let persp = 1.0 + tilt * (yr / h as f32);
        for x in 0..w {
            let xr = (x as f32 - cx) * persp;
            let sx = (xr * cosr - yr * sinr) / z + cx - dx;
            let sy = (xr * sinr + yr * cosr) / z + cy - dy;
            out.put_pixel(x, y, bilinear(img, sx, sy));
        }
    }
    out
}

// ---------- background visualizers & particles ----------

#[derive(Clone, Copy, PartialEq)]
pub enum VizKind {
    Bars,
    Waves,
    Starfield,
    Fireflies,
    Snow,
    Embers,
}

impl VizKind {
    /// Overlay kinds render on a transparent canvas and composite *above* the
    /// background image; the rest are opaque backdrops below it.
    pub fn is_overlay(self) -> bool {
        matches!(self, VizKind::Fireflies | VizKind::Snow | VizKind::Embers)
    }
}

/// Soft radial glow dot, alpha-composited (used by the particle kinds).
fn glow(img: &mut RgbaImage, x: f32, y: f32, r: f32, col: [u8; 3], bright: f32) {
    let (w, h) = img.dimensions();
    if r <= 0.0 || bright <= 0.0 {
        return;
    }
    let x0 = (x - r).floor().max(0.0) as u32;
    let y0 = (y - r).floor().max(0.0) as u32;
    let x1 = ((x + r).ceil() as u32).min(w.saturating_sub(1));
    let y1 = ((y + r).ceil() as u32).min(h.saturating_sub(1));
    for py in y0..=y1 {
        for px in x0..=x1 {
            let d = (((px as f32 - x).powi(2) + (py as f32 - y).powi(2)).sqrt() / r).min(1.0);
            let a = (1.0 - d).powi(2) * bright;
            if a <= 0.004 {
                continue;
            }
            let p = img.get_pixel_mut(px, py);
            for c in 0..3 {
                p.0[c] = (col[c] as f32 * a + p.0[c] as f32 * (1.0 - a)) as u8;
            }
            p.0[3] = p.0[3].max((a * 255.0) as u8);
        }
    }
}

/// Render one visualizer frame — opaque backdrop, or transparent overlay for
/// the particle kinds (see [`VizKind::is_overlay`]).
pub fn render_viz(w: u32, h: u32, t: f32, kind: VizKind, speed: f32, hue: f32) -> RgbaImage {
    let base = if kind.is_overlay() {
        Rgba([0, 0, 0, 0])
    } else {
        Rgba([10, 12, 18, 255])
    };
    let mut img = RgbaImage::from_pixel(w, h, base);
    let (wf, hf) = (w as f32, h as f32);
    match kind {
        VizKind::Fireflies => {
            for i in 0..60u32 {
                let x = (hash01(i * 3 + 1)
                    + 0.08 * (t * speed * 0.3 * TAU * (0.5 + hash01(i * 5)) + hash01(i * 7) * TAU).sin())
                .rem_euclid(1.0)
                    * wf;
                let y = (hash01(i * 3 + 2)
                    + 0.06 * (t * speed * 0.25 * TAU * (0.5 + hash01(i * 11)) + hash01(i * 13) * TAU).cos())
                .rem_euclid(1.0)
                    * hf;
                let pulse =
                    0.2 + 0.8 * (t * speed * (1.0 + hash01(i * 17)) * 2.0 + hash01(i * 19) * TAU).sin().abs();
                let c = hsv(hue + hash01(i) * 30.0, 0.55, 1.0);
                glow(&mut img, x, y, hf * 0.008 + 2.0, c, pulse);
            }
        }
        VizKind::Snow => {
            for i in 0..140u32 {
                let fall = (hash01(i * 2)
                    + t * speed * 0.05 * (0.5 + hash01(i * 3)))
                .rem_euclid(1.0);
                let x = (hash01(i * 5)
                    + 0.03 * (t * speed * 0.4 * TAU * (0.5 + hash01(i * 7)) + hash01(i * 11) * TAU).sin())
                .rem_euclid(1.0)
                    * wf;
                let r = hf * 0.003 + 1.0 + hash01(i * 13) * 2.0;
                let c = hsv(hue, 0.06, 1.0);
                glow(&mut img, x, fall * hf, r, c, 0.5 + 0.4 * hash01(i * 17));
            }
        }
        VizKind::Embers => {
            for i in 0..90u32 {
                let prog = (hash01(i * 2) + t * speed * 0.07 * (0.4 + hash01(i * 5))).rem_euclid(1.0);
                let x = (hash01(i * 3)
                    + 0.05 * prog * (t * speed * 0.6 * TAU * (0.5 + hash01(i * 7)) + hash01(i * 11) * TAU).sin())
                .rem_euclid(1.0)
                    * wf;
                let y = (1.0 - prog) * hf;
                let bright = (1.0 - prog) * 0.85 + 0.1;
                let c = hsv(hue + hash01(i * 13) * 25.0, 0.9, 1.0);
                glow(&mut img, x, y, hf * 0.004 + 1.5, c, bright);
            }
        }
        VizKind::Bars => {
            let n = 48u32;
            let bw = (w as f32 / n as f32).max(1.0);
            for i in 0..n {
                let a = (t * speed * 3.1 + i as f32 * 1.7).sin().abs()
                    * (0.4 + 0.6 * (t * speed * 1.3 + i as f32 * 0.31).sin().abs());
                let bh = ((h as f32 * (0.05 + 0.85 * a)) as u32).min(h);
                let c = hsv(hue + i as f32 * 4.0, 0.75, 0.9);
                let x0 = (i as f32 * bw) as u32;
                let x1 = (((i as f32 + 0.8) * bw) as u32).min(w);
                for y in h - bh..h {
                    for x in x0..x1 {
                        img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
                    }
                }
            }
        }
        VizKind::Waves => {
            for y in 0..h {
                for x in 0..w {
                    let v = 0.5
                        + 0.5
                            * (x as f32 / w as f32 * TAU * 3.0 + t * speed).sin()
                            * (y as f32 / h as f32 * TAU * 2.0 - t * speed * 0.7).sin();
                    let c = hsv(hue + v * 70.0, 0.7, 0.2 + 0.6 * v);
                    img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
        VizKind::Starfield => {
            for i in 0..240u32 {
                let x = ((hash01(i * 2 + 1) * w as f32) as u32).min(w - 1);
                let drift = t * speed * 12.0 * (0.3 + hash01(i * 7));
                let y = ((hash01(i * 2) * h as f32 + drift) as u32) % h;
                let tw = 0.35 + 0.65 * (t * speed * 2.0 + hash01(i * 3) * TAU).sin().abs();
                let b = (255.0 * tw) as u8;
                for dy in 0..2u32 {
                    for dx in 0..2u32 {
                        img.put_pixel((x + dx).min(w - 1), (y + dy).min(h - 1), Rgba([b, b, b, 255]));
                    }
                }
            }
        }
    }
    img
}

// ---------- per-frame composition ----------

#[derive(Clone, PartialEq)]
pub struct RenderCfg {
    pub blink_on: bool,
    pub eye_polys: Vec<Poly>,
    pub blink_starts: Vec<f32>,
    pub talk_on: bool,
    pub mouth_poly: Poly,
    pub jaw_pct: f32,
    pub talk_amount: f32,
    pub inner_color: [u8; 3],
    /// Per-frame audio amplitudes (at `fps`); None = built-in speech rhythm.
    pub talk_env: Option<Vec<f32>>,
    pub fps: f32,
    pub viz_on: bool,
    pub viz_kind: VizKind,
    pub viz_speed: f32,
    pub viz_hue: f32,
    pub bg_motion: BgMotion,
    pub bg_motion_speed: f32,
    pub bg_motion_amount: f32,
}

impl RenderCfg {
    /// Scale all polygon coordinates (for rendering on a downscaled preview).
    pub fn scaled(&self, s: f32) -> Self {
        let sp = |p: &Poly| -> Poly { p.iter().map(|(x, y)| (x * s, y * s)).collect() };
        Self {
            eye_polys: self.eye_polys.iter().map(&sp).collect(),
            mouth_poly: sp(&self.mouth_poly),
            ..self.clone()
        }
    }
}

/// Compose all enabled layers at time `t`: blink warp, then jaw drop, then
/// composite the character over the backdrop stack — visualizer at the bottom,
/// then the optional background image (`bg_img` must already be canvas-sized,
/// see [`cover`]), character on top.
pub fn render_frame(base: &RgbaImage, cfg: &RenderCfg, t: f32, bg_img: Option<&RgbaImage>) -> RgbaImage {
    let mut frame = base.clone();
    if cfg.blink_on && !cfg.eye_polys.is_empty() {
        let c = closeness_at(t, &cfg.blink_starts);
        if c > 0.0 {
            let open = frame.clone();
            for poly in &cfg.eye_polys {
                warp_eye(&mut frame, &open, poly, c);
            }
        }
    }
    if cfg.talk_on && cfg.mouth_poly.len() >= 3 {
        let raw = match &cfg.talk_env {
            Some(env) => env.get((t * cfg.fps) as usize).copied().unwrap_or(0.0),
            None => talk_amplitude(t),
        };
        let amp = (raw * cfg.talk_amount).clamp(0.0, 1.5);
        frame = jaw_drop(&frame, &cfg.mouth_poly, amp, cfg.jaw_pct, cfg.inner_color);
    }
    // Backdrop stack, bottom to top: opaque visualizer → background image
    // (with optional camera motion) → particle overlay → character.
    let (w, h) = frame.dimensions();
    let mut backdrop: Option<RgbaImage> = None;
    if cfg.viz_on && !cfg.viz_kind.is_overlay() {
        backdrop = Some(render_viz(w, h, t, cfg.viz_kind, cfg.viz_speed, cfg.viz_hue));
    }
    if let Some(bg) = bg_img {
        let moved = animate_bg(bg, t, cfg.bg_motion, cfg.bg_motion_speed, cfg.bg_motion_amount);
        match backdrop.as_mut() {
            Some(b) => over(b, &moved),
            None => backdrop = Some(moved),
        }
    }
    if cfg.viz_on && cfg.viz_kind.is_overlay() {
        let b = backdrop.get_or_insert_with(|| RgbaImage::from_pixel(w, h, Rgba([10, 12, 18, 255])));
        over(b, &render_viz(w, h, t, cfg.viz_kind, cfg.viz_speed, cfg.viz_hue));
    }
    if let Some(mut b) = backdrop {
        over(&mut b, &frame);
        frame = b;
    }
    frame
}

/// Output container/codec presets (Kdenlive-style).
#[derive(Clone, Copy, PartialEq)]
pub enum ExportFormat {
    /// VP9 WebM, transparency preserved.
    WebmAlpha,
    /// H.264 MP4, opaque, plays everywhere.
    Mp4,
    /// Animated GIF, looping, opaque, wide compatibility.
    Gif,
    /// Lossless PNG frames written to a folder.
    PngSequence,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::WebmAlpha => "WebM · VP9 (transparent)",
            ExportFormat::Mp4 => "MP4 · H.264 (universal)",
            ExportFormat::Gif => "Animated GIF (looping)",
            ExportFormat::PngSequence => "PNG Sequence (lossless)",
        }
    }
    /// One-line "what this is good for".
    pub fn blurb(self) -> &'static str {
        match self {
            ExportFormat::WebmAlpha => "Keeps a transparent background. Best for overlays and compositing; not every player supports it.",
            ExportFormat::Mp4 => "Flattened onto black. The safe choice for sharing, phones, and social media.",
            ExportFormat::Gif => "Loops forever, flattened onto black. Great for stickers and emotes; limited colors.",
            ExportFormat::PngSequence => "One numbered PNG per frame, no quality loss. Import as an image sequence in Kdenlive.",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::WebmAlpha => "webm",
            ExportFormat::Mp4 => "mp4",
            ExportFormat::Gif => "gif",
            ExportFormat::PngSequence => "",
        }
    }
    pub fn alpha(self) -> bool {
        self == ExportFormat::WebmAlpha
    }
    pub fn uses_crf(self) -> bool {
        matches!(self, ExportFormat::WebmAlpha | ExportFormat::Mp4)
    }
    pub fn has_audio(self) -> bool {
        matches!(self, ExportFormat::WebmAlpha | ExportFormat::Mp4)
    }
    pub fn is_sequence(self) -> bool {
        self == ExportFormat::PngSequence
    }
}

/// Export the clip through the system `ffmpeg`, piping raw RGBA frames. Formats
/// without alpha are flattened onto black; audio (if given and the format
/// supports it) is muxed in. Returns frames written. Not for `PngSequence` —
/// that path writes files directly (see the app layer).
#[allow(clippy::too_many_arguments)]
pub fn export_video(
    base: &RgbaImage,
    cfg: &RenderCfg,
    bg: Option<&RgbaImage>,
    fps: f32,
    duration: f32,
    audio: Option<&std::path::Path>,
    crf: u32,
    format: ExportFormat,
    out_path: &std::path::Path,
    mut progress: impl FnMut(u32),
) -> std::io::Result<u32> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let total = ((fps * duration).round() as u32).max(1);
    let (w, h) = base.dimensions();
    let matte = (!format.alpha()).then_some([0u8, 0, 0]);
    let audio = audio.filter(|_| format.has_audio());

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-v", "error", "-y",
        "-f", "rawvideo", "-pix_fmt", "rgba",
        "-s", &format!("{w}x{h}"),
        "-r", &format!("{fps}"),
        "-i", "-",
    ]);
    if let Some(a) = audio {
        cmd.arg("-i").arg(a);
    }
    let crf_s = format!("{}", crf.min(63));
    match format {
        ExportFormat::WebmAlpha => {
            cmd.args([
                // yuva420p needs even dimensions; pad with transparent pixels if odd.
                "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:0:0:color=black@0",
                "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-b:v", "0",
                "-crf", &crf_s, "-row-mt", "1",
            ]);
        }
        ExportFormat::Mp4 => {
            cmd.args([
                "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p",
                "-c:v", "libx264", "-preset", "fast", "-crf", &crf_s,
                "-movflags", "+faststart",
            ]);
        }
        ExportFormat::Gif => {
            // Single-pass optimized palette so colors don't band badly.
            cmd.args([
                "-vf", "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3",
                "-loop", "0",
            ]);
        }
        ExportFormat::PngSequence => {
            return Err(std::io::Error::other("PNG sequence is written directly, not via ffmpeg"));
        }
    }
    if let Some(a) = audio {
        let _ = a;
        match format {
            ExportFormat::WebmAlpha => cmd.args(["-c:a", "libopus", "-shortest"]),
            ExportFormat::Mp4 => cmd.args(["-c:a", "aac", "-shortest"]),
            _ => &mut cmd,
        };
    }
    let mut child = cmd
        .arg(out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => std::io::Error::other(
                "ffmpeg not found — install it, or use 'Export PNG Sequence' instead",
            ),
            _ => e,
        })?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    let mut write_err = Ok(());
    for i in 0..total {
        let frame = render_frame(base, cfg, i as f32 / fps, bg);
        let frame = match matte {
            Some(c) => flatten(&frame, c),
            None => frame,
        };
        // A broken pipe here means ffmpeg died — fall through to report its stderr.
        if let Err(e) = stdin.write_all(frame.as_raw()) {
            write_err = Err(e);
            break;
        }
        progress(i + 1);
    }
    drop(stdin); // close the pipe so ffmpeg finalizes the file
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(std::io::Error::other(format!(
            "ffmpeg failed: {}",
            err.lines().last().unwrap_or("unknown error")
        )));
    }
    write_err?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_curve_bounded_and_returns_to_zero() {
        let starts = [0.0, 3.0];
        for i in 0..1000 {
            let c = closeness_at(i as f32 * 0.01, &starts);
            assert!((0.0..=1.0).contains(&c));
        }
        assert_eq!(closeness_at(1.0, &starts), 0.0); // between blinks
        assert!(closeness_at(0.175, &starts) > 0.9); // mid-blink hold
        assert!(closeness_at(3.175, &starts) > 0.9); // second blink fires
    }

    #[test]
    fn background_removal_clears_border_color_only() {
        let mut img = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        for y in 5..15 {
            for x in 5..15 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        let (out, n) = remove_background(&img, 30.0);
        assert_eq!(n, 300); // 400 - 100 subject pixels
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(10, 10).0[3], 255);
    }

    #[test]
    fn jaw_drop_fills_polygon_and_shifts_content() {
        let img = RgbaImage::from_pixel(30, 30, Rgba([100, 150, 200, 255]));
        let poly = vec![(5.0, 5.0), (25.0, 5.0), (25.0, 15.0), (5.0, 15.0)];
        let out = jaw_drop(&img, &poly, 1.0, 0.5, [0, 0, 0]);
        assert_eq!(out.get_pixel(10, 6).0, [0, 0, 0, 255]); // inner fill
        assert_eq!(out.get_pixel(10, 20).0, [100, 150, 200, 255]); // shifted content
        // amplitude 0 = no-op
        assert_eq!(jaw_drop(&img, &poly, 0.0, 0.5, [0, 0, 0]), img);
    }

    #[test]
    fn video_export_writes_webm_when_ffmpeg_present() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return; // no ffmpeg on this machine — nothing to verify
        }
        let img = RgbaImage::from_pixel(32, 33, Rgba([10, 20, 30, 255])); // odd height exercises padding
        let cfg = RenderCfg {
            blink_on: false,
            eye_polys: vec![],
            blink_starts: vec![],
            talk_on: false,
            mouth_poly: vec![],
            jaw_pct: 0.5,
            talk_amount: 1.0,
            inner_color: [0, 0, 0],
            talk_env: None,
            fps: 10.0,
            viz_on: true,
            viz_kind: VizKind::Fireflies,
            viz_speed: 1.0,
            viz_hue: 200.0,
            bg_motion: BgMotion::Zoom,
            bg_motion_speed: 1.0,
            bg_motion_amount: 0.5,
        };
        let path = std::env::temp_dir().join("moranima_export_test.webm");
        let bg = RgbaImage::from_pixel(32, 33, Rgba([50, 60, 70, 255]));
        let mut last = 0;
        let n = export_video(&img, &cfg, Some(&bg), 10.0, 0.5, None, 30, ExportFormat::WebmAlpha, &path, |i| last = i).unwrap();
        assert_eq!(n, 5);
        assert_eq!(last, 5); // progress callback reached the final frame
        assert!(std::fs::metadata(&path).unwrap().len() > 0);

        // MP4 (flattened, no alpha) must also produce a file.
        let mp4 = std::env::temp_dir().join("moranima_export_test.mp4");
        let n = export_video(&img, &cfg, None, 10.0, 0.5, None, 23, ExportFormat::Mp4, &mp4, |_| {}).unwrap();
        assert_eq!(n, 5);
        assert!(std::fs::metadata(&mp4).unwrap().len() > 0);
        let _ = std::fs::remove_file(&mp4);

        // With audio: generate a 0.5 s sine, decode it, and mux it in.
        let wav = std::env::temp_dir().join("moranima_test_tone.wav");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.5"])
            .arg(&wav)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let samples = decode_audio(&wav).unwrap();
        assert!(samples.len() > 20_000); // ~0.5 s at 44.1 kHz
        let n = export_video(&img, &cfg, None, 10.0, 0.5, Some(&wav), 20, ExportFormat::WebmAlpha, &path, |_| {}).unwrap();
        assert_eq!(n, 5);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn envelope_follows_loudness() {
        // 1 s: 0.5 s of 440 Hz tone, then 0.5 s silence.
        let sr = AUDIO_SR as usize;
        let audio: Vec<f64> = (0..sr)
            .map(|i| {
                if i < sr / 2 {
                    (TAU as f64 * 440.0 * i as f64 / sr as f64).sin() * 0.8
                } else {
                    0.0
                }
            })
            .collect();
        let env = get_envelope(&audio, 10.0, 1.75, 0.015, 0.0);
        assert_eq!(env.len(), 10);
        assert!(env[2] > 0.5, "tone frames should be loud, got {}", env[2]);
        assert!(env[8] < 0.05, "silent frames should be gated, got {}", env[8]);
        assert!(env.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn animate_bg_keeps_dims_and_static_is_identity() {
        let img = RgbaImage::from_pixel(40, 30, Rgba([9, 9, 9, 255]));
        assert_eq!(animate_bg(&img, 1.0, BgMotion::Static, 1.0, 1.0), img);
        for kind in [BgMotion::Zoom, BgMotion::Drift, BgMotion::Sway, BgMotion::Tilt] {
            assert_eq!(animate_bg(&img, 1.3, kind, 1.0, 0.8).dimensions(), (40, 30));
        }
    }

    #[test]
    fn cover_crops_to_exact_dims() {
        let img = RgbaImage::from_pixel(100, 50, Rgba([1, 2, 3, 255]));
        assert_eq!(cover(&img, 30, 60).dimensions(), (30, 60));
        assert_eq!(cover(&img, 200, 20).dimensions(), (200, 20));
    }

    #[test]
    fn warp_eye_preserves_alpha_and_stays_in_bbox() {
        let img = RgbaImage::from_pixel(30, 30, Rgba([180, 140, 120, 255]));
        let mut frame = img.clone();
        let poly = vec![(8.0, 10.0), (22.0, 10.0), (22.0, 18.0), (8.0, 18.0)];
        warp_eye(&mut frame, &img, &poly, 1.0);
        assert_ne!(frame, img); // it did something
        assert_eq!(frame.get_pixel(0, 0).0, [180, 140, 120, 255]); // outside untouched
        for p in frame.pixels() {
            assert_eq!(p.0[3], 255);
        }
    }
}
