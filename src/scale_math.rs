//! Scale validity is a numeric contract, not a configured gameplay range.

#[inline]
pub(crate) fn valid(scale: f32) -> bool {
    scale.is_finite() && scale > 0.0
}

/// Scaling cannot turn a finite nonzero input into infinity or zero.
#[inline]
pub(crate) fn representable(input: f32, output: f32) -> bool {
    input.is_finite() && output.is_finite() && (input == 0.0 || output != 0.0)
}

#[inline]
pub(crate) fn product(input: f32, scale: f32) -> Option<f32> {
    let output = input * scale;
    (valid(scale) && representable(input, output)).then_some(output)
}
