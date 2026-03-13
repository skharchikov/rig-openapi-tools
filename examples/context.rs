use std::collections::HashMap;

use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig_openapi_tools::OpenApiToolset;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    // ---------------------------------------------------------------
    // 1. Visible context — LLM sees the values and uses them in calls
    // ---------------------------------------------------------------
    println!("=== Visible context ===\n");

    let toolset = OpenApiToolset::builder_from_file("examples/petstore.json")?
        .base_url("https://petstore3.swagger.io/api/v3")
        .build()?;

    println!("Loaded {} tools from Petstore spec\n", toolset.len());

    // The LLM sees these values in its preamble and uses them
    // when calling tools. For example, it will pass `username` to getUserByName.
    let visible_ctx = HashMap::from([
        ("username".to_string(), "user1".to_string()),
        ("preferred_status".to_string(), "available".to_string()),
    ]);
    let context_preamble = OpenApiToolset::context_preamble(&visible_ctx);

    let preamble = format!(
        "You have access to the Swagger Petstore API.\n\n\
         {context_preamble}\n\n\
         When I refer to \"my\" profile or data, use the username from the context above."
    );

    let agent = openai
        .agent("gpt-4o")
        .preamble(&preamble)
        .tools(toolset.tools_with_context(&HashMap::new()))
        .build();

    // The LLM picks up username=user1 from the preamble
    // and calls getUserByName with username "user1".
    println!(">>> Look up my user profile and summarize it.");
    let response: String = agent
        .prompt("Look up my user profile and summarize it.")
        .await?;
    println!("{response}\n");

    // The LLM picks up preferred_status=available from the preamble
    // and calls findPetsByStatus with status "available".
    println!(">>> Find pets matching my preferred status.");
    let response: String = agent
        .prompt("Find pets matching my preferred status.")
        .await?;
    println!("{response}\n");

    // ---------------------------------------------------------------
    // 2. Hidden context — auto-injected, LLM never sees the values
    // ---------------------------------------------------------------
    println!("=== Hidden context ===\n");

    // Hidden context is useful for secrets, user IDs, or any parameter
    // the LLM should NOT decide — it's injected automatically at execution
    // time and removed from the tool schema so the LLM can't see or override it.

    // Static hidden context set at build time (e.g. API key for the upstream service)
    let toolset = OpenApiToolset::builder_from_file("examples/petstore.json")?
        .base_url("https://petstore3.swagger.io/api/v3")
        .hidden_context("api_key", "special-key")
        .build()?;

    // Per-request hidden context (e.g. current user from session).
    // The LLM won't see `username` in the tool schema — it's filled in
    // automatically, so it can't hallucinate a different user.
    let per_request_ctx = HashMap::from([("username".to_string(), "user1".to_string())]);

    let agent = openai
        .agent("gpt-4o")
        .preamble(
            "You have access to the Swagger Petstore API. \
             Use the available tools to answer questions about the pet store.",
        )
        .tools(toolset.tools_with_context(&per_request_ctx))
        .build();

    // The LLM calls getUserByName without providing `username` —
    // it's not in the schema. The library injects username=user1 automatically.
    println!(">>> Get my profile.");
    let response: String = agent.prompt("Get my profile.").await?;
    println!("{response}\n");

    Ok(())
}
