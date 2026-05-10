//! Value-or-keyframed-curve. Story F-030 will fill in the `Animated`
//! variant (bezier tangents, hold/linear interp). For now only the static
//! form is supported so transforms can compile.

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Curve<T> {
    Static(T),
}

impl<T: Clone> Curve<T> {
    /// Sample the curve at a given frame. The signature accepts the frame
    /// index now so call sites won't need to change once `Animated` lands.
    pub fn sample_at(&self, _frame: u32) -> T {
        match self {
            Curve::Static(v) => v.clone(),
        }
    }
}

impl<T: Default> Default for Curve<T> {
    fn default() -> Self {
        Curve::Static(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve_samples_constant() {
        let c = Curve::Static(0.5_f32);
        assert_eq!(c.sample_at(0), 0.5);
        assert_eq!(c.sample_at(100), 0.5);
    }

    #[test]
    fn static_curve_default() {
        let c: Curve<f32> = Curve::default();
        assert_eq!(c.sample_at(0), 0.0);
    }
}
