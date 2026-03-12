use openapiv3::{Parameter, ParameterSchemaOrContent};
use serde_json::Value;

use crate::resolve::Resolver;
use crate::tool::{ParamInfo, ParamLocation};

pub(crate) fn extract_param_info(param: &Parameter, resolver: &Resolver) -> Option<ParamInfo> {
    let (data, location) = match param {
        Parameter::Path { parameter_data, .. } => (parameter_data, ParamLocation::Path),
        Parameter::Query { parameter_data, .. } => (parameter_data, ParamLocation::Query),
        Parameter::Header { parameter_data, .. } => (parameter_data, ParamLocation::Header),
        Parameter::Cookie { .. } => return None,
    };

    let schema = match &data.format {
        ParameterSchemaOrContent::Schema(ref_or_schema) => {
            match resolver.resolve_schema(ref_or_schema) {
                Some(schema) => {
                    let mut val = serde_json::to_value(schema)
                        .unwrap_or(serde_json::json!({"type": "string"}));
                    resolver.inline_refs(&mut val);
                    val
                }
                None => serde_json::json!({"type": "string"}),
            }
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

pub(crate) fn extract_body_schema(
    body: &openapiv3::RequestBody,
    resolver: &Resolver,
) -> (Option<Value>, bool) {
    let schema = body
        .content
        .get("application/json")
        .and_then(|mt| mt.schema.as_ref())
        .and_then(|s| resolver.resolve_schema(s))
        .and_then(|schema| serde_json::to_value(schema).ok())
        .map(|mut val| {
            resolver.inline_refs(&mut val);
            val
        });
    (schema, body.required)
}
