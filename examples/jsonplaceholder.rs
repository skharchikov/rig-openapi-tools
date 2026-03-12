use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig_openapi_tools::OpenApiToolset;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    let toolset = OpenApiToolset::from_file("openapi.yaml")?;
    println!("Loaded {} tools from OpenAPI spec", toolset.len());

    let agent = openai
        .agent("gpt-4o")
        .preamble("You have access to API tools. Use them when asked.")
        .tools(toolset.into_tools())
        .build();

    let response: String = agent
        .prompt("Use the API tool to get user 1 and summarize the result.")
        .await?;

    println!("{response}");

    Ok(())
}
