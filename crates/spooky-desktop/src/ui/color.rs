//! A colour that belongs to no toolkit: red, green, blue and alpha in 0..=1.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub(crate) const BLACK: Self = Self::from_rgb8(0, 0, 0);

    pub(crate) const fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// The same colour with its alpha multiplied by `factor`.
    pub(crate) fn scale_alpha(self, factor: f32) -> Self {
        Self {
            a: self.a * factor,
            ..self
        }
    }

    /// The same colour with this alpha.
    pub(crate) fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// `#rrggbb`, ignoring alpha.
    pub(crate) fn hex(self) -> String {
        let byte = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            byte(self.r),
            byte(self.g),
            byte(self.b)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn hex_round_trips_the_byte_values() {
        assert_eq!(Color::from_rgb8(15, 16, 20).hex(), "#0f1014");
        assert_eq!(Color::from_rgb8(255, 255, 255).hex(), "#ffffff");
    }

    #[test]
    fn scaling_alpha_leaves_the_channels() {
        let color = Color::from_rgb8(10, 20, 30).scale_alpha(0.5);
        assert_eq!(color.a, 0.5);
        assert_eq!(color.hex(), "#0a141e");
    }
}
