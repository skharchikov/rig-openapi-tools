mod extract;
mod tool;

use std::collections::HashMap;
use std::path::Path;

use openapi_utils::SpecExt;
use openapiv3::{OpenAPI, ReferenceOr};
use rig::tool::ToolDyn;

use crate::extract::{extract_body_schema, extract_param_info};
use crate::tool::{HttpMethod, OpenApiTool};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A set of tools generated from an OpenAPI specification.
///
/// Each operation in the spec becomes a tool that can be registered with a rig agent.
/// The toolset is designed to be parsed once and reused across requests.
pub struct OpenApiToolset {
    tools: Vec<OpenApiTool>,
}

/// Builder for configuring an [`OpenApiToolset`].
pub struct OpenApiToolsetBuilder {
    spec_str: String,
    base_url: Option<String>,
    client: Option<reqwest::Client>,
    hidden_context: HashMap<String, String>,
}

impl OpenApiToolsetBuilder {
    /// Override the base URL from the spec.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Provide a pre-configured reqwest client (e.g. with default auth headers or timeouts).
    pub fn client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Add a hidden context parameter that will be auto-injected into tool calls.
    /// The LLM will not see this parameter in the tool schema.
    pub fn hidden_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.hidden_context.insert(key.into(), value.into());
        self
    }

    /// Convenience: create a client with a bearer token `Authorization` header.
    pub fn bearer_token(self, token: &str) -> Self {
        use reqwest::header;
        let mut headers = header::HeaderMap::new();
        let mut auth_value =
            header::HeaderValue::from_str(&format!("Bearer {token}")).expect("invalid token");
        auth_value.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth_value);

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build reqwest client");
        self.client(client)
    }

    /// Build the toolset, parsing the spec and creating tools.
    pub fn build(self) -> anyhow::Result<OpenApiToolset> {
        OpenApiToolset::build_inner(
            &self.spec_str,
            self.base_url.as_deref(),
            self.client,
            self.hidden_context,
        )
    }
}

impl OpenApiToolset {
    /// Parse an OpenAPI spec from a YAML or JSON file.
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_spec_str(&content)
    }

    /// Parse an OpenAPI spec from a YAML or JSON string.
    pub fn from_spec_str(spec_str: &str) -> anyhow::Result<Self> {
        Self::build_inner(spec_str, None, None, HashMap::new())
    }

    /// Start building a toolset from a YAML or JSON string with configuration options.
    pub fn builder(spec_str: &str) -> OpenApiToolsetBuilder {
        OpenApiToolsetBuilder {
            spec_str: spec_str.to_string(),
            base_url: None,
            client: None,
            hidden_context: HashMap::new(),
        }
    }

    /// Start building a toolset from a file with configuration options.
    pub fn builder_from_file(path: impl AsRef<Path>) -> anyhow::Result<OpenApiToolsetBuilder> {
        let content = std::fs::read_to_string(path)?;
        Ok(OpenApiToolsetBuilder {
            spec_str: content,
            base_url: None,
            client: None,
            hidden_context: HashMap::new(),
        })
    }

    fn build_inner(
        spec_str: &str,
        base_url_override: Option<&str>,
        client: Option<reqwest::Client>,
        hidden_context: HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let spec: OpenAPI = serde_yaml::from_str(spec_str)?;
        let spec = spec.deref_all();

        let base_url = base_url_override
            .map(|s| s.to_string())
            .or_else(|| spec.servers.first().map(|s| s.url.clone()))
            .unwrap_or_else(|| "http://localhost".into());
        let base_url = base_url.trim_end_matches('/').to_string();

        let client = client.unwrap_or_default();
        let mut tools: Vec<OpenApiTool> = Vec::new();

        for (path_template, path_item_ref) in &spec.paths {
            let ReferenceOr::Item(path_item) = path_item_ref else {
                continue;
            };

            let methods = [
                (HttpMethod::Get, &path_item.get),
                (HttpMethod::Post, &path_item.post),
                (HttpMethod::Put, &path_item.put),
                (HttpMethod::Patch, &path_item.patch),
                (HttpMethod::Delete, &path_item.delete),
            ];

            for (method, op) in methods {
                let Some(op) = op else { continue };

                let method_lower = method.as_str().to_lowercase();
                let operation_id = op.operation_id.clone().unwrap_or_else(|| {
                    let path_slug = path_template.replace('/', "_");
                    let path_slug = path_slug.trim_start_matches('_');
                    format!("{}_{}", method_lower, path_slug)
                });

                let description = op
                    .summary
                    .clone()
                    .or_else(|| op.description.clone())
                    .unwrap_or_else(|| format!("{} {}", method.as_str(), path_template));

                let parameters = op
                    .parameters
                    .iter()
                    .filter_map(|p| match p {
                        ReferenceOr::Item(param) => extract_param_info(param),
                        _ => None,
                    })
                    .collect();

                let (request_body_schema, request_body_required) = op
                    .request_body
                    .as_ref()
                    .and_then(|rb| match rb {
                        ReferenceOr::Item(body) => Some(extract_body_schema(body)),
                        _ => None,
                    })
                    .unwrap_or((None, false));

                tools.push(OpenApiTool {
                    client: client.clone(),
                    base_url: base_url.clone(),
                    method,
                    path_template: path_template.clone(),
                    operation_id,
                    description,
                    parameters,
                    request_body_schema,
                    request_body_required,
                    hidden_params: hidden_context.clone(),
                });
            }
        }

        Ok(Self { tools })
    }

    /// Return the number of tools parsed from the spec.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns true if no operations were found in the spec.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Consume the toolset and return tools for use with rig's `AgentBuilder::tools()`.
    pub fn into_tools(self) -> Vec<Box<dyn ToolDyn>> {
        self.tools
            .into_iter()
            .map(|t| Box::new(t) as Box<dyn ToolDyn>)
            .collect()
    }

    /// Clone the tools with per-request context injected as hidden parameters.
    /// The LLM will not see these parameters in tool schemas, but they will be
    /// auto-injected into every tool call at execution time.
    ///
    /// This is the primary way to add per-request state (user ID, session info, etc.)
    /// while reusing the parsed toolset across requests.
    pub fn tools_with_context(
        &self,
        context: &HashMap<String, String>,
    ) -> Vec<Box<dyn ToolDyn>> {
        self.tools
            .iter()
            .map(|t| {
                let mut tool = t.clone();
                tool.hidden_params.extend(context.clone());
                Box::new(tool) as Box<dyn ToolDyn>
            })
            .collect()
    }

    /// Generate a preamble snippet describing the visible context for the LLM.
    /// Include this in your agent's `.preamble()` so the LLM knows about
    /// available context values it can use when calling tools.
    pub fn context_preamble(context: &HashMap<String, String>) -> String {
        if context.is_empty() {
            return String::new();
        }
        let entries: Vec<String> = context
            .iter()
            .map(|(k, v)| format!("- {k} = {v}"))
            .collect();
        format!(
            "The following context is available. Use these values when calling tools:\n{}",
            entries.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const MINIMAL_SPEC: &str = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /users/{id}:
    get:
      operationId: getUser
      summary: Get a user by id
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
          description: The user id
      responses:
        "200":
          description: OK
"#;

    const MULTI_METHOD_SPEC: &str = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /users:
    get:
      operationId: listUsers
      summary: List all users
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
          description: Max results
      responses:
        "200":
          description: OK
    post:
      operationId: createUser
      summary: Create a user
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                name:
                  type: string
                email:
                  type: string
              required:
                - name
      responses:
        "201":
          description: Created
  /users/{id}:
    get:
      operationId: getUser
      summary: Get a user
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        "200":
          description: OK
    delete:
      operationId: deleteUser
      summary: Delete a user
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        "204":
          description: Deleted
"#;

    const REF_SPEC: &str = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /items/{id}:
    get:
      operationId: getItem
      summary: Get an item
      parameters:
        - $ref: '#/components/parameters/ItemId'
      responses:
        "200":
          description: OK
components:
  parameters:
    ItemId:
      name: id
      in: path
      required: true
      schema:
        type: string
      description: The item id
"#;

    #[test]
    fn parse_minimal_spec() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn parse_multi_method_spec() {
        let toolset = OpenApiToolset::from_spec_str(MULTI_METHOD_SPEC).unwrap();
        assert_eq!(toolset.len(), 4);
    }

    #[test]
    fn tool_names_match_operation_ids() {
        let toolset = OpenApiToolset::from_spec_str(MULTI_METHOD_SPEC).unwrap();
        let tools = toolset.into_tools();
        let names: Vec<String> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"listUsers".to_string()));
        assert!(names.contains(&"createUser".to_string()));
        assert!(names.contains(&"getUser".to_string()));
        assert!(names.contains(&"deleteUser".to_string()));
    }

    #[test]
    fn fallback_operation_id_when_missing() {
        let spec = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths:
  /health:
    get:
      summary: Health check
      responses:
        "200":
          description: OK
"#;
        let toolset = OpenApiToolset::from_spec_str(spec).unwrap();
        let tools = toolset.into_tools();
        assert_eq!(tools[0].name(), "get_health");
    }

    #[test]
    fn base_url_from_spec() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        let tools = toolset.into_tools();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn builder_base_url_override() {
        let toolset = OpenApiToolset::builder(MINIMAL_SPEC)
            .base_url("https://override.com")
            .build()
            .unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn builder_bearer_token() {
        let toolset = OpenApiToolset::builder(MINIMAL_SPEC)
            .bearer_token("test-token-123")
            .build()
            .unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn builder_custom_client() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        let toolset = OpenApiToolset::builder(MINIMAL_SPEC)
            .client(client)
            .build()
            .unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn builder_all_options() {
        let toolset = OpenApiToolset::builder(MINIMAL_SPEC)
            .base_url("https://custom.api.com")
            .bearer_token("sk-123")
            .build()
            .unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn base_url_defaults_to_localhost() {
        let spec = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths:
  /ping:
    get:
      operationId: ping
      summary: Ping
      responses:
        "200":
          description: OK
"#;
        let toolset = OpenApiToolset::from_spec_str(spec).unwrap();
        assert_eq!(toolset.len(), 1);
    }

    #[test]
    fn empty_spec_produces_no_tools() {
        let spec = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
"#;
        let toolset = OpenApiToolset::from_spec_str(spec).unwrap();
        assert!(toolset.is_empty());
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let result = OpenApiToolset::from_spec_str("not: [valid: yaml: {{");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tool_definition_has_correct_fields() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        let tools = toolset.into_tools();
        let def = tools[0].definition("".into()).await;

        assert_eq!(def.name, "getUser");
        assert_eq!(def.description, "Get a user by id");
    }

    #[tokio::test]
    async fn tool_definition_path_param_schema() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        let tools = toolset.into_tools();
        let def = tools[0].definition("".into()).await;

        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("id"));

        let required = def.parameters["required"].as_array().unwrap();
        assert!(required.contains(&Value::String("id".into())));
    }

    #[tokio::test]
    async fn tool_definition_query_param_not_required() {
        let toolset = OpenApiToolset::from_spec_str(MULTI_METHOD_SPEC).unwrap();
        let tools = toolset.into_tools();
        let list_tool = tools.iter().find(|t| t.name() == "listUsers").unwrap();
        let def = list_tool.definition("".into()).await;

        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("limit"));

        let required = def.parameters["required"].as_array().unwrap();
        assert!(!required.contains(&Value::String("limit".into())));
    }

    #[tokio::test]
    async fn tool_definition_request_body_schema() {
        let toolset = OpenApiToolset::from_spec_str(MULTI_METHOD_SPEC).unwrap();
        let tools = toolset.into_tools();
        let create_tool = tools.iter().find(|t| t.name() == "createUser").unwrap();
        let def = create_tool.definition("".into()).await;

        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("body"));

        let required = def.parameters["required"].as_array().unwrap();
        assert!(required.contains(&Value::String("body".into())));
    }

    #[tokio::test]
    async fn ref_parameters_are_resolved() {
        let toolset = OpenApiToolset::from_spec_str(REF_SPEC).unwrap();
        let tools = toolset.into_tools();
        assert_eq!(tools.len(), 1);

        let def = tools[0].definition("".into()).await;
        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("id"));
    }

    #[tokio::test]
    async fn tool_definition_header_param() {
        let spec = r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths:
  /data:
    get:
      operationId: getData
      summary: Get data
      parameters:
        - name: X-Request-Id
          in: header
          required: false
          schema:
            type: string
          description: Correlation ID
      responses:
        "200":
          description: OK
"#;
        let toolset = OpenApiToolset::from_spec_str(spec).unwrap();
        let tools = toolset.into_tools();
        let def = tools[0].definition("".into()).await;

        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("X-Request-Id"));
    }

    #[tokio::test]
    async fn tool_call_with_invalid_json_returns_error() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        let tools = toolset.into_tools();
        let result = tools[0].call("not json".into()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn hidden_context_excluded_from_schema() {
        let toolset = OpenApiToolset::builder(MINIMAL_SPEC)
            .hidden_context("id", "123")
            .build()
            .unwrap();
        let tools = toolset.into_tools();
        let def = tools[0].definition("".into()).await;

        let props = def.parameters["properties"].as_object().unwrap();
        assert!(!props.contains_key("id"), "hidden param should not appear in schema");

        let required = def.parameters["required"].as_array().unwrap();
        assert!(!required.contains(&Value::String("id".into())));
    }

    #[tokio::test]
    async fn tools_with_context_excludes_from_schema() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();

        // Without context, "id" is visible
        let tools = toolset.tools_with_context(&HashMap::new());
        let def = tools[0].definition("".into()).await;
        let props = def.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("id"));

        // With context, "id" is hidden
        let ctx = HashMap::from([("id".to_string(), "42".to_string())]);
        let tools = toolset.tools_with_context(&ctx);
        let def = tools[0].definition("".into()).await;
        let props = def.parameters["properties"].as_object().unwrap();
        assert!(!props.contains_key("id"));
    }

    #[test]
    fn toolset_reusable_across_contexts() {
        let toolset = OpenApiToolset::from_spec_str(MULTI_METHOD_SPEC).unwrap();

        let ctx1 = HashMap::from([("id".to_string(), "1".to_string())]);
        let ctx2 = HashMap::from([("id".to_string(), "2".to_string())]);

        let tools1 = toolset.tools_with_context(&ctx1);
        let tools2 = toolset.tools_with_context(&ctx2);

        assert_eq!(tools1.len(), 4);
        assert_eq!(tools2.len(), 4);
    }

    #[test]
    fn context_preamble_generation() {
        let ctx = HashMap::from([("user_id".to_string(), "123".to_string())]);
        let preamble = OpenApiToolset::context_preamble(&ctx);
        assert!(preamble.contains("user_id = 123"));
        assert!(preamble.contains("Use these values"));
    }

    #[test]
    fn context_preamble_empty() {
        let preamble = OpenApiToolset::context_preamble(&HashMap::new());
        assert!(preamble.is_empty());
    }
}
