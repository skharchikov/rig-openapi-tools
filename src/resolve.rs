use openapiv3::{OpenAPI, Parameter, ReferenceOr, RequestBody, Schema};

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
