//! The window icon.
//!
//! The desktop app shows the VibeMux mark: a rounded square in the accent
//! color with a dark chevron, the same shape as
//! `crates/vibemux_web/web/public/vibemux_icon.svg`. The committed icons
//! are produced from that SVG by `npx tauri icon`; this module draws the
//! same mark so that the Windows resource step and the Tauri context
//! always have an icon, even in a tree where the committed assets are
//! absent. It writes plain PNG and ICO bytes and needs no image library.

use std::fs;
use std::io;
use std::path::Path;

const SIZE: usize = 64;
const SUPERSAMPLE: usize = 4;
const ACCENT: [u8; 3] = [0xC9, 0x64, 0x42];
const INK: [u8; 3] = [0x14, 0x14, 0x13];

/// The chevron polygon of the SVG, in the SVG's 64x64 coordinates.
const CHEVRON: [(f64, f64); 7] = [
    (13.0, 17.0),
    (21.0, 17.0),
    (32.0, 37.0),
    (43.0, 17.0),
    (51.0, 17.0),
    (35.0, 48.0),
    (29.0, 48.0),
];

/// Writes `icon.png` and `icon.ico` into `directory`.
pub fn write_icons(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let pixels = render();
    fs::write(directory.join("icon.png"), png_bytes(&pixels))?;
    fs::write(directory.join("icon.ico"), ico_bytes(&pixels))?;
    Ok(())
}

/// The mark at `SIZE` pixels square, as RGBA rows from the top.
fn render() -> Vec<[u8; 4]> {
    let scale = SIZE as f64 / 64.0;
    let mut pixels = Vec::with_capacity(SIZE * SIZE);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let mut square_hits = 0usize;
            let mut chevron_hits = 0usize;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = (px as f64 + (sx as f64 + 0.5) / SUPERSAMPLE as f64) / scale;
                    let y = (py as f64 + (sy as f64 + 0.5) / SUPERSAMPLE as f64) / scale;
                    if !inside_rounded_square(x, y, 64.0) {
                        continue;
                    }
                    square_hits += 1;
                    if inside_polygon(x, y) {
                        chevron_hits += 1;
                    }
                }
            }
            if square_hits == 0 {
                pixels.push([0, 0, 0, 0]);
                continue;
            }
            let samples = (SUPERSAMPLE * SUPERSAMPLE) as f64;
            let ink = chevron_hits as f64 / square_hits as f64;
            let mut color = [0u8; 3];
            for channel in 0..3 {
                color[channel] =
                    round(ACCENT[channel] as f64 * (1.0 - ink) + INK[channel] as f64 * ink);
            }
            pixels.push([
                color[0],
                color[1],
                color[2],
                round(square_hits as f64 / samples * 255.0),
            ]);
        }
    }
    pixels
}

fn inside_rounded_square(x: f64, y: f64, size: f64) -> bool {
    let radius = size * 16.0 / 64.0;
    let center_x = x.clamp(radius, size - radius);
    let center_y = y.clamp(radius, size - radius);
    let dx = x - center_x;
    let dy = y - center_y;
    dx * dx + dy * dy <= radius * radius
}

fn inside_polygon(x: f64, y: f64) -> bool {
    let mut inside = false;
    for index in 0..CHEVRON.len() {
        let (x1, y1) = CHEVRON[index];
        let (x2, y2) = CHEVRON[(index + 1) % CHEVRON.len()];
        if (y1 > y) != (y2 > y) {
            let crossing = (x2 - x1) * (y - y1) / (y2 - y1) + x1;
            if x < crossing {
                inside = !inside;
            }
        }
    }
    inside
}

/// The PNG bytes of the mark. The image data is stored uncompressed, which
/// keeps the encoder small and the output deterministic.
fn png_bytes(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(SIZE * (1 + SIZE * 4));
    for row in pixels.chunks(SIZE) {
        raw.push(0);
        for pixel in row {
            raw.extend_from_slice(pixel);
        }
    }

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::new();
    header.extend_from_slice(&(SIZE as u32).to_be_bytes());
    header.extend_from_slice(&(SIZE as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    push_chunk(&mut png, b"IHDR", &header);
    push_chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    png
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut offset = 0usize;
    loop {
        let length = (data.len() - offset).min(u16::MAX as usize);
        let last = offset + length >= data.len();
        out.push(u8::from(last));
        out.extend_from_slice(&(length as u16).to_le_bytes());
        out.extend_from_slice(&(!(length as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + length]);
        offset += length;
        if last {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// The ICO bytes of the mark: one 32-bit bottom-up bitmap entry.
fn ico_bytes(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut xor = Vec::with_capacity(SIZE * SIZE * 4);
    for row in pixels.chunks(SIZE).rev() {
        for pixel in row {
            xor.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    let mask_row = (SIZE + 31) / 32 * 4;
    let and_mask = vec![0u8; mask_row * SIZE];

    let mut info = Vec::with_capacity(40);
    info.extend_from_slice(&40u32.to_le_bytes());
    info.extend_from_slice(&(SIZE as i32).to_le_bytes());
    info.extend_from_slice(&((SIZE * 2) as i32).to_le_bytes());
    info.extend_from_slice(&1u16.to_le_bytes());
    info.extend_from_slice(&32u16.to_le_bytes());
    info.extend_from_slice(&((xor.len() + and_mask.len()) as u32).to_le_bytes());
    info.extend_from_slice(&0u32.to_le_bytes());
    info.extend_from_slice(&0u32.to_le_bytes());
    info.extend_from_slice(&0u32.to_le_bytes());
    info.extend_from_slice(&0u32.to_le_bytes());

    let image = [info, xor, and_mask].concat();
    let mut out = Vec::with_capacity(6 + 16 + image.len());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.push(SIZE as u8);
    out.push(SIZE as u8);
    out.push(0);
    out.push(0);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    out.extend_from_slice(&((6 + 16) as u32).to_le_bytes());
    out.extend_from_slice(&image);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut low = 1u32;
    let mut high = 0u32;
    for byte in data {
        low = (low + u32::from(*byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    (high << 16) | low
}

fn round(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}
