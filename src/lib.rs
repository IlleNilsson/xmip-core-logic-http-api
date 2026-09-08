#![forbid(unsafe_code)]

//! The HTTP API logic technology — a technology of `xmip-core-logic`.
//!
//! A method and a path name the operation; the body, JSON, is the arguments;
//! path and query values are parameters. An `OpenAPI` document, when one is
//! bound, gives each method-and-path its `operationId` and the service its
//! title, and a path template such as `/orders/{id}` yields `id` as a
//! parameter. Without one, the operation is named by what arrived:
//! `GET /orders/42`. A result goes back as `200` with its body; a fault as its
//! status with a JSON body naming it. This is what retired `xmip-core-webapi`
//! (ADR-0014 amendment 2026-08-26): the web API is this technology over the
//! `http` transport.

use contract::ContractId;
use logic::{
    Arrival, Fault, Header, Invocation, Logic, LogicError, OperationName, Outcome, Reply, Request,
};
use serde_json::Value;
use stream::Stream;

/// One operation an `OpenAPI` document declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    pub template: String,
    pub operation_id: String,
}

/// The HTTP API technology, bare or bound to an `OpenAPI` document.
pub struct HttpApi {
    service: String,
    routes: Vec<Route>,
}

impl HttpApi {
    /// Operations named by what arrives.
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: String::new(),
            routes: Vec::new(),
        }
    }

    /// Operations named by an `OpenAPI` document: every `paths` entry's
    /// method with an `operationId`, and `info.title` as the service.
    ///
    /// # Errors
    /// The document is not JSON or has no `paths`.
    pub fn with_openapi(document: &str) -> Result<Self, LogicError> {
        let document: Value = serde_json::from_str(document)
            .map_err(|error| LogicError::new(format!("not valid JSON: {error}")))?;
        let paths = document
            .get("paths")
            .and_then(Value::as_object)
            .ok_or_else(|| LogicError::new("the document has no paths"))?;
        let mut routes = Vec::new();
        for (template, item) in paths {
            let Some(operations) = item.as_object() else {
                continue;
            };
            for (method, operation) in operations {
                if let Some(id) = operation.get("operationId").and_then(Value::as_str) {
                    routes.push(Route {
                        method: method.to_ascii_uppercase(),
                        template: template.clone(),
                        operation_id: id.to_string(),
                    });
                }
            }
        }
        Ok(Self {
            service: document
                .pointer("/info/title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            routes,
        })
    }

    #[must_use]
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    fn route_for(&self, method: &str, path: &str) -> Option<(&Route, Vec<Header>)> {
        self.routes
            .iter()
            .filter(|r| r.method.eq_ignore_ascii_case(method))
            .find_map(|r| bind(&r.template, path).map(|p| (r, p)))
    }
}

impl Default for HttpApi {
    fn default() -> Self {
        Self::new()
    }
}

/// Match `path` against `template`, yielding each `{name}` as a parameter.
fn bind(template: &str, path: &str) -> Option<Vec<Header>> {
    let wanted: Vec<&str> = template.trim_matches('/').split('/').collect();
    let actual: Vec<&str> = path.trim_matches('/').split('/').collect();
    if wanted.len() != actual.len() {
        return None;
    }
    let mut parameters = Vec::new();
    for (segment, value) in wanted.iter().zip(actual) {
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(name) => parameters.push(Header::new(name, value)),
            None if *segment == value => {}
            None => return None,
        }
    }
    Some(parameters)
}

fn split_query(target: &str) -> (&str, Vec<Header>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let parameters = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Header::new(name, value)
        })
        .collect();
    (path, parameters)
}

fn fill(template: &str, parameters: &[Header]) -> String {
    let mut path = template.to_string();
    for parameter in parameters {
        path = path.replace(&format!("{{{}}}", parameter.name), &parameter.value);
    }
    path
}

fn status_of(headers: &[Header]) -> Option<u16> {
    headers
        .iter()
        .find(|h| h.name == ":status")
        .and_then(|h| h.value.parse().ok())
}

impl Logic for HttpApi {
    fn technology(&self) -> &'static str {
        "http-api"
    }

    fn invocation(&self, arrival: &Arrival<'_>) -> Result<Invocation, LogicError> {
        let (path, mut parameters) = split_query(arrival.target);
        let operation = match self.route_for(arrival.method, path) {
            Some((route, bound)) => {
                parameters.extend(bound);
                OperationName::new(self.service.clone(), route.operation_id.clone())
            }
            None if self.routes.is_empty() => OperationName::new(
                "",
                format!("{} {path}", arrival.method.to_ascii_uppercase()),
            ),
            None => {
                return Err(LogicError::new(format!(
                    "{} {path} is not an operation the API declares",
                    arrival.method
                )));
            }
        };
        Ok(Invocation {
            operation,
            arguments: arrival.body.clone(),
            parameters,
            contract: Some(ContractId("json-schema".to_string())),
        })
    }

    fn reply(&self, invocation: &Invocation, outcome: &Outcome) -> Result<Reply, LogicError> {
        let id = invocation.arguments.id();
        Ok(match outcome {
            Outcome::Result(result) => Reply {
                headers: vec![
                    Header::new(":status", "200"),
                    Header::new(
                        "Content-Type",
                        result.media_type().unwrap_or("application/json"),
                    ),
                ],
                body: result.clone(),
            },
            Outcome::Fault(fault) => {
                let status = fault.code.parse::<u16>().unwrap_or(500);
                let body = serde_json::json!({ "error": fault.code, "message": fault.message });
                Reply {
                    headers: vec![
                        Header::new(":status", status.to_string()),
                        Header::new("Content-Type", "application/problem+json"),
                    ],
                    body: Stream::new(
                        id,
                        body.to_string().into_bytes(),
                        Some("application/problem+json".into()),
                    ),
                }
            }
        })
    }

    fn request(&self, invocation: &Invocation) -> Result<Request, LogicError> {
        let (method, target) = match self
            .routes
            .iter()
            .find(|r| r.operation_id == invocation.operation.name)
        {
            Some(route) => (
                route.method.clone(),
                fill(&route.template, &invocation.parameters),
            ),
            None => invocation
                .operation
                .name
                .split_once(' ')
                .map(|(m, p)| (m.to_string(), p.to_string()))
                .ok_or_else(|| {
                    LogicError::new(format!(
                        "{} is neither a declared operation nor METHOD /path",
                        invocation.operation.name
                    ))
                })?,
        };
        Ok(Request {
            target,
            method,
            headers: vec![Header::new(
                "Content-Type",
                invocation
                    .arguments
                    .media_type()
                    .unwrap_or("application/json"),
            )],
            body: invocation.arguments.clone(),
        })
    }

    fn outcome(&self, _invocation: &Invocation, reply: &Reply) -> Result<Outcome, LogicError> {
        let status =
            status_of(&reply.headers).ok_or_else(|| LogicError::new("the reply has no :status"))?;
        if (200..300).contains(&status) {
            return Ok(Outcome::Result(reply.body.clone()));
        }
        let message = std::str::from_utf8(reply.body.bytes())
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|json| {
                json.get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        Ok(Outcome::Fault(Fault {
            code: status.to_string(),
            message,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::StreamId;

    fn json(text: &str) -> Stream {
        Stream::new(
            StreamId::new(1),
            text.as_bytes().to_vec(),
            Some("application/json".to_string()),
        )
    }

    const OPENAPI: &str = r#"{"openapi":"3.1.0","info":{"title":"Orders"},"paths":{
      "/orders":{"post":{"operationId":"placeOrder"}},
      "/orders/{id}":{"get":{"operationId":"getOrder"},"delete":{"operationId":"cancelOrder"}}}}"#;

    #[test]
    fn a_bound_api_names_operations_and_binds_path_and_query() {
        let api = HttpApi::with_openapi(OPENAPI).expect("openapi");
        assert_eq!(api.routes().len(), 3);
        let body = json("{}");
        let arrival = Arrival {
            target: "/orders/42?expand=lines",
            method: "get",
            headers: &[],
            body: &body,
        };
        let invocation = api.invocation(&arrival).expect("invocation");
        assert_eq!(
            invocation.operation,
            OperationName::new("Orders", "getOrder")
        );
        assert!(invocation.parameters.contains(&Header::new("id", "42")));
        assert!(
            invocation
                .parameters
                .contains(&Header::new("expand", "lines"))
        );
        let unknown = Arrival {
            target: "/customers",
            method: "GET",
            headers: &[],
            body: &body,
        };
        assert!(api.invocation(&unknown).is_err());
    }

    #[test]
    fn a_bare_api_names_operations_by_what_arrived() {
        let body = json(r#"{"sku":"X"}"#);
        let arrival = Arrival {
            target: "/orders",
            method: "post",
            headers: &[],
            body: &body,
        };
        let invocation = HttpApi::new().invocation(&arrival).expect("invocation");
        assert_eq!(invocation.operation.name, "POST /orders");
        assert_eq!(invocation.arguments.bytes(), body.bytes());
    }

    #[test]
    fn a_result_is_200_and_a_fault_is_its_status_with_a_problem_body() {
        let api = HttpApi::new();
        let invocation = Invocation {
            operation: OperationName::new("", "POST /orders"),
            arguments: json("{}"),
            parameters: vec![],
            contract: None,
        };
        let ok = api
            .reply(&invocation, &Outcome::Result(json(r#"{"id":42}"#)))
            .expect("reply");
        assert!(ok.headers.contains(&Header::new(":status", "200")));
        let refused = api
            .reply(
                &invocation,
                &Outcome::Fault(Fault {
                    code: "404".into(),
                    message: "no such order".into(),
                }),
            )
            .expect("reply");
        assert!(refused.headers.contains(&Header::new(":status", "404")));
        assert!(
            std::str::from_utf8(refused.body.bytes())
                .expect("json")
                .contains("no such order")
        );
    }

    #[test]
    fn a_request_fills_the_template_and_an_outcome_reads_the_status() {
        let api = HttpApi::with_openapi(OPENAPI).expect("openapi");
        let invocation = Invocation {
            operation: OperationName::new("Orders", "cancelOrder"),
            arguments: json(""),
            parameters: vec![Header::new("id", "42")],
            contract: None,
        };
        let request = api.request(&invocation).expect("request");
        assert_eq!(
            (request.method.as_str(), request.target.as_str()),
            ("DELETE", "/orders/42")
        );
        let gone = Reply {
            headers: vec![Header::new(":status", "204")],
            body: json(""),
        };
        assert!(matches!(
            api.outcome(&invocation, &gone).expect("outcome"),
            Outcome::Result(_)
        ));
        let missing = Reply {
            headers: vec![Header::new(":status", "404")],
            body: json(r#"{"message":"no such order"}"#),
        };
        match api.outcome(&invocation, &missing).expect("outcome") {
            Outcome::Fault(fault) => assert_eq!(
                fault,
                Fault {
                    code: "404".into(),
                    message: "no such order".into()
                }
            ),
            Outcome::Result(_) => panic!("expected a fault"),
        }
    }
}
