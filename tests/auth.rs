use rig_openapi_tools::OpenApiToolset;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn spec_with_base_url(base_url: &str) -> String {
    format!(
        r#"openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
servers:
  - url: {}
paths:
  /hello:
    get:
      operationId: sayHello
      summary: Hello
      responses:
        "200":
          description: OK
"#,
        base_url
    )
}

#[tokio::test]
async fn bearer_token_sent_as_authorization_header() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello"))
        .and(header("authorization", "Bearer mytoken"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let spec = spec_with_base_url(&server.uri());
    let toolset = OpenApiToolset::builder(&spec)
        .bearer_token("mytoken")
        .build()
        .unwrap();
    let tools = toolset.into_tools();
    let result = tools[0].call("{}".to_string()).await;
    assert!(result.is_ok(), "tool call failed: {:?}", result.err());
}

#[tokio::test]
async fn api_key_header_sent_correctly() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello"))
        .and(header("x-api-key", "mykey"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let spec = spec_with_base_url(&server.uri());
    let toolset = OpenApiToolset::builder(&spec)
        .api_key_header("X-API-Key", "mykey")
        .build()
        .unwrap();
    let tools = toolset.into_tools();
    let result = tools[0].call("{}".to_string()).await;
    assert!(result.is_ok(), "tool call failed: {:?}", result.err());
}

#[tokio::test]
async fn api_key_query_param_appended() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello"))
        .and(query_param("api_key", "mykey"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let spec = spec_with_base_url(&server.uri());
    let toolset = OpenApiToolset::builder(&spec)
        .api_key_query("api_key", "mykey")
        .build()
        .unwrap();
    let tools = toolset.into_tools();
    let result = tools[0].call("{}".to_string()).await;
    assert!(result.is_ok(), "tool call failed: {:?}", result.err());
}

#[tokio::test]
async fn basic_auth_sent_as_authorization_header() {
    let server = MockServer::start().await;

    // base64("user:pass") = "dXNlcjpwYXNz"
    Mock::given(method("GET"))
        .and(path("/hello"))
        .and(header("authorization", "Basic dXNlcjpwYXNz"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let spec = spec_with_base_url(&server.uri());
    let toolset = OpenApiToolset::builder(&spec)
        .basic_auth("user", "pass")
        .build()
        .unwrap();
    let tools = toolset.into_tools();
    let result = tools[0].call("{}".to_string()).await;
    assert!(result.is_ok(), "tool call failed: {:?}", result.err());
}
