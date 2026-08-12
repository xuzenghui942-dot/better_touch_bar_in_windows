use std::{fs, io, path::Path};

fn main() {
    generate_original_icon().expect("failed to generate the Windows application icon");
    tauri_build::build()
}

fn generate_original_icon() -> io::Result<()> {
    let icon_path = Path::new("icons/icon.ico");
    if icon_path.exists() {
        return Ok(());
    }

    if let Some(parent) = icon_path.parent() {
        fs::create_dir_all(parent)?;
    }

    const SIZE: usize = 32;
    const MASK_ROW_BYTES: usize = SIZE / 8;
    const PIXEL_BYTES: usize = SIZE * SIZE * 4;
    const MASK_BYTES: usize = MASK_ROW_BYTES * SIZE;
    const IMAGE_BYTES: usize = 40 + PIXEL_BYTES + MASK_BYTES;

    let mut icon = Vec::with_capacity(22 + IMAGE_BYTES);

    // ICONDIR.
    icon.extend_from_slice(&0_u16.to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());

    // ICONDIRENTRY.
    icon.extend_from_slice(&[SIZE as u8, SIZE as u8, 0, 0]);
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&32_u16.to_le_bytes());
    icon.extend_from_slice(&(IMAGE_BYTES as u32).to_le_bytes());
    icon.extend_from_slice(&22_u32.to_le_bytes());

    // BITMAPINFOHEADER. ICO DIB height includes the XOR and AND planes.
    icon.extend_from_slice(&40_u32.to_le_bytes());
    icon.extend_from_slice(&(SIZE as i32).to_le_bytes());
    icon.extend_from_slice(&((SIZE * 2) as i32).to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&32_u16.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());
    icon.extend_from_slice(&((PIXEL_BYTES + MASK_BYTES) as u32).to_le_bytes());
    icon.extend_from_slice(&0_i32.to_le_bytes());
    icon.extend_from_slice(&0_i32.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());

    // A small, original three-stroke icon. DIB rows are stored bottom-up as BGRA.
    for y in (0..SIZE).rev() {
        for x in 0..SIZE {
            let stroke = (8..=11).contains(&x) || (14..=17).contains(&x) || (20..=23).contains(&x);
            let rounded_cap = (6..=25).contains(&y)
                || ((x == 9 || x == 10 || x == 15 || x == 16 || x == 21 || x == 22)
                    && (4..=27).contains(&y));
            let (b, g, r) = if stroke && rounded_cap {
                (245, 248, 255)
            } else {
                (126, 82, 37)
            };
            icon.extend_from_slice(&[b, g, r, 255]);
        }
    }
    icon.resize(icon.len() + MASK_BYTES, 0);

    fs::write(icon_path, icon)
}
