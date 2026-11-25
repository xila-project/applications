//! Home Assistant REST API calls: reading entity states and calling
//! services, with `Authorization: Bearer <token>` on every request.

use crate::configuration::Configuration;
use crate::error::{Error, Result};
use crate::json::Reader;
use crate::net::{Connector, Endpoint, read_response, status_error};
use crate::service::{service_path, state_path};
use crate::state::{self, EntityState};
use std::{string::String, vec::Vec};

/// Reads one entity state: `GET /api/states/<entity_id>`.
pub fn fetch_state<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    token: &str,
    entity_id: &str,
) -> Result<EntityState> {
    let request = endpoint.request("GET", &state_path(entity_id), token, None);
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, body) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    state::parse(&mut Reader::new(body))
}

/// Calls one service: `POST /api/services/<domain>/<service>` with `body`.
/// Home Assistant answers 200 with the states it changed; they are not
/// shown, so the body is not read.
pub fn call_service<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    token: &str,
    domain: &str,
    service: &str,
    body: &str,
) -> Result<()> {
    let request = endpoint.request(
        "POST",
        &service_path(domain, service),
        token,
        Some(body.as_bytes()),
    );
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, _body) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    Ok(())
}

/// Reads every configured entity state, one request each, in order. An
/// entity that fails does not stop the others: its error is returned in its
/// slot, so a missing entity (404) shows on its card alone. A server-level
/// failure (no network, timeout, refused token) stops the refresh and is
/// copied to the remaining entities, so one dead server is one request, not
/// one per entity.
pub fn refresh_states<C: Connector>(
    connector: &mut C,
    configuration: &Configuration,
    token: &str,
    entities: &[String],
) -> Result<Vec<Result<EntityState>>> {
    let endpoint = Endpoint::parse(&configuration.url)?;
    let mut results: Vec<Result<EntityState>> = Vec::with_capacity(entities.len());
    for entity_id in entities {
        let result = fetch_state(connector, &endpoint, token, entity_id);
        let fatal = matches!(
            &result,
            Err(Error::Network | Error::Timeout | Error::Unauthorized)
        );
        results.push(result);
        if fatal {
            let error = results.last().and_then(|r| r.as_ref().err().cloned());
            if let Some(error) = error {
                while results.len() < entities.len() {
                    results.push(Err(error.clone()));
                }
            }
            break;
        }
    }
    Ok(results)
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
            url: String::from("http://homeassistant.local:8123"),
            refresh_seconds: 30,
            entities: std::vec![
                String::from("light.living_room"),
                String::from("sensor.outdoor_temperature"),
                String::from("media_player.television"),
            ],
        }
    }

    fn ok(body: &str) -> String {
        std::format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}")
    }

    fn endpoint() -> Endpoint {
        Endpoint::parse("http://homeassistant.local:8123").unwrap()
    }

    #[test]
    fn fetches_a_state_with_the_token() {
        let mut connector = recorder(&[&ok(include_str!("../tests/fixtures/light.json"))]);
        let state =
            fetch_state(&mut connector, &endpoint(), "secret", "light.living_room").unwrap();
        assert_eq!(state.state, "on");
        assert_eq!(state.display_name(), "Living room");
        let (device, request) = &connector.requests[0];
        assert_eq!(device, "/devices/network/http_client");
        assert!(request.starts_with("GET /api/states/light.living_room HTTP/1.1\r\n"));
        assert!(request.contains("\r\nHost: homeassistant.local:8123\r\n"));
        assert!(request.contains("\r\nAuthorization: Bearer secret\r\n"));
        assert!(request.ends_with("\r\n\r\n"));
    }

    #[test]
    fn state_errors_map_to_user_states() {
        for (response, expected) in [
            ("HTTP/1.1 401 Unauthorized\r\n\r\n{}", Error::Unauthorized),
            ("HTTP/1.1 403 Forbidden\r\n\r\n{}", Error::Unauthorized),
            ("HTTP/1.1 404 Not Found\r\n\r\n{}", Error::NotFound),
            (
                "HTTP/1.1 500 Internal Server Error\r\n\r\n",
                Error::Http(500),
            ),
            (
                "HTTP/1.1 200 OK\r\n\r\n{\"entity_id\": ",
                Error::InvalidResponse,
            ),
        ] {
            let mut connector = recorder(&[response]);
            assert_eq!(
                fetch_state(&mut connector, &endpoint(), "t", "light.living_room"),
                Err(expected)
            );
        }
        let mut offline = recorder(&[]);
        assert_eq!(
            fetch_state(&mut offline, &endpoint(), "t", "light.living_room"),
            Err(Error::Network)
        );
    }

    #[test]
    fn calls_a_service_with_its_body() {
        let mut connector = recorder(&[&ok("[]")]);
        call_service(
            &mut connector,
            &endpoint(),
            "secret",
            "light",
            "toggle",
            &crate::service::service_body("light.living_room"),
        )
        .unwrap();
        let (device, request) = &connector.requests[0];
        assert_eq!(device, "/devices/network/http_client");
        assert!(request.starts_with("POST /api/services/light/toggle HTTP/1.1\r\n"));
        assert!(request.contains("\r\nAuthorization: Bearer secret\r\n"));
        assert!(request.contains("\r\nContent-Type: application/json\r\n"));
        assert!(
            request.ends_with("\r\n\r\n{\"entity_id\":\"light.living_room\"}"),
            "{request}"
        );
        for (response, expected) in [
            ("HTTP/1.1 401 Unauthorized\r\n\r\n[]", Error::Unauthorized),
            ("HTTP/1.1 404 Not Found\r\n\r\n[]", Error::NotFound),
            ("HTTP/1.1 400 Bad Request\r\n\r\n", Error::Http(400)),
        ] {
            let mut connector = recorder(&[response]);
            assert_eq!(
                call_service(&mut connector, &endpoint(), "t", "light", "toggle", "{}"),
                Err(expected)
            );
        }
    }

    #[test]
    fn refresh_reads_every_entity_in_order() {
        let responses = [
            ok(include_str!("../tests/fixtures/light.json")),
            "HTTP/1.1 404 Not Found\r\n\r\n{}".to_owned(),
            ok(include_str!("../tests/fixtures/media_player.json")),
        ];
        let mut connector = recorder(&responses.iter().map(|r| r.as_str()).collect::<Vec<_>>());
        let results = refresh_states(
            &mut connector,
            &configuration(),
            "secret",
            &configuration().entities,
        )
        .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap().entity_id, "light.living_room");
        assert_eq!(results[1], Err(Error::NotFound));
        assert_eq!(
            results[2].as_ref().unwrap().entity_id,
            "media_player.television"
        );
        let requests = &connector.requests;
        assert_eq!(requests.len(), 3);
        assert!(
            requests[0]
                .1
                .starts_with("GET /api/states/light.living_room ")
        );
        assert!(
            requests[1]
                .1
                .starts_with("GET /api/states/sensor.outdoor_temperature ")
        );
        assert!(
            requests[2]
                .1
                .starts_with("GET /api/states/media_player.television ")
        );
    }

    #[test]
    fn refresh_stops_at_the_first_server_level_failure() {
        let configuration = configuration();
        // No canned response: the connector itself reports no network.
        let mut connector = recorder(&[]);
        let results =
            refresh_states(&mut connector, &configuration, "t", &configuration.entities).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| *r == Err(Error::Network)));
        assert_eq!(
            connector.requests.len(),
            1,
            "one request, not one per entity"
        );
        // A refused token behaves the same.
        let mut connector = recorder(&["HTTP/1.1 401 Unauthorized\r\n\r\n{}"]);
        let results =
            refresh_states(&mut connector, &configuration, "t", &configuration.entities).unwrap();
        assert!(results.iter().all(|r| *r == Err(Error::Unauthorized)));
        assert_eq!(connector.requests.len(), 1);
        // A missing entity (404) is not server level: the others still load.
        let responses = [
            String::from("HTTP/1.1 404 Not Found\r\n\r\n{}"),
            ok(include_str!("../tests/fixtures/sensor.json")),
            ok(include_str!("../tests/fixtures/media_player.json")),
        ];
        let mut connector = recorder(&responses.iter().map(String::as_str).collect::<Vec<_>>());
        let results =
            refresh_states(&mut connector, &configuration, "t", &configuration.entities).unwrap();
        assert_eq!(results[0], Err(Error::NotFound));
        assert!(results[1].is_ok() && results[2].is_ok());
        assert_eq!(connector.requests.len(), 3);
    }

    #[test]
    fn refresh_fails_as_a_whole_on_a_bad_url() {
        let mut configuration = configuration();
        configuration.url = String::from("homeassistant.local");
        let mut connector = recorder(&[]);
        assert_eq!(
            refresh_states(&mut connector, &configuration, "t", &configuration.entities),
            Err(Error::Configuration)
        );
    }
}
