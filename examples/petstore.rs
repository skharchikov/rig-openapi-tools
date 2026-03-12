use std::collections::HashMap;

use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig_openapi_tools::OpenApiToolset;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    // Parse once at startup — reuse across requests
    let toolset = OpenApiToolset::builder_from_file("examples/petstore.json")?
        .base_url("https://petstore3.swagger.io/api/v3")
        .build()?;

    println!("Loaded {} tools from Petstore spec\n", toolset.len());

    // Simulate a per-request context (e.g. from a logged-in user session)
    let visible_ctx = HashMap::from([
        ("username".to_string(), "user1".to_string()),
        ("preferred_status".to_string(), "available".to_string()),
    ]);
    let context_preamble = OpenApiToolset::context_preamble(&visible_ctx);

    let preamble = format!(
        "You have access to the Swagger Petstore API. \
         Use the available tools to answer questions about the pet store.\n\n\
         {context_preamble}"
    );

    // Create agent with per-request tools (cheap clone)
    let agent = openai
        .agent("gpt-4o")
        .preamble(&preamble)
        .tools(toolset.tools_with_context(&HashMap::new()))
        .build();

    let prompts = [
        "What pets are currently available in the store? Show me the first 3.",
        "How many dogs are in the store?",
        "Look up my user profile and summarize it.",
    ];

    for prompt in prompts {
        println!(">>> {prompt}");
        let response: String = agent.prompt(prompt).await?;
        println!("{response}\n");
    }

    Ok(())
}
