//! Geographic layers and Web Mercator geometry. The bundled country outlines
//! are Natural Earth 1:110m admin-0 data, stripped to exterior rings and rounded.
//! Source: https://github.com/nvkelso/natural-earth-vector/blob/master/geojson/ne_110m_admin_0_countries.geojson
//! Public domain: https://www.naturalearthdata.com/about/terms-of-use/
use crate::{
    color::{self, Rgba},
    data::Frame,
    spec::FieldSpec,
    transform::{self, Cell},
};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    pub latitude: f64,
    pub longitude: f64,
    pub zoom: f64,
    pub fit: bool,
    pub show_zoom: bool,
    pub wheel_zoom: bool,
    pub tooltip: bool,
    pub basemap: bool,
    pub layers: Vec<Layer>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            latitude: 0.,
            longitude: 0.,
            zoom: 1.,
            fit: false,
            show_zoom: true,
            wheel_zoom: false,
            tooltip: true,
            basemap: true,
            layers: vec![Layer::default()],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Markers,
    Heat,
}
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Layer {
    pub kind: LayerKind,
    pub name: String,
    pub query: Option<String>,
    pub latitude: Option<String>,
    pub longitude: Option<String>,
    pub color_field: Option<String>,
    pub color: Rgba,
    pub size_field: Option<String>,
    pub size_min: f32,
    pub size_max: f32,
    pub size_fixed: f32,
    pub opacity: f32,
    pub blur: f32,
    pub weight: f64,
    pub tooltip: bool,
    pub legend: bool,
}
impl Default for Layer {
    fn default() -> Self {
        Self {
            kind: LayerKind::Markers,
            name: "Locations".into(),
            query: None,
            latitude: None,
            longitude: None,
            color_field: None,
            color: color::GREEN,
            size_field: None,
            size_min: 2.,
            size_max: 20.,
            size_fixed: 5.,
            opacity: 0.8,
            blur: 10.,
            weight: 1.,
            tooltip: true,
            legend: false,
        }
    }
}
fn number(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}
fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}
pub(crate) fn parse(raw: &Value, ignored: &mut Vec<String>) -> Options {
    let mut o = Options {
        latitude: number(raw.pointer("/view/lat"))
            .unwrap_or(0.)
            .clamp(-85., 85.),
        longitude: number(raw.pointer("/view/lon"))
            .unwrap_or(0.)
            .clamp(-180., 180.),
        zoom: number(raw.pointer("/view/zoom"))
            .unwrap_or(1.)
            .clamp(0., 12.),
        basemap: raw.pointer("/basemap/type").and_then(Value::as_str) != Some("none"),
        fit: matches!(raw.pointer("/view/id").and_then(Value::as_str), Some("fit")),
        show_zoom: raw
            .pointer("/controls/showZoom")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        wheel_zoom: raw
            .pointer("/controls/mouseWheelZoom")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        tooltip: raw.pointer("/tooltip/mode").and_then(Value::as_str) != Some("none"),
        ..Options::default()
    };
    if raw.pointer("/view/shared").and_then(Value::as_bool) == Some(true) {
        ignored.push("geomap shared view".into());
    }
    if let Some(kind) = raw.pointer("/basemap/type").and_then(Value::as_str)
        && !matches!(kind, "default" | "none")
    {
        ignored.push(format!("geomap basemap {kind} (using offline geography)"));
    }
    if let Some(layers) = raw.get("layers").and_then(Value::as_array) {
        o.layers.clear();
        for value in layers {
            let mut l = Layer::default();
            l.kind = match value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("markers")
            {
                "markers" => LayerKind::Markers,
                "heatmap" => LayerKind::Heat,
                kind => {
                    ignored.push(format!("geomap layer {kind}"));
                    continue;
                }
            };
            l.name = text(value.get("name")).unwrap_or(l.name);
            if value.pointer("/filterData/id").and_then(Value::as_str) == Some("byRefId") {
                l.query = text(value.pointer("/filterData/options"));
            }
            match value
                .pointer("/location/mode")
                .and_then(Value::as_str)
                .unwrap_or("auto")
            {
                "auto" => {}
                "coords" => {
                    l.latitude = text(value.pointer("/location/latitude"));
                    l.longitude = text(value.pointer("/location/longitude"));
                }
                mode => {
                    ignored.push(format!("geomap location {mode}"));
                    continue;
                }
            }
            l.tooltip = value
                .get("tooltip")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let config = value.get("config").unwrap_or(&Value::Null);
            if l.kind == LayerKind::Heat {
                l.size_fixed = number(config.get("radius")).unwrap_or(8.).clamp(1., 100.) as f32;
                l.blur = number(config.get("blur")).unwrap_or(10.).clamp(0., 100.) as f32;
                l.color_field = text(config.pointer("/weight/field"));
                l.weight = number(config.pointer("/weight/fixed"))
                    .unwrap_or(1.)
                    .max(0.);
                l.opacity = number(value.get("opacity")).unwrap_or(0.8).clamp(0., 1.) as f32;
            } else {
                // Older exports put marker styles directly in the layer config.
                let style = config.get("style").unwrap_or(config);
                l.color_field = text(style.pointer("/color/field"));
                l.color = text(style.pointer("/color/fixed"))
                    .as_deref()
                    .and_then(color::parse)
                    .unwrap_or(l.color);
                l.size_field = text(style.pointer("/size/field"));
                l.size_fixed = number(style.pointer("/size/fixed"))
                    .unwrap_or(5.)
                    .clamp(1., 100.) as f32;
                l.size_min = number(style.pointer("/size/min"))
                    .unwrap_or(2.)
                    .clamp(1., 100.) as f32;
                l.size_max = number(style.pointer("/size/max"))
                    .unwrap_or(20.)
                    .clamp(f64::from(l.size_min), 100.) as f32;
                l.opacity = number(style.get("opacity").or_else(|| style.get("fillOpacity")))
                    .unwrap_or(0.8)
                    .clamp(0., 1.) as f32;
                l.legend = config
                    .get("showLegend")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(shape) = style.get("shape").and_then(Value::as_str)
                    && shape != "circle"
                {
                    ignored.push(format!("geomap shape {shape}"));
                }
                if style
                    .pointer("/rotation/field")
                    .and_then(Value::as_str)
                    .is_some()
                    || number(style.pointer("/rotation/fixed")).is_some_and(|r| r != 0.)
                {
                    ignored.push("geomap marker rotation".into());
                }
                if style
                    .pointer("/text/field")
                    .and_then(Value::as_str)
                    .is_some()
                    || style
                        .pointer("/text/fixed")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                {
                    ignored.push("geomap marker text".into());
                }
                if let Some(symbol) = style.pointer("/symbol/fixed").and_then(Value::as_str)
                    && !symbol.ends_with("circle.svg")
                {
                    ignored.push(format!("geomap symbol {symbol}"));
                }
            }
            o.layers.push(l);
        }
    }
    o
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    pub label: String,
    pub value: f64,
    pub weight: f32,
    pub text: String,
    pub color: Rgba,
    pub radius: f32,
    pub details: Vec<(String, String)>,
}
impl Location {
    pub fn new(latitude: f64, longitude: f64, value: f64) -> Self {
        Self {
            latitude,
            longitude,
            value,
            label: format!("{latitude:.2}, {longitude:.2}"),
            weight: 1.,
            text: value.to_string(),
            color: color::GREEN,
            radius: 5.,
            details: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct MapData {
    pub layers: Vec<Vec<Location>>,
}
impl MapData {
    pub fn from_frame(frame: &Frame, options: &Options, field: &FieldSpec) -> Self {
        let mut layers = Vec::new();
        for layer in &options.layers {
            let series: Vec<_> = frame
                .series
                .iter()
                .filter(|s| layer.query.as_ref().is_none_or(|q| *q == s.query))
                .cloned()
                .collect();
            let queries: Vec<_> = series.iter().map(|s| s.query.as_str()).collect();
            let table = transform::table(&[], &series, &queries);
            let fields: Vec<_> = (0..table.columns.len())
                .map(|i| {
                    let values: Vec<_> = table
                        .rows
                        .iter()
                        .filter_map(|row| match row[i] {
                            Cell::Number(value) => Some(value),
                            _ => None,
                        })
                        .collect();
                    let context = table.field_context(i).values(&values);
                    field.for_field(&context).into_owned()
                })
                .collect();
            let column = |name: Option<&str>, auto: &[&str]| {
                table.columns.iter().position(|n| {
                    name.map_or_else(
                        || auto.iter().any(|a| n.eq_ignore_ascii_case(a)),
                        |name| name == n,
                    )
                })
            };
            let (Some(lat), Some(lon)) = (
                column(layer.latitude.as_deref(), &["latitude", "lat"]),
                column(layer.longitude.as_deref(), &["longitude", "lon", "lng"]),
            ) else {
                layers.push(Vec::new());
                continue;
            };
            let numeric = |row: &[Cell], i: usize| match &row[i] {
                Cell::Number(v) => *v,
                Cell::Text(v) => v.parse().unwrap_or(f64::NAN),
            };
            let value_col = column(layer.color_field.as_deref(), &[]);
            let size_col = column(layer.size_field.as_deref(), &[]);
            let range = |col: Option<usize>| {
                let values: Vec<_> = col
                    .into_iter()
                    .flat_map(|i| table.rows.iter().map(move |r| numeric(r, i)))
                    .filter(|v| v.is_finite())
                    .collect();
                let f = col.map(|i| &fields[i]).unwrap_or(field);
                (
                    f.min
                        .unwrap_or(values.iter().copied().reduce(f64::min).unwrap_or(0.)),
                    f.max
                        .unwrap_or(values.iter().copied().reduce(f64::max).unwrap_or(1.)),
                )
            };
            let color_range = range(value_col);
            let size_range = range(size_col);
            let fraction = |v: f64, (min, max): (f64, f64)| {
                if max > min {
                    ((v - min) / (max - min)).clamp(0., 1.)
                } else {
                    0.5
                }
            };
            let mut locations = Vec::new();
            for row in &table.rows {
                let (latitude, longitude) = (numeric(row, lat), numeric(row, lon));
                if !latitude.is_finite()
                    || !longitude.is_finite()
                    || latitude.abs() > 90.
                    || longitude.abs() > 180.
                {
                    continue;
                }
                let value = value_col.map(|i| numeric(row, i)).unwrap_or(layer.weight);
                let value_field = value_col.map(|i| &fields[i]).unwrap_or(field);
                let details: Vec<_> = row
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != lat && *i != lon)
                    .map(|(i, c)| {
                        (
                            table.columns[i].clone(),
                            match c {
                                Cell::Text(s) => s.clone(),
                                Cell::Number(v) => fields[i].format(*v),
                            },
                        )
                    })
                    .collect();
                let label = details
                    .iter()
                    .find(|(_, s)| !s.is_empty())
                    .map(|(_, s)| s.clone())
                    .unwrap_or_else(|| format!("{latitude:.2}, {longitude:.2}"));
                locations.push(Location {
                    latitude,
                    longitude,
                    label,
                    value,
                    weight: if layer.kind == LayerKind::Heat && value_col.is_some() {
                        fraction(value, color_range) as f32
                    } else {
                        layer.weight as f32
                    },
                    text: value_field.format(value),
                    details,
                    radius: size_col.map_or(layer.size_fixed, |i| {
                        layer.size_min
                            + fraction(numeric(row, i), size_range) as f32
                                * (layer.size_max - layer.size_min)
                    }),
                    color: if layer.kind == LayerKind::Heat {
                        crate::heatmap::Palette::Turbo.sample(fraction(value, color_range))
                    } else if value_col.is_some() {
                        value_field.series_color_in_range(0, value, color_range)
                    } else {
                        layer.color
                    },
                });
            }
            layers.push(locations);
        }
        Self { layers }
    }
}

pub fn mercator(latitude: f64, longitude: f64) -> (f64, f64) {
    let lat = latitude.clamp(-85.05112878, 85.05112878).to_radians();
    (
        (longitude + 180.) / 360.,
        (1. - (lat.tan() + 1. / lat.cos()).ln() / std::f64::consts::PI) / 2.,
    )
}
pub fn unproject(x: f64, y: f64) -> (f64, f64) {
    (
        (std::f64::consts::PI * (1. - 2. * y))
            .sinh()
            .atan()
            .to_degrees(),
        (x * 360. - 180. + 180.).rem_euclid(360.) - 180.,
    )
}
pub fn world_outline() -> &'static Vec<Vec<(f64, f64)>> {
    static OUTLINE: OnceLock<Vec<Vec<(f64, f64)>>> = OnceLock::new();
    OUTLINE.get_or_init(|| {
        serde_json::from_str::<Vec<Vec<(f64, f64)>>>(include_str!("../assets/world-outline.json"))
            .expect("bundled Natural Earth coordinates")
            .into_iter()
            .map(|ring| {
                ring.into_iter()
                    .map(|(lon, lat)| mercator(lat, lon))
                    .collect()
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_marker_heat_coordinates_and_offline_fallback() {
        let raw = serde_json::json!({"view":{"lat":35,"lon":100,"zoom":4.5},"basemap":{"type":"xyz"},"controls":{"mouseWheelZoom":true},"layers":[
            {"name":"Response","type":"markers","location":{"mode":"coords","latitude":"y","longitude":"x"},"filterData":{"id":"byRefId","options":"A"},"config":{"showLegend":true,"style":{"color":{"field":"response"},"size":{"field":"response","min":2,"max":20},"opacity":0.6}}},
            {"type":"heatmap","config":{"weight":{"field":"PV"},"radius":8,"blur":10},"opacity":0.7}
        ]});
        let mut ignored = Vec::new();
        let options = parse(&raw, &mut ignored);
        assert_eq!(
            (options.latitude, options.longitude, options.zoom),
            (35., 100., 4.5)
        );
        assert!(options.wheel_zoom && options.layers[0].legend);
        assert_eq!(options.layers[0].latitude.as_deref(), Some("y"));
        assert_eq!(options.layers[0].query.as_deref(), Some("A"));
        assert_eq!(options.layers[1].kind, LayerKind::Heat);
        assert_eq!(options.layers[1].color_field.as_deref(), Some("PV"));
        assert_eq!(ignored, ["geomap basemap xyz (using offline geography)"]);
    }
    #[test]
    fn mercator_round_trips_and_clamps_poles() {
        for (lat, lon) in [(0., 0.), (39.904, 116.407), (-33.868, 151.209)] {
            let (x, y) = mercator(lat, lon);
            let (latitude, longitude) = unproject(x, y);
            assert!((latitude - lat).abs() < 1e-8 && (longitude - lon).abs() < 1e-8);
        }
        assert!(mercator(90., 0.).1.is_finite());
        assert_eq!(unproject(1.5, 0.5).1, 0.);
        assert!(world_outline().len() > 100);
    }
    #[test]
    fn layers_filter_queries_and_reject_invalid_coordinates() {
        use crate::data::Series;
        let mut frame = Frame::default();
        for (query, city, lat, lon, value) in [
            ("A", "Valid", 35., 110., 80.),
            ("A", "Invalid", 95., 110., 10.),
            ("B", "Other", 30., 115., 20.),
        ] {
            for (field, value) in [("lat", lat), ("lon", lon), ("metric", value)] {
                frame.series.push(Series {
                    name: city.into(),
                    query: query.into(),
                    field: Some(field.into()),
                    labels: vec![("city".into(), city.into())],
                    values: vec![value],
                });
            }
        }
        let layer = Layer {
            query: Some("A".into()),
            color_field: Some("metric".into()),
            size_field: Some("metric".into()),
            ..Layer::default()
        };
        let options = Options {
            layers: vec![layer],
            ..Options::default()
        };
        let field = crate::Dashboard::parse(r#"{"panels":[{"type":"geomap","fieldConfig":{"defaults":{"min":0,"max":100,"color":{"mode":"thresholds"},"thresholds":{"steps":[{"color":"green","value":null},{"color":"red","value":50}]}}}}]}"#).unwrap().panels.remove(0).field;
        let data = MapData::from_frame(&frame, &options, &field);
        assert_eq!(data.layers[0].len(), 1);
        let location = &data.layers[0][0];
        assert_eq!(location.label, "Valid");
        assert_eq!(location.value, 80.);
        assert_eq!(location.color, color::parse("red").unwrap());
        assert!((location.radius - 16.4).abs() < 0.001);
    }
    #[test]
    fn legacy_marker_styles_parse_and_undrawn_options_remain_reported() {
        let mut ignored = Vec::new();
        let options = parse(
            &serde_json::json!({"view":{"shared":true},"layers":[{"type":"markers","config":{
                "color":{"field":"Price","fixed":"red"},"size":{"field":"Count","min":3,"max":15},
                "fillOpacity":0.4,"shape":"star","showLegend":true
            }}]}),
            &mut ignored,
        );
        let layer = &options.layers[0];
        assert_eq!(layer.color_field.as_deref(), Some("Price"));
        assert_eq!(layer.size_field.as_deref(), Some("Count"));
        assert_eq!(layer.color, color::parse("red").unwrap());
        assert_eq!(
            (layer.size_min, layer.size_max, layer.opacity),
            (3., 15., 0.4)
        );
        assert!(layer.legend);
        assert_eq!(ignored, ["geomap shared view", "geomap shape star"]);
    }
}
