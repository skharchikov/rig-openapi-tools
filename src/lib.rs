use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// OpenAPI spec types (private, minimal subset)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OpenApiSpec {
    #[serde(default)]
    servers: Vec<Server>,
    #[serde(default)]
    paths: BTreeMap<String, PathItem>,
}

#[derive(Deserialize)]
struct Server {
    url: String,
}

#[derive(Deserialize, Default)]
struct PathItem {
    #[serde(default)]
    get: Option<Operation>,
    #[serde(default)]
    post: Option<Operation>,
    #[serde(default)]
    put: Option<Operation>,
    #[serde(default)]
    patch: Option<Operation>,
    #[serde(default)]
    delete: Option<Operation>,
}

#[derive(Deserialize)]
struct Operation {
    #[serde(rename = "operationId")]
    operation_id: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    #[serde(default)]
    parameters: Vec<Parameter>,
    #[serde(rename = "requestBody")]
    request_body: Option<RequestBody>,
}

#[derive(Deserialize)]
struct Parameter {
    name: String,
    #[serde(rename = "in")]
    location: String,
    #[serde(default)]
    required: Option<bool>,
    description: Option<String>,
    #[serde(default)]
    schema: Option<Value>,
}

#[derive(Deserialize)]
struct RequestBody {
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    content: BTreeMap<String, MediaType>,
}

#[derive(Deserialize)]
struct MediaType {
    schema: Option<Value>,
}

// ---------------------------------------------------------------------------
// $ref resolution
// ---------------------------------------------------------------------------

fn resolve_refs(value: &mut Value, root: &Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ref_path)) = map.get("$ref") {
                if let Some(resolved) = json_pointer(root, ref_path) {
                    let mut resolved = resolved.clone();
                    resolve_refs(&mut resolved, root);
                    *value = resolved;
                    return;
                }
            }
            for v in map.values_mut() {
                resolve_refs(v, root);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                resolve_refs(v, root);
            }
        }
        _ => {}
    }
}

fn json_pointer<'a>(root: &'a Value, ref_path: &str) -> Option<&'a Value> {
    let path = ref_path.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let decoded = segment.replace("~1", "/").replace("~0", "~");
        current = current.get(&decoded)?;
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// Internal tool representation
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

// Send + Sync is required by ToolDyn (via WasmCompatSend/Sync).
// All fields are Send + Sync, so this is automatic.

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

        // Build request
        let mut req = match self.method {
            HttpMethod::Get => self.client.get(&url),
            HttpMethod::Post => self.client.post(&url),
            HttpMethod::Put => self.client.put(&url),
            HttpMethod::Patch => self.client.patch(&url),
            HttpMethod::Delete => self.client.delete(&url),
        };

        // Add query params
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

        // Add request body
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
        Self::parse(spec_str, None)
    }

    /// Parse an OpenAPI spec, overriding the base URL from the spec.
    pub fn from_str_with_base_url(spec_str: &str, base_url: &str) -> anyhow::Result<Self> {
        Self::parse(spec_str, Some(base_url))
    }

    /// Parse an OpenAPI spec file, overriding the base URL from the spec.
    pub fn from_file_with_base_url(path: impl AsRef<Path>, base_url: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content, Some(base_url))
    }

    fn parse(spec_str: &str, base_url_override: Option<&str>) -> anyhow::Result<Self> {
        // Parse YAML/JSON into a Value for $ref resolution
        let mut root: Value = serde_yaml::from_str(spec_str)?;
        let snapshot = root.clone();
        resolve_refs(&mut root, &snapshot);

        // Deserialize into typed structs
        let spec: OpenApiSpec = serde_json::from_value(root)?;

        let base_url = base_url_override
            .map(|s| s.to_string())
            .or_else(|| spec.servers.first().map(|s| s.url.clone()))
            .unwrap_or_else(|| "http://localhost".into());
        let base_url = base_url.trim_end_matches('/').to_string();

        let client = reqwest::Client::new();
        let mut tools: Vec<Box<dyn ToolDyn>> = Vec::new();

        for (path_template, item) in spec.paths {
            let methods = [
                (HttpMethod::Get, item.get),
                (HttpMethod::Post, item.post),
                (HttpMethod::Put, item.put),
                (HttpMethod::Patch, item.patch),
                (HttpMethod::Delete, item.delete),
            ];

            for (method, op) in methods {
                let Some(op) = op else { continue };

                let operation_id = op.operation_id.unwrap_or_else(|| {
                    format!(
                        "{}_{}",
                        method.as_str().to_lowercase(),
                        path_template.replace('/', "_").trim_start_matches('_')
                    )
                });

                let description = op
                    .summary
                    .or(op.description)
                    .unwrap_or_else(|| format!("{} {}", method.as_str(), path_template));

                let parameters: Vec<ParamInfo> = op
                    .parameters
                    .into_iter()
                    .filter_map(|p| {
                        let location = match p.location.as_str() {
                            "path" => ParamLocation::Path,
                            "query" => ParamLocation::Query,
                            "header" => ParamLocation::Header,
                            _ => return None,
                        };
                        Some(ParamInfo {
                            name: p.name,
                            location,
                            required: p
                                .required
                                .unwrap_or(matches!(location, ParamLocation::Path)),
                            description: p.description.unwrap_or_default(),
                            schema: p.schema.unwrap_or(serde_json::json!({"type": "string"})),
                        })
                    })
                    .collect();

                let (request_body_schema, request_body_required) = op
                    .request_body
                    .and_then(|rb| {
                        let required = rb.required.unwrap_or(false);
                        let schema = rb.content.get("application/json")?.schema.clone()?;
                        Some((Some(schema), required))
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
