use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig_openapi_tools::OpenApiToolset;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    let toolset = OpenApiToolset::builder_from_file("examples/petstore.json")?
        .base_url("https://petstore3.swagger.io/api/v3")
        .build()?;

    println!("Loaded {} tools from Petstore spec\n", toolset.len());

    let agent = openai
        .agent("gpt-4o")
        .preamble(
            "You have access to the Swagger Petstore API. \
             Use the available tools to answer questions about the pet store.",
        )
        .tools(toolset.into_tools())
        .build();

    let prompts = [
        "What pets are currently available in the store? Show me the first 3.",
        "Get the store inventory and tell me the status counts.",
        "Look up user 'user1' and summarize their profile.",
    ];

    for prompt in prompts {
        println!(">>> {prompt}");
        let response: String = agent.prompt(prompt).await?;
        println!("{response}\n");
    }

    Ok(())
}
