//! Series stacking and axis scales, shared by parsing and drawing.
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StackMode {
    #[default]
    None,
    Normal,
    Percent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stacking {
    pub mode: StackMode,
    pub group: String,
}
impl Stacking {
    pub fn new(mode: StackMode, group: impl Into<String>) -> Self {
        Self {
            mode,
            group: group.into(),
        }
    }
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let mode = match value.get("mode")?.as_str()? {
            "none" => StackMode::None,
            "normal" => StackMode::Normal,
            "percent" => StackMode::Percent,
            _ => return None,
        };
        // Old dashboard migrations sometimes preserve stack:false as group:false.
        if value.get("group").and_then(Value::as_bool) == Some(false) {
            return Some(Self::default());
        }
        Some(Self::new(
            mode,
            value.get("group").and_then(Value::as_str).unwrap_or("A"),
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AxisScale {
    #[default]
    Linear,
    Log(f64),
}
impl AxisScale {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("linear")
        {
            "linear" => Some(Self::Linear),
            "log" => Some(Self::Log(
                value
                    .get("log")
                    .and_then(Value::as_f64)
                    .filter(|n| *n > 1. && n.is_finite())
                    .unwrap_or(10.),
            )),
            _ => None,
        }
    }
    pub fn project(self, value: f64) -> f64 {
        match self {
            Self::Linear => value,
            Self::Log(base) if value > 0. => value.log(base),
            Self::Log(_) => f64::NAN,
        }
    }
    pub fn invert(self, value: f64) -> f64 {
        match self {
            Self::Linear => value,
            Self::Log(base) => base.powf(value),
        }
    }
}

/// Positive and negative values stack independently, by group, mode and axis.
/// Missing samples keep a gap without poisoning later series in the group.
pub fn stack(
    values: &[f64],
    definitions: &[Stacking],
    right_axes: &[bool],
) -> (Vec<f64>, Vec<f64>) {
    let mut tops = values.to_vec();
    let mut bases = vec![0.; values.len()];
    for (i, (value, definition)) in values.iter().zip(definitions).enumerate() {
        if definition.mode == StackMode::None || !value.is_finite() {
            continue;
        }
        let same = |j: usize| {
            definitions[j] == *definition
                && right_axes[j] == right_axes[i]
                && values[j].is_sign_negative() == value.is_sign_negative()
        };
        let total = if definition.mode == StackMode::Percent {
            values
                .iter()
                .enumerate()
                .filter(|(j, v)| same(*j) && v.is_finite())
                .map(|(_, v)| v.abs())
                .sum::<f64>()
        } else {
            100.
        };
        let factor = if total > 0. { 100. / total } else { 0. };
        bases[i] = values[..i]
            .iter()
            .enumerate()
            .filter(|(j, v)| same(*j) && v.is_finite())
            .map(|(_, v)| v * factor)
            .sum();
        tops[i] = bases[i] + value * factor;
    }
    (tops, bases)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_axes_signs_and_missing_samples_are_independent() {
        let a = Stacking::new(StackMode::Normal, "A");
        let b = Stacking::new(StackMode::Normal, "B");
        let defs = vec![
            a.clone(),
            a.clone(),
            a.clone(),
            a.clone(),
            b,
            a.clone(),
            Stacking::default(),
            a,
        ];
        let (tops, bases) = stack(
            &[2., 3., -4., -5., 7., 8., 99., f64::NAN],
            &defs,
            &[false, false, false, false, false, true, false, false],
        );
        assert_eq!(&tops[..7], &[2., 5., -4., -9., 7., 8., 99.]);
        assert_eq!(&bases[..7], &[0., 2., 0., -4., 0., 0., 0.]);
        assert!(tops[7].is_nan());
    }
    #[test]
    fn percent_stacks_normalize_each_sign_and_axis_and_handle_zero() {
        let defs = vec![Stacking::new(StackMode::Percent, "A"); 6];
        let (tops, bases) = stack(
            &[1., 3., -2., -6., 0., 4.],
            &defs,
            &[false, false, false, false, true, true],
        );
        assert_eq!(tops, [25., 100., -25., -100., 0., 100.]);
        assert_eq!(bases, [0., 25., 0., -25., 0., 0.]);
        assert_eq!(stack(&[0., 0.], &defs[..2], &[false, false]).0, [0., 0.]);
    }
}
