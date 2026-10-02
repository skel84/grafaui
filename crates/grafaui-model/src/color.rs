//! Grafana's color names, the classic series palette and threshold lookup.

/// A color as `0xRRGGBBAA`. The desktop crate turns it into a GPUI color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba(pub u32);

impl Rgba {
    pub const fn rgb(hex: u32) -> Self {
        Self((hex << 8) | 0xff)
    }

    pub fn red(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub fn green(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn blue(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn alpha(self) -> u8 {
        self.0 as u8
    }
}

/// Grafana's "classic" palette, which series take in order when a panel
/// uses the default `palette-classic` color mode.
pub const CLASSIC: [Rgba; 16] = [
    Rgba::rgb(0x7EB26D),
    Rgba::rgb(0xEAB839),
    Rgba::rgb(0x6ED0E0),
    Rgba::rgb(0xEF843C),
    Rgba::rgb(0xE24D42),
    Rgba::rgb(0x1F78C1),
    Rgba::rgb(0xBA43A9),
    Rgba::rgb(0x705DA0),
    Rgba::rgb(0x508642),
    Rgba::rgb(0xCCA300),
    Rgba::rgb(0x447EBC),
    Rgba::rgb(0xC15C17),
    Rgba::rgb(0x890F02),
    Rgba::rgb(0x0A437C),
    Rgba::rgb(0x6D1F62),
    Rgba::rgb(0x584477),
];

pub fn classic(index: usize) -> Rgba {
    CLASSIC[index % CLASSIC.len()]
}

/// Value-based palettes used by Grafana's field color schemes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    GreenYellowRed,
    RedYellowGreen,
    BluePurple,
    YellowRed,
}

impl ColorScheme {
    pub(crate) fn parse(mode: &str) -> Option<Self> {
        Some(match mode {
            "continuous-GrYlRd" => Self::GreenYellowRed,
            "continuous-RdYlGr" => Self::RedYellowGreen,
            "continuous-BlPu" => Self::BluePurple,
            "continuous-YlRd" => Self::YellowRed,
            _ => return None,
        })
    }

    pub fn colors(self) -> &'static [Rgba] {
        const GR_YL_RD: &[Rgba] = &[
            Rgba::rgb(0x73BF69),
            Rgba::rgb(0xFADE2A),
            Rgba::rgb(0xF2495C),
        ];
        const RD_YL_GR: &[Rgba] = &[
            Rgba::rgb(0xF2495C),
            Rgba::rgb(0xFADE2A),
            Rgba::rgb(0x73BF69),
        ];
        const BL_PU: &[Rgba] = &[Rgba::rgb(0x5794F2), Rgba::rgb(0xB877D9)];
        const YL_RD: &[Rgba] = &[Rgba::rgb(0xFFF899), Rgba::rgb(0xC4162A)];
        match self {
            Self::GreenYellowRed => GR_YL_RD,
            Self::RedYellowGreen => RD_YL_GR,
            Self::BluePurple => BL_PU,
            Self::YellowRed => YL_RD,
        }
    }

    /// Grafana's continuous field colors use an RGB basis spline.
    pub fn sample(self, fraction: f64) -> Rgba {
        let colors = self.colors();
        let fraction = if fraction.is_nan() {
            0.
        } else {
            fraction.clamp(0., 1.)
        };
        let t = fraction * (colors.len() - 1) as f64;
        let i = (t.floor() as usize).min(colors.len() - 2);
        let t = t - i as f64;
        let channel = |get: fn(Rgba) -> u8| {
            let v1 = f64::from(get(colors[i]));
            let v2 = f64::from(get(colors[i + 1]));
            let v0 = if i > 0 {
                f64::from(get(colors[i - 1]))
            } else {
                2. * v1 - v2
            };
            let v3 = if i + 2 < colors.len() {
                f64::from(get(colors[i + 2]))
            } else {
                2. * v2 - v1
            };
            let t2 = t * t;
            let t3 = t2 * t;
            (((1. - 3. * t + 3. * t2 - t3) * v0
                + (4. - 6. * t2 + 3. * t3) * v1
                + (1. + 3. * t + 3. * t2 - 3. * t3) * v2
                + t3 * v3)
                / 6.)
                .round()
                .clamp(0., 255.) as u32
        };
        Rgba::rgb(channel(Rgba::red) << 16 | channel(Rgba::green) << 8 | channel(Rgba::blue))
    }
}

/// Linear interpolation between neighboring palette stops, as used by
/// Grafana's chart and gauge gradients.
pub fn interpolate(colors: &[Rgba], fraction: f64) -> Rgba {
    if colors.is_empty() {
        return GREEN;
    }
    let fraction = if fraction.is_nan() {
        0.
    } else {
        fraction.clamp(0., 1.)
    };
    let t = fraction * (colors.len() - 1) as f64;
    let i = (t.floor() as usize).min(colors.len() - 1);
    let a = colors[i];
    let b = colors[(i + 1).min(colors.len() - 1)];
    let mix = |a: u8, b: u8| {
        (f64::from(a) + (f64::from(b) - f64::from(a)) * (t - i as f64)).round() as u32
    };
    Rgba(
        mix(a.red(), b.red()) << 24
            | mix(a.green(), b.green()) << 16
            | mix(a.blue(), b.blue()) << 8
            | mix(a.alpha(), b.alpha()),
    )
}

pub const GREEN: Rgba = Rgba::rgb(0x73BF69);
pub const TEXT: Rgba = Rgba::rgb(0xCCCCDC);

/// Grafana's named colors (dark theme), with their shades.
const NAMED: &[(&str, u32)] = &[
    ("red", 0xF2495C),
    ("semi-dark-red", 0xE02F44),
    ("dark-red", 0xC4162A),
    ("light-red", 0xFF7383),
    ("super-light-red", 0xFFA6B0),
    ("orange", 0xFF9830),
    ("semi-dark-orange", 0xFF780A),
    ("dark-orange", 0xFA6400),
    ("light-orange", 0xFFB357),
    ("super-light-orange", 0xFFCB7D),
    ("yellow", 0xFADE2A),
    ("semi-dark-yellow", 0xF2CC0C),
    ("dark-yellow", 0xE0B400),
    ("light-yellow", 0xFFEE52),
    ("super-light-yellow", 0xFFF899),
    ("green", 0x73BF69),
    ("semi-dark-green", 0x56A64B),
    ("dark-green", 0x37872D),
    ("light-green", 0x96D98D),
    ("super-light-green", 0xC8F2C2),
    ("blue", 0x5794F2),
    ("semi-dark-blue", 0x3274D9),
    ("dark-blue", 0x1F60C4),
    ("light-blue", 0x8AB8FF),
    ("super-light-blue", 0xC0D8FF),
    ("purple", 0xB877D9),
    ("semi-dark-purple", 0xA352CC),
    ("dark-purple", 0x8F3BB8),
    ("light-purple", 0xCA95E5),
    ("super-light-purple", 0xDEB6F2),
    ("text", 0xCCCCDC),
    ("white", 0xFFFFFF),
    ("black", 0x000000),
];

/// Reads a Grafana color: a name (`green`, `dark-red`), `#rgb`, `#rrggbb`,
/// `#rrggbbaa`, `rgb(…)` or `rgba(…)`. Anything else is `None`.
pub fn parse(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("transparent") {
        return Some(Rgba(0));
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(args) = text
        .strip_prefix("rgba(")
        .or_else(|| text.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<f64> = args
            .split(',')
            .map(|part| part.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .ok()?;
        let channel = |v: f64| v.clamp(0., 255.).round() as u32;
        return match parts.as_slice() {
            [r, g, b] => Some(Rgba::rgb(
                channel(*r) << 16 | channel(*g) << 8 | channel(*b),
            )),
            [r, g, b, a] => Some(Rgba(
                channel(*r) << 24
                    | channel(*g) << 16
                    | channel(*b) << 8
                    | channel(a.clamp(0., 1.) * 255.),
            )),
            _ => None,
        };
    }
    let lower = text.to_ascii_lowercase();
    NAMED
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, hex)| Rgba::rgb(*hex))
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let value = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        3 => {
            let (r, g, b) = ((value >> 8) & 0xf, (value >> 4) & 0xf, value & 0xf);
            Some(Rgba::rgb((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11)))
        }
        6 => Some(Rgba::rgb(value)),
        8 => Some(Rgba(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_palettes_interpolate_and_clamp() {
        for scheme in [
            ColorScheme::GreenYellowRed,
            ColorScheme::RedYellowGreen,
            ColorScheme::BluePurple,
            ColorScheme::YellowRed,
        ] {
            assert_eq!(scheme.sample(-1.), scheme.colors()[0]);
            assert_eq!(scheme.sample(f64::NAN), scheme.colors()[0]);
            assert_eq!(scheme.sample(2.), *scheme.colors().last().unwrap());
        }
        assert_eq!(ColorScheme::GreenYellowRed.sample(0.5), Rgba::rgb(0xE2C03D));
        assert_eq!(ColorScheme::BluePurple.sample(0.5), Rgba::rgb(0x8886E6));
        assert_eq!(
            interpolate(&[Rgba(0), Rgba::rgb(0xFFFFFF)], 0.5),
            Rgba(0x80808080)
        );
    }

    #[test]
    fn reads_every_spelling() {
        assert_eq!(parse("green"), Some(Rgba::rgb(0x73BF69)));
        assert_eq!(parse("Dark-Red"), Some(Rgba::rgb(0xC4162A)));
        assert_eq!(parse("#fff"), Some(Rgba::rgb(0xFFFFFF)));
        assert_eq!(parse("#EAB839"), Some(Rgba::rgb(0xEAB839)));
        assert_eq!(parse("#11223344"), Some(Rgba(0x11223344)));
        assert_eq!(
            parse("rgba(50, 172, 45, 0.97)"),
            Some(Rgba(0x32AC2D00 | 247))
        );
        assert_eq!(parse("rgb(0,0,255)"), Some(Rgba::rgb(0x0000FF)));
        assert_eq!(parse("nonsense"), None);
    }
}
