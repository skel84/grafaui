//! One Prometheus HTTP API call as data: a path under `api/v1/` and its
//! parameters. Any transport can send it, as a GET with a query string or a
//! POST form; `Client` is one such transport.
use crate::Result;

/// The longest GET target (path and query string) this crate builds. Proxies
/// and API servers commonly cap a request line near 8 KiB; a longer request is
/// refused here rather than cut or rejected obscurely on the way.
pub const MAX_GET_TARGET: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiRequest {
    /// Relative to `api/v1/`, such as `query_range` or `label/job/values`.
    pub path: String,
    pub params: Vec<(String, String)>,
}

impl ApiRequest {
    pub fn new(path: impl Into<String>, params: Vec<(String, String)>) -> Self {
        Self {
            path: path.into(),
            params,
        }
    }

    /// The form body for a POST, `application/x-www-form-urlencoded`.
    pub fn form(&self) -> String {
        encode(&self.params)
    }

    /// `api/v1/<path>?<query>`, for a GET relative to the Prometheus base.
    /// Errors when the target would pass [`MAX_GET_TARGET`] bytes.
    pub fn get_target(&self) -> Result<String> {
        let query = encode(&self.params);
        let target = if query.is_empty() {
            format!("api/v1/{}", self.path)
        } else {
            format!("api/v1/{}?{query}", self.path)
        };
        if target.len() > MAX_GET_TARGET {
            return Err(format!(
                "query is too long to send as a GET ({} KiB; the limit is 8 KiB)",
                target.len().div_ceil(1024)
            ));
        }
        Ok(target)
    }
}

fn encode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", escape(key), escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes everything but RFC 3986's unreserved characters, so the
/// result is valid both in a query string and in a form body.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_target_encodes_promql_and_matchers() {
        let request = ApiRequest::new(
            "query",
            vec![
                (
                    "query".into(),
                    r#"sum by (pod) (rate(x{a=~"b|c"}[5m]))"#.into(),
                ),
                ("time".into(), "1000".into()),
                ("match[]".into(), "up ü".into()),
            ],
        );
        assert_eq!(
            request.get_target().unwrap(),
            "api/v1/query?query=sum%20by%20%28pod%29%20%28rate%28x%7Ba%3D~%22b%7Cc%22%7D%5B5m%5D%29%29&time=1000&match%5B%5D=up%20%C3%BC"
        );
        assert_eq!(request.form(), request.get_target().unwrap()[13..]);
        assert_eq!(
            ApiRequest::new("labels", vec![]).get_target().unwrap(),
            "api/v1/labels"
        );
    }

    #[test]
    fn get_target_refuses_past_eight_kib_instead_of_cutting() {
        let fits = ApiRequest::new("query", vec![("query".into(), "a".repeat(8_000))]);
        assert!(fits.get_target().is_ok());
        let long = ApiRequest::new("query", vec![("query".into(), "a".repeat(8_200))]);
        let error = long.get_target().unwrap_err();
        assert!(error.contains("too long"), "{error}");
        assert!(error.contains("8 KiB"), "{error}");
    }
}
