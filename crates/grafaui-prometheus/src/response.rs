//! Pure Prometheus JSON conversion. Timestamps are seconds with millisecond
//! precision; the shared time axis is a union, with NaN for absent samples.
use std::collections::BTreeSet;

use grafaui_model::data::{Frame, Series};
use regex::{Captures, Regex};
use serde_json::Value;

use crate::Result;
use crate::query::{QueryKind, Request};

pub fn envelope(value: &Value) -> Result<(&Value, Vec<String>)> {
    if value.get("status").and_then(Value::as_str) != Some("success") {
        let kind = value
            .get("errorType")
            .and_then(Value::as_str)
            .unwrap_or("API error");
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("invalid Prometheus response envelope");
        return Err(format!("{kind}: {error}"));
    }
    let data = value.get("data").ok_or("Prometheus response has no data")?;
    let warnings = ["warnings", "infos"]
        .into_iter()
        .flat_map(|key| {
            value
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    Ok((data, warnings))
}

pub fn metric_name(labels: &[(String, String)]) -> String {
    let name = labels
        .iter()
        .find(|(k, _)| k == "__name__")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let labels = labels
        .iter()
        .filter(|(k, _)| k != "__name__")
        .map(|(k, v)| format!("{k}={}", serde_json::to_string(v).unwrap()))
        .collect::<Vec<_>>()
        .join(", ");
    if labels.is_empty() && !name.is_empty() {
        name.into()
    } else {
        format!("{name}{{{labels}}}")
    }
}

fn legend(template: Option<&str>, labels: &[(String, String)]) -> String {
    let Some(template) = template else {
        return metric_name(labels);
    };
    Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}")
        .unwrap()
        .replace_all(template, |c: &Captures| {
            // Grafana uses the label name itself when the series lacks that label.
            labels
                .iter()
                .find(|(k, _)| k == &c[1])
                .map(|(_, v)| v.as_str())
                .unwrap_or(&c[1])
                .to_owned()
        })
        .into_owned()
}

/// Prometheus stores timestamps at millisecond precision. Integer keys avoid
/// float comparison surprises when aligning independently returned series.
fn timestamp(value: &Value) -> Result<i64> {
    let seconds = value.as_f64().ok_or("sample timestamp is not numeric")?;
    if !seconds.is_finite() || seconds.abs() > 9e12 {
        return Err("invalid sample timestamp".into());
    }
    Ok((seconds * 1000.).round() as i64)
}

fn sample(value: &Value) -> Result<(i64, f64)> {
    let pair = value
        .as_array()
        .filter(|p| p.len() == 2)
        .ok_or("invalid Prometheus sample")?;
    let time = timestamp(&pair[0])?;
    let text = pair[1].as_str().ok_or("sample value is not a string")?;
    let number = match text {
        "NaN" => f64::NAN,
        "+Inf" | "Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        _ => text
            .parse::<f64>()
            .map_err(|_| format!("invalid sample value {text:?}"))?,
    };
    Ok((time, number))
}

pub fn convert(value: &Value, request: &Request) -> Result<(Frame, Vec<String>)> {
    let (data, warnings) = envelope(value)?;
    let kind = data
        .get("resultType")
        .and_then(Value::as_str)
        .ok_or("missing resultType")?;
    let result = data.get("result").ok_or("missing result")?;
    let mut times = BTreeSet::new();
    if let QueryKind::Range { start, end, step } = request.kind() {
        let count = ((end - start) / step).round() as usize + 1;
        if count > 10_002 {
            return Err("range request exceeds 10,000 samples per series".into());
        }
        for i in 0..count {
            times.insert(((start + i as f64 * step) * 1000.).round() as i64);
        }
    }
    let mut series_samples = Vec::new();
    match kind {
        "matrix" | "vector" => {
            for entry in result.as_array().ok_or("result is not an array")? {
                if entry.get("histogram").is_some() || entry.get("histograms").is_some() {
                    return Err("native histogram samples are unsupported; query classic buckets or histogram_count/sum".into());
                }
                let mut labels = entry
                    .get("metric")
                    .and_then(Value::as_object)
                    .ok_or("missing metric labels")?
                    .iter()
                    .map(|(k, v)| {
                        Ok((
                            k.clone(),
                            v.as_str().ok_or("metric label is not a string")?.to_owned(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                labels.sort();
                let samples = if kind == "matrix" {
                    entry
                        .get("values")
                        .and_then(Value::as_array)
                        .ok_or("matrix has no values")?
                        .iter()
                        .map(sample)
                        .collect::<Result<Vec<_>>>()?
                } else {
                    vec![sample(entry.get("value").ok_or("vector has no value")?)?]
                };
                times.extend(samples.iter().map(|(t, _)| *t));
                series_samples.push((labels, samples));
            }
        }
        "scalar" => {
            let sample = sample(result)?;
            times.insert(sample.0);
            series_samples.push((Vec::new(), vec![sample]));
        }
        other => return Err(format!("unsupported Prometheus result type {other}")),
    }
    let times: Vec<_> = times.into_iter().collect();
    let series = series_samples
        .into_iter()
        .map(|(labels, samples)| {
            let mut values = vec![f64::NAN; times.len()];
            for (t, v) in samples {
                values[times.binary_search(&t).unwrap()] = v;
            }
            let name = if labels.is_empty() && request.legend().is_none() {
                request.query().to_owned()
            } else {
                legend(request.legend(), &labels)
            };
            Series {
                name,
                query: request.ref_id().into(),
                field: None,
                labels,
                values,
            }
        })
        .collect();
    Ok((
        Frame {
            times: times.into_iter().map(|t| t as f64 / 1000.).collect(),
            series,
        },
        warnings,
    ))
}

/// Align multiple targets/modes onto one axis without filling missing values.
pub fn merge(frames: Vec<Frame>) -> Frame {
    let times: BTreeSet<_> = frames
        .iter()
        .flat_map(|f| f.times.iter().map(|t| (t * 1000.).round() as i64))
        .collect();
    let times: Vec<_> = times.into_iter().collect();
    let mut series = Vec::new();
    for frame in frames {
        let slots: Vec<_> = frame
            .times
            .iter()
            .map(|t| times.binary_search(&((t * 1000.).round() as i64)).unwrap())
            .collect();
        for mut s in frame.series {
            let mut values = vec![f64::NAN; times.len()];
            for (slot, value) in slots.iter().zip(s.values) {
                values[*slot] = value;
            }
            s.values = values;
            series.push(s);
        }
    }
    Frame {
        times: times.into_iter().map(|t| t as f64 / 1000.).collect(),
        series,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{self, Variables};
    use grafaui_model::{Dashboard, data::QueryContext, time::TimeWindow};
    use serde_json::json;

    fn request(instant: bool) -> Request {
        let dashboard = Dashboard::parse(&format!(r#"{{"panels":[{{"type":"timeseries","targets":[{{"refId":"Q","expr":"up","legendFormat":"{{{{instance}}}} · {{{{missing}}}}","instant":{instant},"range":{}}}]}}]}}"#, !instant)).unwrap();
        query::requests(
            dashboard.panel(0),
            &QueryContext {
                window: TimeWindow::new(30, 30, 3),
                variables: vec![],
            },
            &Variables::default(),
            15.,
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn matrix_grid_keeps_entirely_missing_steps_and_series_gaps() {
        let (frame, notes) = convert(&json!({"status":"success","warnings":["partial"],"infos":["note"],"data":{"resultType":"matrix","result":[
            {"metric":{"instance":"a","job":"node"},"values":[[0,"1"],[30,"3"]]},
            {"metric":{"instance":"b"},"values":[[15,"NaN"],[30,"+Inf"]]}
        ]}}), &request(false)).unwrap();
        assert_eq!(frame.times, [0., 15., 30.]);
        assert_eq!(frame.series[0].values[0], 1.);
        assert!(frame.series[0].values[1].is_nan());
        assert_eq!(frame.series[0].values[2], 3.);
        assert!(frame.series[1].values[0].is_nan());
        assert!(frame.series[1].values[1].is_nan());
        assert_eq!(frame.series[1].values[2], f64::INFINITY);
        assert_eq!(frame.series[0].name, "a · missing");
        assert_eq!(frame.series[0].query, "Q");
        assert_eq!(
            frame.series[0].labels,
            [
                ("instance".into(), "a".into()),
                ("job".into(), "node".into())
            ]
        );
        assert_eq!(notes, ["partial", "note"]);
    }

    #[test]
    fn fractional_vector_and_scalar_timestamps_are_preserved_and_merged() {
        let (vector, _) = convert(
            &json!({"status":"success","data":{"resultType":"vector","result":[
                {"metric":{"instance":"a"},"value":[10.125,"5"]},
                {"metric":{"instance":"b"},"value":[10.5,"7"]}
            ]}}),
            &request(true),
        )
        .unwrap();
        let (scalar, _) = convert(
            &json!({"status":"success","data":{"resultType":"scalar","result":[10.25,"2"]}}),
            &request(true),
        )
        .unwrap();
        let merged = merge(vec![vector, scalar]);
        assert_eq!(merged.times, [10.125, 10.25, 10.5]);
        assert_eq!(merged.series[0].values[0], 5.);
        assert!(merged.series[0].values[1].is_nan());
        assert_eq!(merged.series[1].values[2], 7.);
        assert_eq!(merged.series[2].values[1], 2.);
        assert!(merged.series[2].values[0].is_nan());
    }

    #[test]
    fn rejects_errors_malformed_samples_strings_and_native_histograms() {
        for value in [
            json!({"status":"error","errorType":"bad_data","error":"bad expression"}),
            json!({"status":"success","data":{"resultType":"string","result":[0,"text"]}}),
            json!({"status":"success","data":{"resultType":"vector","result":[{"metric":{},"histogram":[0,{}]}]}}),
            json!({"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":["wrong","5"]}]}}),
            json!({"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1,"oops"]}]}}),
        ] {
            assert!(convert(&value, &request(true)).is_err(), "{value}");
        }
    }

    #[test]
    fn empty_success_is_empty_data_with_no_invented_series() {
        let (frame, _) = convert(
            &json!({"status":"success","data":{"resultType":"vector","result":[]}}),
            &request(true),
        )
        .unwrap();
        assert!(frame.series.is_empty());
        assert!(frame.times.is_empty());
        assert_eq!(
            metric_name(&[
                ("__name__".into(), "up".into()),
                ("job".into(), "nodes".into())
            ]),
            "up{job=\"nodes\"}"
        );
    }
}
