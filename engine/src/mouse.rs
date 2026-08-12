#[derive(Debug, Default)]
pub struct PointerAccumulator {
    fractional_x: f32,
    fractional_y: f32,
}

impl PointerAccumulator {
    pub fn consume(&mut self, delta_x: f32, delta_y: f32) -> (i32, i32) {
        let total_x = delta_x + self.fractional_x;
        let total_y = delta_y + self.fractional_y;
        let integer_x = total_x as i32;
        let integer_y = total_y as i32;
        self.fractional_x = total_x - integer_x as f32;
        self.fractional_y = total_y - integer_y as f32;
        (integer_x, integer_y)
    }
}
