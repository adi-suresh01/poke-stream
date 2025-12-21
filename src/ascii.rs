use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, Frame, RgbImage};

pub struct AsciiImage {
    pub width: usize,
    pub height: usize,
    pub chars: Vec<char>,
    pub colors: Vec<(u8, u8, u8)>,
}

pub fn load_ascii_image(path: &str, width: usize, height: usize, charset: &str) -> AsciiImage {
    let img = image::open(path)
        .unwrap_or_else(|_| panic!("failed to load image: {path}"))
        .resize_exact(width as u32, height as u32, FilterType::Nearest)
        .to_rgb8();
    ascii_from_rgb(img, charset)
}

pub fn load_ascii_animation(path: &str, width: usize, height: usize, charset: &str) -> Vec<AsciiImage> {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|_| panic!("failed to load animation: {path}"));
    let reader = std::io::BufReader::new(file);
    let decoder = GifDecoder::new(reader)
        .unwrap_or_else(|_| panic!("failed to decode animation: {path}"));
    let frames = decoder
        .into_frames()
        .collect_frames()
        .unwrap_or_else(|_| panic!("failed to read animation frames: {path}"));

    let mut out = Vec::with_capacity(frames.len());
    for frame in frames.into_iter() {
        let frame: Frame = frame;
        let img = DynamicImage::ImageRgba8(frame.into_buffer())
            .resize_exact(width as u32, height as u32, FilterType::Nearest)
            .to_rgb8();
        out.push(ascii_from_rgb(img, charset));
    }
    out
}

fn ascii_from_rgb(img: RgbImage, charset: &str) -> AsciiImage {
    let charset: Vec<char> = charset.chars().collect();
    let width = img.width() as usize;
    let height = img.height() as usize;

    let mut base_rgb = Vec::with_capacity(width * height);
    let mut base_lum = Vec::with_capacity(width * height);
    for pixel in img.pixels() {
        let [r, g, b] = pixel.0;
        let lum = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
        base_rgb.push((r, g, b));
        base_lum.push(lum);
    }

    let mut bg_mask = vec![false; width * height];
    let corner_samples = [
        img.get_pixel(0, 0).0,
        img.get_pixel((width - 1) as u32, 0).0,
        img.get_pixel(0, (height - 1) as u32).0,
        img.get_pixel((width - 1) as u32, (height - 1) as u32).0,
    ];
    let mut bg_r = 0u32;
    let mut bg_g = 0u32;
    let mut bg_b = 0u32;
    for c in corner_samples {
        bg_r += c[0] as u32;
        bg_g += c[1] as u32;
        bg_b += c[2] as u32;
    }
    let bg_r = (bg_r / 4) as i32;
    let bg_g = (bg_g / 4) as i32;
    let bg_b = (bg_b / 4) as i32;
    let bg_thresh = 18i32;

    let mut stack = Vec::new();
    for x in 0..width {
        stack.push((x, 0));
        stack.push((x, height - 1));
    }
    for y in 0..height {
        stack.push((0, y));
        stack.push((width - 1, y));
    }
    while let Some((x, y)) = stack.pop() {
        let idx = x + y * width;
        if bg_mask[idx] {
            continue;
        }
        let (r, g, b) = base_rgb[idx];
        let dr = (r as i32 - bg_r).abs();
        let dg = (g as i32 - bg_g).abs();
        let db = (b as i32 - bg_b).abs();
        if dr <= bg_thresh && dg <= bg_thresh && db <= bg_thresh {
            bg_mask[idx] = true;
            if x > 0 {
                stack.push((x - 1, y));
            }
            if x + 1 < width {
                stack.push((x + 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if y + 1 < height {
                stack.push((x, y + 1));
            }
        }
    }

    let mut chars = Vec::with_capacity(width * height);
    let mut colors = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let idx = x + y * width;
            if bg_mask[idx] {
                chars.push(' ');
                colors.push((0, 0, 0));
                continue;
            }
            let lum = base_lum[idx];

            let left = if x > 0 { base_lum[idx - 1] } else { lum };
            let right = if x + 1 < width { base_lum[idx + 1] } else { lum };
            let up = if y > 0 { base_lum[idx - width] } else { lum };
            let down = if y + 1 < height { base_lum[idx + width] } else { lum };
            let edge = ((right - left).abs() + (down - up).abs()) * 0.7;

            let lx = -0.6;
            let ly = -0.4;
            let light = ((x as f32 / (width - 1) as f32) * lx
                + (y as f32 / (height - 1) as f32) * ly
                + 1.0)
                .clamp(0.4, 1.2);

            let shaded_lum = (lum * light - edge * 0.45).clamp(0.0, 1.0);
            let shade = (0.55 + shaded_lum * 0.7).clamp(0.35, 1.15);
            let (r, g, b) = base_rgb[idx];
            let (r, g, b) = apply_color_boost(r, g, b, shade);

            let char_idx = ((1.0 - shaded_lum) * (charset.len() - 1) as f32).round() as usize;
            chars.push(charset[char_idx]);
            colors.push((r, g, b));
        }
    }

    AsciiImage {
        width,
        height,
        chars,
        colors,
    }
}

fn apply_color_boost(r: u8, g: u8, b: u8, shade: f32) -> (u8, u8, u8) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let lum = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
    let sat = 1.15;
    let rf = (lum + (rf - lum) * sat) * shade;
    let gf = (lum + (gf - lum) * sat) * shade;
    let bf = (lum + (bf - lum) * sat) * shade;
    let r = (rf * 255.0).clamp(0.0, 255.0) as u8;
    let g = (gf * 255.0).clamp(0.0, 255.0) as u8;
    let b = (bf * 255.0).clamp(0.0, 255.0) as u8;
    (r, g, b)
}
