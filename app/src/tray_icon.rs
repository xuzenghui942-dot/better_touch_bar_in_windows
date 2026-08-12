pub fn create_tray_rgba(size: usize) -> Vec<u8> {
    let mut pixels = vec![0_u8; size * size * 4];
    if size == 0 {
        return pixels;
    }

    let center = (size as f32 - 1.0) / 2.0;
    let outer_radius = size as f32 * 0.46;
    for y in 0..size {
        for x in 0..size {
            let offset_x = x as f32 - center;
            let offset_y = y as f32 - center;
            if offset_x.hypot(offset_y) <= outer_radius {
                set_pixel(&mut pixels, size, x, y, [72, 92, 199, 255]);
            }
        }
    }

    let finger_radius = (size as f32 * 0.09).max(1.0);
    let finger_centers = [
        (center - size as f32 * 0.20, center - size as f32 * 0.03),
        (center, center - size as f32 * 0.13),
        (center + size as f32 * 0.20, center - size as f32 * 0.03),
    ];
    for (finger_x, finger_y) in finger_centers {
        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - finger_x;
                let dy = y as f32 - finger_y;
                if dx.hypot(dy) <= finger_radius {
                    set_pixel(&mut pixels, size, x, y, [255, 255, 255, 255]);
                }
            }
        }
    }

    let palm_y = center + size as f32 * 0.22;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - center) / (size as f32 * 0.24);
            let dy = (y as f32 - palm_y) / (size as f32 * 0.12);
            if dx * dx + dy * dy <= 1.0 {
                set_pixel(&mut pixels, size, x, y, [255, 255, 255, 255]);
            }
        }
    }
    pixels
}

fn set_pixel(pixels: &mut [u8], size: usize, x: usize, y: usize, color: [u8; 4]) {
    let index = (y * size + x) * 4;
    pixels[index..index + 4].copy_from_slice(&color);
}
