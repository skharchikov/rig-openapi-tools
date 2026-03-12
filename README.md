# rig-openapi-tools

Point at an OpenAPI 3.0 spec, get a [rig](https://github.com/0xPlaygrounds/rig) agent that can call every endpoint. No codegen, no macros.

Parses the spec at runtime into `ToolDyn` trait objects. Supports path/query/header parameters, JSON request bodies, and `$ref` resolution. Parse once at startup, clone cheaply per request with per-user context injection.

## Quick start

```toml
[dependencies]
rig-openapi-tools = { path = "." }
rig-core = "0.32"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig_openapi_tools::OpenApiToolset;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    let toolset = OpenApiToolset::from_file("petstore.json")?;

    let agent = openai
        .agent("gpt-4o")
        .preamble("You have access to the Petstore API.")
        .tools(toolset.into_tools())
        .build();

    let response: String = agent
        .prompt("What pets are available?")
        .await?;

    println!("{response}");
    Ok(())
}
```

## Builder

```rust
let toolset = OpenApiToolset::builder_from_file("spec.yaml")?
    .base_url("https://api.example.com")
    .bearer_token("sk-your-token")
    .build()?;
```

| Method | Description |
|---|---|
| `.base_url(url)` | Override the base URL from the spec |
| `.bearer_token(token)` | Set a Bearer token for all requests |
| `.client(client)` | Provide a pre-configured `reqwest::Client` |
| `.hidden_context(key, value)` | Auto-inject a param into tool calls, invisible to the LLM |

## Context

Real apps need per-request state: user IDs, session tokens, tenant info. The toolset supports two kinds of context.

**Hidden context** gets injected into tool calls at execution time. The LLM doesn't see these params in the schema at all:

```rust
// Parse once at startup
let toolset = OpenApiToolset::builder_from_file("spec.json")?
    .hidden_context("api_key", "sk-xxx")  // static, all requests
    .build()?;

// Per request
let ctx = HashMap::from([
    ("user_id".to_string(), current_user.id.to_string()),
]);
let tools = toolset.tools_with_context(&ctx);

let agent = openai
    .agent("gpt-4o")
    .preamble("You are an assistant.")
    .tools(tools)
    .build();
```

**Visible context** is for things the LLM should know about. Generate a preamble snippet:

```rust
let visible = HashMap::from([
    ("username".to_string(), "alice".to_string()),
    ("role".to_string(), "admin".to_string()),
]);

let agent = openai
    .agent("gpt-4o")
    .preamble(&format!(
        "You are an assistant.\n\n{}",
        OpenApiToolset::context_preamble(&visible)
    ))
    .tools(toolset.tools_with_context(&HashMap::new()))
    .build();
```

The LLM sees this in the system prompt:
```
The following context is available. Use these values when calling tools:
- username = alice
- role = admin
```

## How it works

```
OpenAPI Spec (YAML/JSON)
        |
        v
  OpenApiToolset::from_file()    <-- parse once at startup
        |
        v
  Vec<OpenApiTool>               <-- internal, cloneable
        |
        +---> .into_tools()              --> Vec<Box<dyn ToolDyn>>  (simple case)
        +---> .tools_with_context(&ctx)  --> Vec<Box<dyn ToolDyn>>  (per request)
                    |
                    v
              rig AgentBuilder::tools()
                    |
                    v
              LLM picks tool + fills args --> HTTP request --> response back to LLM
```

Each operation in the spec becomes a tool. The name comes from `operationId` (falls back to `get_users` style). Description comes from `summary`/`description`. Parameter schemas are passed through to the LLM as is.

## Examples

```bash
cargo run --example jsonplaceholder
cargo run --example petstore
```

Requires `OPENAI_API_KEY` in the environment.

## License

MIT
