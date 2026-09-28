use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.svg");
    println!("cargo:rerun-if-changed=build.rs");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let svg = fs::read("assets/icon.svg").expect("Read icon artwork");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .expect("Valid icon SVG");
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let mut pixels = resvg::tiny_skia::Pixmap::new(size, size).expect("Icon pixel buffer");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 256.0, size as f32 / 256.0),
            &mut pixels.as_mut(),
        );
        if size == 64 {
            // Winit expects straight-alpha RGBA; the SVG renderer uses premultiplied RGB.
            let mut rgba = pixels.data().to_vec();
            for pixel in rgba.as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                for channel in &mut pixel[..3] {
                    *channel = (u32::from(*channel) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0)
                        .min(255) as u8;
                }
            }
            fs::write(output.join("window-icon.rgba"), rgba).expect("Write window icon");
        }
        let png = pixels.encode_png().expect("Encode icon PNG");
        if size == 256 {
            fs::write(output.join("icon-preview.png"), &png).expect("Write icon preview");
        }
        frames.push(png);
    }

    // ICO directory followed by PNG frames, supported by Windows Vista and newer.
    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = (6 + 16 * sizes.len()) as u32;
    for (size, frame) in sizes.into_iter().zip(&frames) {
        let dimension = if size == 256 { 0 } else { size as u8 };
        ico.extend_from_slice(&[dimension, dimension, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in frames {
        ico.extend_from_slice(&frame);
    }
    let icon_path = output.join("mobi-reader.ico");
    fs::write(&icon_path, ico).expect("Write executable icon");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon(icon_path.to_str().expect("Icon path"))
            .set("ProductName", "Mobi Reader")
            .set(
                "FileDescription",
                "Mobi Reader - a quiet place for your books",
            )
            .compile()
            .expect("Embed Windows icon resource");
    }
}
