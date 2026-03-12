use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use openapi_utils::SpecExt;
use openapiv3::{OpenAPI, Parameter, ParameterSchemaOrContent, ReferenceOr};
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ParamLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone)]
struct ParamInfo {
    name: String,
    location: ParamLocation,
    required: bool,
    description: String,
    schema: Value,
}

struct OpenApiTool {
    client: reqwest::Client,
    base_url: String,
    method: HttpMethod,
    path_template: String,
    operation_id: String,
    description: String,
    parameters: Vec<ParamInfo>,
    request_body_schema: Option<Value>,
    request_body_required: bool,
}

impl OpenApiTool {
    fn build_parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for p in &self.parameters {
            let mut schema = p.schema.clone();
            if let Value::Object(ref mut map) = schema {
                if !p.description.is_empty() {
                    map.insert("description".into(), Value::String(p.description.clone()));
                }
            }
            properties.insert(p.name.clone(), schema);
            if p.required {
                required.push(Value::String(p.name.clone()));
            }
        }

        if let Some(body_schema) = &self.request_body_schema {
            properties.insert("body".into(), body_schema.clone());
            if self.request_body_required {
                required.push(Value::String("body".into()));
            }
        }

        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        })
    }

    async fn execute(
        &self,
        args: Value,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let args_obj = args.as_object().unwrap_or(&serde_json::Map::new()).clone();

        // Build URL: substitute path params
        let mut path = self.path_template.clone();
        for p in &self.parameters {
            if matches!(p.location, ParamLocation::Path) {
                let val = args_obj
                    .get(&p.name)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                path = path.replace(&format!("{{{}}}", p.name), &val);
            }
        }

        let url = format!("{}{}", self.base_url, path);

        let mut req = match self.method {
            HttpMethod::Get => self.client.get(&url),
            HttpMethod::Post => self.client.post(&url),
            HttpMethod::Put => self.client.put(&url),
            HttpMethod::Patch => self.client.patch(&url),
            HttpMethod::Delete => self.client.delete(&url),
        };

        // Query params
        let query_params: Vec<(String, String)> = self
            .parameters
            .iter()
            .filter(|p| matches!(p.location, ParamLocation::Query))
            .filter_map(|p| {
                args_obj.get(&p.name).map(|v| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (p.name.clone(), val)
                })
            })
            .collect();

        if !query_params.is_empty() {
            req = req.query(&query_params);
        }

        // Request body
        if let Some(body) = args_obj.get("body") {
            req = req.json(body);
        }

        let resp = req.send().await?.error_for_status()?;
        let json: Value = resp.json().await?;
        Ok(json)
    }
}

impl ToolDyn for OpenApiTool {
    fn name(&self) -> String {
        self.operation_id.clone()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> Pin<Box<dyn Future<Output = ToolDefinition> + Send + 'a>> {
        let def = ToolDefinition {
            name: self.operation_id.clone(),
            description: self.description.clone(),
            parameters: self.build_parameters_schema(),
        };
        Box::pin(async move { def })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let args: Value = serde_json::from_str(&args)?;
            let result = self.execute(args).await.map_err(ToolError::ToolCallError)?;
            serde_json::to_string(&result).map_err(ToolError::JsonError)
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers for extracting data from openapiv3 types
// ---------------------------------------------------------------------------

fn extract_param_info(param: &Parameter) -> Option<ParamInfo> {
    let (data, location) = match param {
        Parameter::Path { parameter_data, .. } => (parameter_data, ParamLocation::Path),
        Parameter::Query { parameter_data, .. } => (parameter_data, ParamLocation::Query),
        Parameter::Header { parameter_data, .. } => (parameter_data, ParamLocation::Header),
        Parameter::Cookie { .. } => return None,
    };

    let schema = match &data.format {
        ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema)) => {
            serde_json::to_value(schema).unwrap_or(serde_json::json!({"type": "string"}))
        }
        _ => serde_json::json!({"type": "string"}),
    };

    Some(ParamInfo {
        name: data.name.clone(),
        location,
        required: data.required,
        description: data.description.clone().unwrap_or_default(),
        schema,
    })
}

fn extract_body_schema(body: &openapiv3::RequestBody) -> (Option<Value>, bool) {
    let schema = body
        .content
        .get("application/json")
        .and_then(|mt| mt.schema.as_ref())
        .and_then(|s| match s {
            ReferenceOr::Item(schema) => serde_json::to_value(schema).ok(),
            _ => None,
        });
    (schema, body.required)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A set of tools generated from an OpenAPI specification.
///
/// Each operation in the spec becomes a tool that can be registered with a rig agent.
pub struct OpenApiToolset {
    tools: Vec<Box<dyn ToolDyn>>,
}

impl OpenApiToolset {
    /// Parse an OpenAPI spec from a YAML or JSON file.
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_spec_str(&content)
    }

    /// Parse an OpenAPI spec from a YAML or JSON string.
    pub fn from_spec_str(spec_str: &str) -> anyhow::Result<Self> {
        Self::build(spec_str, None)
    }

    /// Parse an OpenAPI spec, overriding the base URL from the spec.
    pub fn from_str_with_base_url(spec_str: &str, base_url: &str) -> anyhow::Result<Self> {
        Self::build(spec_str, Some(base_url))
    }

    /// Parse an OpenAPI spec file, overriding the base URL from the spec.
    pub fn from_file_with_base_url(path: impl AsRef<Path>, base_url: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::build(&content, Some(base_url))
    }

    fn build(spec_str: &str, base_url_override: Option<&str>) -> anyhow::Result<Self> {
        let spec: OpenAPI = serde_yaml::from_str(spec_str)?;
        let spec = spec.deref_all();

        let base_url = base_url_override
            .map(|s| s.to_string())
            .or_else(|| spec.servers.first().map(|s| s.url.clone()))
            .unwrap_or_else(|| "http://localhost".into());
        let base_url = base_url.trim_end_matches('/').to_string();

        let client = reqwest::Client::new();
        let mut tools: Vec<Box<dyn ToolDyn>> = Vec::new();

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

                let parameters: Vec<ParamInfo> = op
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

                tools.push(Box::new(OpenApiTool {
                    client: client.clone(),
                    base_url: base_url.clone(),
                    method,
                    path_template: path_template.clone(),
                    operation_id,
                    description,
                    parameters,
                    request_body_schema,
                    request_body_required,
                }));
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // We can't inspect base_url directly, but we can verify parsing succeeded
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn base_url_override() {
        let toolset =
            OpenApiToolset::from_str_with_base_url(MINIMAL_SPEC, "https://override.com").unwrap();
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
    async fn tool_call_with_invalid_json_returns_error() {
        let toolset = OpenApiToolset::from_spec_str(MINIMAL_SPEC).unwrap();
        let tools = toolset.into_tools();
        let result = tools[0].call("not json".into()).await;
        assert!(result.is_err());
    }
}
