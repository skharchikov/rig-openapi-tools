use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub(crate) enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub(crate) fn as_str(self) -> &'static str {
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
pub(crate) enum ParamLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone)]
#[allow(clippy::manual_non_exhaustive)]
pub(crate) struct ParamInfo {
    pub name: String,
    pub location: ParamLocation,
    pub required: bool,
    pub description: String,
    pub schema: Value,
}

#[derive(Clone)]
pub(crate) struct OpenApiTool {
    pub client: reqwest::Client,
    pub base_url: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub operation_id: String,
    pub description: String,
    pub parameters: Vec<ParamInfo>,
    pub request_body_schema: Option<Value>,
    pub request_body_required: bool,
    pub hidden_params: HashMap<String, String>,
}

impl OpenApiTool {
    fn build_parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for p in &self.parameters {
            // Skip params that will be auto-injected from hidden context
            if self.hidden_params.contains_key(&p.name) {
                continue;
            }
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
        let mut args_obj = args.as_object().unwrap_or(&serde_json::Map::new()).clone();

        // Inject hidden context params (don't override LLM-provided values)
        for (key, val) in &self.hidden_params {
            args_obj
                .entry(key.clone())
                .or_insert_with(|| Value::String(val.clone()));
        }

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

        // Header params
        for p in &self.parameters {
            if matches!(p.location, ParamLocation::Header) {
                if let Some(val) = args_obj.get(&p.name) {
                    let header_val = match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    req = req.header(&p.name, header_val);
                }
            }
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
