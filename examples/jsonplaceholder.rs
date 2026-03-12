use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;

include!(concat!(env!("OUT_DIR"), "/generated_openapi_tools.rs"));

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let openai = rig::providers::openai::Client::from_env();

    let api = GeneratedApiClient::new("https://jsonplaceholder.typicode.com");

    let agent = add_openapi_tools(
        openai
            .agent("gpt-4o")
            .preamble("You have access to API tools. Use them when asked."),
        api,
    )
    .build();

    let response: String = agent
        .prompt("Use the API tool to get user 1 and summarize the result.")
        .await?;

    println!("{response}");

    Ok(())
}
