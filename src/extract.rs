use openapiv3::{Parameter, ParameterSchemaOrContent, ReferenceOr};
use serde_json::Value;

use crate::tool::{ParamInfo, ParamLocation};

pub(crate) fn extract_param_info(param: &Parameter) -> Option<ParamInfo> {
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

pub(crate) fn extract_body_schema(body: &openapiv3::RequestBody) -> (Option<Value>, bool) {
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
