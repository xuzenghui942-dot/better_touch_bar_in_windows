use crate::settings::DeviceDragSettings;

pub fn apply_threshold_speed(distance: f32, device: &DeviceDragSettings) -> f32 {
    distance * (device.cursor_speed / 60.0)
}

pub fn apply_cursor_response(
    delta: (f32, f32),
    elapsed_ms: u64,
    device: &DeviceDragSettings,
) -> (f32, f32) {
    let mut scaled = (
        delta.0 * (device.cursor_speed / 120.0),
        delta.1 * (device.cursor_speed / 120.0),
    );

    let length = scaled.0.hypot(scaled.1);
    let mut mouse_velocity = (length / elapsed_ms as f32).min(4.0);
    if !mouse_velocity.is_finite() {
        mouse_velocity = 1.0;
    }

    let acceleration = device.cursor_acceleration / 10.0;
    let pointer_velocity = if acceleration == 0.0 {
        1.0
    } else {
        // The C# implementation promotes this expression to `double`
        // because it calls Math.Log2/Math.Exp. Keeping that precision also
        // avoids premature f32 overflow for allowed high acceleration values.
        let acceleration = f64::from(acceleration);
        let mouse_velocity = f64::from(mouse_velocity);
        let offset = (3.0 - (0.8_f64 / 0.3 - 1.0).log2()) / (2.6 * acceleration);
        let value = 2.6 * acceleration * (mouse_velocity - 1.0 + offset) - 3.0;
        (0.7 + 0.8 * sigmoid(value)) as f32
    };

    scaled.0 *= pointer_velocity;
    scaled.1 *= pointer_velocity;
    scaled
}

fn sigmoid(value: f64) -> f64 {
    let exponential = value.exp();
    exponential / (1.0 + exponential)
}

#[cfg(test)]
mod tests {
    use super::sigmoid;

    #[test]
    fn sigmoid_is_centered_at_one_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < f64::EPSILON);
    }
}
