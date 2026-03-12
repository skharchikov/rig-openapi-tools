use anyhow::Result;
use quote::{format_ident, quote};
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Debug, Deserialize)]
struct OpenApi {
    #[serde(default)]
    servers: Vec<Server>,
    paths: BTreeMap<String, PathItem>,
}

#[derive(Debug, Deserialize)]
struct Server {
    url: String,
}

#[derive(Debug, Deserialize, Default)]
struct PathItem {
    #[serde(default)]
    get: Option<Operation>,
}

#[derive(Debug, Deserialize, Default)]
struct Operation {
    #[serde(rename = "operationId")]
    operation_id: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    parameters: Vec<Parameter>,
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    #[serde(rename = "in")]
    location: String,
    #[serde(default)]
    required: Option<bool>,
    description: Option<String>,
}

fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut next_upper = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            next_upper = true;
        } else if next_upper {
            out.push(ch.to_ascii_uppercase());
            next_upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=openapi.yaml");

    let spec_str = fs::read_to_string("openapi.yaml")?;
    let spec: OpenApi = serde_yaml::from_str(&spec_str)?;

    let _base_url = spec
        .servers
        .first()
        .map(|s| s.url.clone())
        .unwrap_or_else(|| "http://localhost".to_string());

    let mut tool_impls = Vec::new();
    let mut registrar_calls = Vec::new();

    for (path, item) in &spec.paths {
        let Some(op) = &item.get else { continue };

        let op_name = op
            .operation_id
            .clone()
            .unwrap_or_else(|| "generatedGet".to_string());
        let tool_struct = format_ident!("{}Tool", pascal_case(&op_name));
        let args_struct = format_ident!("{}Args", pascal_case(&op_name));
        let description = op.summary.clone().unwrap_or_else(|| format!("GET {path}"));

        let path_params: Vec<_> = op
            .parameters
            .iter()
            .filter(|p| p.location == "path")
            .collect();

        // Build fields for the Args struct
        let mut arg_fields = Vec::new();
        let mut prop_entries = Vec::new();
        let mut required_entries = Vec::new();

        for p in &path_params {
            let field_ident = format_ident!("{}", snake_case(&p.name));
            let desc = p
                .description
                .clone()
                .unwrap_or_else(|| format!("{} parameter", p.name));
            let field_name_str = snake_case(&p.name);

            arg_fields.push(quote! { pub #field_ident: String });

            prop_entries.push(quote! {
                #field_name_str: { "type": "string", "description": #desc }
            });

            if p.required.unwrap_or(false) {
                required_entries.push(quote! { #field_name_str });
            }
        }

        // Build the URL format string: replace {param} with {}
        // and collect the corresponding field accesses
        let mut url_fmt = path.clone();
        let mut fmt_args = Vec::new();
        for p in &path_params {
            let field_ident = format_ident!("{}", snake_case(&p.name));
            url_fmt = url_fmt.replace(&format!("{{{}}}", p.name), "{}");
            fmt_args.push(quote! { args.#field_ident });
        }

        tool_impls.push(quote! {
            #[derive(Debug, Clone, serde::Deserialize)]
            pub struct #args_struct {
                #(#arg_fields,)*
            }

            #[derive(Clone)]
            pub struct #tool_struct {
                api: GeneratedApiClient,
            }

            impl #tool_struct {
                pub fn new(api: GeneratedApiClient) -> Self {
                    Self { api }
                }
            }

            impl rig::tool::Tool for #tool_struct {
                const NAME: &'static str = #op_name;

                type Error = ApiToolError;
                type Args = #args_struct;
                type Output = serde_json::Value;

                async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
                    rig::completion::ToolDefinition {
                        name: #op_name.to_string(),
                        description: #description.to_string(),
                        parameters: serde_json::json!({
                            "type": "object",
                            "properties": {
                                #(#prop_entries),*
                            },
                            "required": [#(#required_entries),*]
                        }),
                    }
                }

                async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
                    let path = format!(#url_fmt, #(#fmt_args),*);
                    let url = format!("{}{}", self.api.base_url, path);
                    let resp = self.api.client.get(&url).send().await?.error_for_status()?;
                    let json = resp.json::<serde_json::Value>().await?;
                    Ok(json)
                }
            }
        });

        registrar_calls.push(quote! {
            .tool(#tool_struct::new(api.clone()))
        });
    }

    let output = quote! {
        #[derive(Debug)]
        pub struct ApiToolError(pub String);

        impl std::fmt::Display for ApiToolError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::error::Error for ApiToolError {}

        impl From<reqwest::Error> for ApiToolError {
            fn from(e: reqwest::Error) -> Self {
                ApiToolError(e.to_string())
            }
        }

        #[derive(Clone)]
        pub struct GeneratedApiClient {
            pub client: reqwest::Client,
            pub base_url: String,
        }

        impl GeneratedApiClient {
            pub fn new(base_url: impl Into<String>) -> Self {
                Self {
                    client: reqwest::Client::new(),
                    base_url: base_url.into(),
                }
            }
        }

        #(#tool_impls)*

        pub fn add_openapi_tools<M: rig::completion::CompletionModel>(
            builder: rig::agent::AgentBuilder<M>,
            api: GeneratedApiClient,
        ) -> rig::agent::AgentBuilder<M, (), rig::agent::WithBuilderTools> {
            builder
                #(#registrar_calls)*
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(
        out_dir.join("generated_openapi_tools.rs"),
        output.to_string(),
    )?;

    Ok(())
}
