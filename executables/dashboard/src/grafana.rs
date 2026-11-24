//! Grafana calls: panel queries, dashboard import and the connection test.

use crate::configuration::{Configuration, Imported, Kind, Panel, is_valid_uid};
use crate::error::{Error, Result};
use crate::json::Reader;
use crate::net::{Connector, Endpoint, read_response, status_error};
use crate::query::batches;
use crate::response::{PanelData, parse};
use std::{format, vec::Vec};

const QUERY_PATH: &str = "/api/ds/query";

/// Runs every panel query, one request per batch, and returns the data in
/// panel order. A batch that fails as a whole fails the refresh.
pub fn query<C: Connector>(
    connector: &mut C,
    configuration: &Configuration,
    token: &str,
    panels: &[&Panel],
) -> Result<Vec<PanelData>> {
    let endpoint = Endpoint::parse(&configuration.url)?;
    let kinds: Vec<Kind> = panels.iter().map(|panel| panel.kind()).collect();
    let mut data = std::vec![PanelData::default(); panels.len()];
    for batch in batches(panels, &configuration.data_source) {
        let request = endpoint.request("POST", QUERY_PATH, token, Some(batch.body.as_bytes()));
        drop(batch.body);
        let transport = connector.send(endpoint.device(), &request)?;
        drop(request);
        let (status, body) = read_response(transport)?;
        // Grafana answers 400, or 207, when one of the queries failed, with
        // each query's result or error in the body.
        if !matches!(status, 200 | 207 | 400) {
            return Err(status_error(status));
        }
        let mut json = Reader::new(body);
        if let Err(error) = parse(&mut json, &kinds, &mut data) {
            return Err(if status == 200 {
                error
            } else {
                Error::Http(status)
            });
        }
    }
    Ok(data)
}

/// Fetches and converts the configured dashboard.
pub fn import<C: Connector>(
    connector: &mut C,
    configuration: &Configuration,
    token: &str,
) -> Result<Imported> {
    let uid = configuration
        .dashboard
        .as_deref()
        .filter(|uid| is_valid_uid(uid))
        .ok_or(Error::Configuration)?;
    let endpoint = Endpoint::parse(&configuration.url)?;
    let request = endpoint.request("GET", &format!("/api/dashboards/uid/{uid}"), token, None);
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, body) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    crate::import::parse_dashboard(&mut Reader::new(body), uid)
}

/// Runs `panel`, or `vector(1)` when there is none, to check the URL, the
/// token and the data source.
pub fn test_connection<C: Connector>(
    connector: &mut C,
    configuration: &Configuration,
    token: &str,
    panel: Option<&Panel>,
) -> Result<(Panel, PanelData)> {
    let panel = panel.cloned().unwrap_or_else(|| Panel {
        title: std::string::String::from("vector(1)"),
        expression: std::string::String::from("vector(1)"),
        ..Panel::new()
    });
    let mut data = query(connector, configuration, token, &[&panel])?;
    let data = data.pop().unwrap_or_default();
    if let Some(error) = &data.error {
        return Err(Error::Query(error.clone()));
    }
    Ok((panel, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::{Canned, canned};
    use std::string::String;

    /// Records requests and answers them in order.
    struct Recorder {
        responses: Vec<String>,
        requests: Vec<(String, String)>,
    }

    impl Connector for Recorder {
        type Transport = Canned;

        fn send(&mut self, device: &str, request: &[u8]) -> Result<Canned> {
            self.requests.push((
                String::from(device),
                String::from_utf8(request.to_vec()).unwrap(),
            ));
            if self.responses.is_empty() {
                return Err(Error::Network);
            }
            Ok(canned(&self.responses.remove(0), 7))
        }
    }

    fn recorder(responses: &[&str]) -> Recorder {
        Recorder {
            responses: responses.iter().map(|text| String::from(*text)).collect(),
            requests: Vec::new(),
        }
    }

    fn configuration() -> Configuration {
        Configuration {
            url: String::from("https://grafana.example.com"),
            data_source: String::from("prometheus"),
            refresh_seconds: 60,
            dashboard: Some(String::from("servers")),
            ..Configuration::default()
        }
    }

    fn ok(body: &str) -> String {
        std::format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}")
    }

    #[test]
    fn queries_post_with_the_token_and_parse_values() {
        let panel = Panel {
            title: String::from("CPU"),
            expression: String::from("cpu"),
            ..Panel::new()
        };
        let mut connector = recorder(&[&ok(include_str!("../tests/fixtures/instant.json"))]);
        let data = query(
            &mut connector,
            &configuration(),
            "secret",
            &[&panel, &panel],
        )
        .unwrap();
        assert_eq!(data[0].series.len(), 3);
        assert!(data[1].error.is_some());
        let (device, request) = &connector.requests[0];
        assert_eq!(device, "/devices/network/https_client");
        assert!(request.starts_with("POST /api/ds/query HTTP/1.1\r\n"));
        assert!(request.contains("\r\nAuthorization: Bearer secret\r\n"));
        assert!(request.ends_with(r#""expr":"cpu","instant":true,"range":false}]}"#));
    }

    #[test]
    fn query_errors_map_to_user_states() {
        let panel = Panel {
            title: String::from("CPU"),
            expression: String::from("cpu"),
            ..Panel::new()
        };
        for (response, expected) in [
            ("HTTP/1.1 401 Unauthorized\r\n\r\n{}", Error::Unauthorized),
            ("HTTP/1.1 502 Bad Gateway\r\n\r\n<html>", Error::Http(502)),
            ("HTTP/1.1 400 Bad Request\r\n\r\n<html>", Error::Http(400)),
            (
                "HTTP/1.1 200 OK\r\n\r\n{\"results\": ",
                Error::InvalidResponse,
            ),
        ] {
            let mut connector = recorder(&[response]);
            assert_eq!(
                query(&mut connector, &configuration(), "t", &[&panel]),
                Err(expected)
            );
        }
        let mut offline = recorder(&[]);
        assert_eq!(
            query(&mut offline, &configuration(), "t", &[&panel]),
            Err(Error::Network)
        );
        // A failed query in a 400 response is shown on its panel only.
        let body = r#"{"results": {"P0": {"error": "parse error", "status": 400}}}"#;
        let mut connector = recorder(&[&std::format!("HTTP/1.1 400 Bad Request\r\n\r\n{body}")]);
        let data = query(&mut connector, &configuration(), "t", &[&panel]).unwrap();
        assert_eq!(data[0].error.as_deref(), Some("parse error"));
        let mut connector = recorder(&[&std::format!("HTTP/1.1 400 Bad Request\r\n\r\n{body}")]);
        assert_eq!(
            test_connection(&mut connector, &configuration(), "t", None),
            Err(Error::Query(String::from("parse error")))
        );
    }

    #[test]
    fn imports_the_configured_dashboard() {
        let mut connector = recorder(&[&ok(include_str!("../tests/fixtures/dashboard.json"))]);
        let imported = import(&mut connector, &configuration(), "secret").unwrap();
        assert_eq!(imported.panels.len(), 6);
        assert!(
            connector.requests[0]
                .1
                .starts_with("GET /api/dashboards/uid/servers HTTP/1.1\r\n")
        );
        let mut missing = recorder(&["HTTP/1.1 404 Not Found\r\n\r\n{}"]);
        assert_eq!(
            import(&mut missing, &configuration(), "secret"),
            Err(Error::NotFound)
        );
        let mut manual = configuration();
        manual.dashboard = None;
        assert_eq!(
            import(&mut recorder(&[]), &manual, "secret"),
            Err(Error::Configuration)
        );
    }

    #[test]
    fn connection_test_runs_one_query() {
        let body = r#"{"results": {"P0": {"frames": [{"schema": {"fields": [{"name": "Time", "type": "time"}, {"name": "Value", "type": "number"}]}, "data": {"values": [[1], [1]]}}]}}}"#;
        let mut connector = recorder(&[&ok(body)]);
        let (panel, data) = test_connection(&mut connector, &configuration(), "t", None).unwrap();
        assert_eq!(panel.expression, "vector(1)");
        assert_eq!(data.series[0].last, Some(1.0));
    }
}
