use std::collections::HashSet;

use openapiv3::{OpenAPI, Parameter, ReferenceOr, RequestBody, Schema};
use serde_json::Value;

/// Lightweight ref resolver that looks up `$ref` strings in the spec's components.
/// Only resolves the types we actually need, avoiding the cost of resolving the entire spec.
pub(crate) struct Resolver<'a> {
    spec: &'a OpenAPI,
}

impl<'a> Resolver<'a> {
    pub fn new(spec: &'a OpenAPI) -> Self {
        Self { spec }
    }

    pub fn resolve_parameter(&self, r: &'a ReferenceOr<Parameter>) -> Option<&'a Parameter> {
        match r {
            ReferenceOr::Item(item) => Some(item),
            ReferenceOr::Reference { reference } => {
                let name = ref_name(reference, "parameters")?;
                let components = self.spec.components.as_ref()?;
                match components.parameters.get(name)? {
                    ReferenceOr::Item(item) => Some(item),
                    _ => None, // nested ref, skip
                }
            }
        }
    }

    pub fn resolve_request_body(&self, r: &'a ReferenceOr<RequestBody>) -> Option<&'a RequestBody> {
        match r {
            ReferenceOr::Item(item) => Some(item),
            ReferenceOr::Reference { reference } => {
                let name = ref_name(reference, "requestBodies")?;
                let components = self.spec.components.as_ref()?;
                match components.request_bodies.get(name)? {
                    ReferenceOr::Item(item) => Some(item),
                    _ => None,
                }
            }
        }
    }

    pub fn resolve_schema(&self, r: &'a ReferenceOr<Schema>) -> Option<&'a Schema> {
        match r {
            ReferenceOr::Item(item) => Some(item),
            ReferenceOr::Reference { reference } => {
                let name = ref_name(reference, "schemas")?;
                let components = self.spec.components.as_ref()?;
                match components.schemas.get(name)? {
                    ReferenceOr::Item(item) => Some(item),
                    _ => None,
                }
            }
        }
    }

    /// Recursively inline all `$ref` references in a JSON schema value.
    /// Handles circular references by replacing them with an empty object.
    pub fn inline_refs(&self, value: &mut Value) {
        let mut visited = HashSet::new();
        self.inline_refs_inner(value, &mut visited);
    }

    fn inline_refs_inner(&self, value: &mut Value, visited: &mut HashSet<String>) {
        match value {
            Value::Object(map) => {
                // Check if this object is a $ref
                if let Some(Value::String(ref_str)) = map.get("$ref") {
                    let ref_str = ref_str.clone();
                    if let Some(name) = ref_name(&ref_str, "schemas") {
                        if visited.contains(name) {
                            // Circular reference — replace with empty object
                            map.clear();
                            map.insert("type".into(), Value::String("object".into()));
                            return;
                        }
                        visited.insert(name.to_string());

                        if let Some(resolved) = self
                            .spec
                            .components
                            .as_ref()
                            .and_then(|c| c.schemas.get(name))
                        {
                            if let ReferenceOr::Item(schema) = resolved {
                                if let Ok(mut inlined) = serde_json::to_value(schema) {
                                    self.inline_refs_inner(&mut inlined, visited);
                                    *value = inlined;
                                    visited.remove(name);
                                    return;
                                }
                            }
                        }
                        visited.remove(name);
                    }
                } else {
                    // Recurse into all values
                    for val in map.values_mut() {
                        self.inline_refs_inner(val, visited);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.inline_refs_inner(item, visited);
                }
            }
            _ => {}
        }
    }
}

/// Extract the component name from a `$ref` string like `#/components/parameters/Foo`.
fn ref_name<'a>(reference: &'a str, expected_kind: &str) -> Option<&'a str> {
    let path = reference.strip_prefix("#/components/")?;
    let (kind, name) = path.split_once('/')?;
    if kind == expected_kind {
        Some(name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_spec(yaml: &str) -> OpenAPI {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn inline_refs_resolves_nested_schema_ref() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
components:
  schemas:
    Category:
      type: object
      properties:
        id:
          type: integer
        name:
          type: string
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({
            "type": "object",
            "properties": {
                "category": { "$ref": "#/components/schemas/Category" }
            }
        });

        resolver.inline_refs(&mut value);

        let category = &value["properties"]["category"];
        assert!(category.get("$ref").is_none(), "ref should be inlined");
        assert_eq!(category["type"], "object");
        assert!(category["properties"]["id"].is_object());
        assert!(category["properties"]["name"].is_object());
    }

    #[test]
    fn inline_refs_resolves_ref_inside_array_items() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
components:
  schemas:
    Tag:
      type: object
      properties:
        name:
          type: string
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/Tag" }
                }
            }
        });

        resolver.inline_refs(&mut value);

        let items = &value["properties"]["tags"]["items"];
        assert!(items.get("$ref").is_none(), "ref in array items should be inlined");
        assert_eq!(items["type"], "object");
        assert!(items["properties"]["name"].is_object());
    }

    #[test]
    fn inline_refs_handles_circular_reference() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
components:
  schemas:
    Node:
      type: object
      properties:
        value:
          type: string
        child:
          $ref: '#/components/schemas/Node'
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({ "$ref": "#/components/schemas/Node" });

        resolver.inline_refs(&mut value);

        // Top-level should be resolved
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["value"].is_object());
        // Circular child should become {"type": "object"}
        let child = &value["properties"]["child"];
        assert!(child.get("$ref").is_none(), "circular ref should be replaced");
        assert_eq!(child["type"], "object");
    }

    #[test]
    fn inline_refs_resolves_deeply_nested_refs() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
components:
  schemas:
    Inner:
      type: object
      properties:
        value:
          type: integer
    Middle:
      type: object
      properties:
        inner:
          $ref: '#/components/schemas/Inner'
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({
            "type": "object",
            "properties": {
                "middle": { "$ref": "#/components/schemas/Middle" }
            }
        });

        resolver.inline_refs(&mut value);

        let inner = &value["properties"]["middle"]["properties"]["inner"];
        assert!(inner.get("$ref").is_none(), "transitive ref should be inlined");
        assert_eq!(inner["type"], "object");
        assert!(inner["properties"]["value"].is_object());
    }

    #[test]
    fn inline_refs_no_op_without_refs() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });

        let expected = value.clone();
        resolver.inline_refs(&mut value);

        assert_eq!(value, expected);
    }

    #[test]
    fn inline_refs_unknown_ref_left_as_is() {
        let spec = parse_spec(
            r#"
openapi: "3.0.0"
info:
  title: Test
  version: "1.0"
paths: {}
components:
  schemas: {}
"#,
        );
        let resolver = Resolver::new(&spec);

        let mut value = json!({
            "type": "object",
            "properties": {
                "thing": { "$ref": "#/components/schemas/DoesNotExist" }
            }
        });

        resolver.inline_refs(&mut value);

        // Unresolvable ref stays as-is
        assert!(value["properties"]["thing"].get("$ref").is_some());
    }

    #[test]
    fn petstore_addpet_schema_has_no_refs() {
        let spec_str = std::fs::read_to_string("examples/petstore.json").unwrap();
        let spec: OpenAPI = serde_json::from_str(&spec_str).unwrap();
        let resolver = Resolver::new(&spec);

        // Resolve the Pet schema (used by addPet's request body)
        let pet_ref = spec
            .components
            .as_ref()
            .unwrap()
            .schemas
            .get("Pet")
            .unwrap();
        if let ReferenceOr::Item(schema) = pet_ref {
            let mut value = serde_json::to_value(schema).unwrap();
            resolver.inline_refs(&mut value);

            let serialized = serde_json::to_string(&value).unwrap();
            assert!(
                !serialized.contains("$ref"),
                "Pet schema should have no $ref after inlining, got: {}",
                serialized
            );

            // Verify Category and Tag are inlined
            let category = &value["properties"]["category"];
            assert_eq!(category["type"], "object");
            let tags_items = &value["properties"]["tags"]["items"];
            assert_eq!(tags_items["type"], "object");
        } else {
            panic!("Pet should be an Item, not a Reference");
        }
    }
}
