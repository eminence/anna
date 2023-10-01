use std::time::Duration;

use anna::wttr;
use anyhow::{bail, Context};
use bytes::Buf;
use chrono::Utc;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

pub const SYSTEM_PROMPT: &str = "You are chatbot in an online chat room.  There are multiple people in this chatroom, their names will appear in angle brackets.  You can answer questions, or extend the conversation with interesting comments.  Answer with short messages and do not repeat yourself. Be creative. Your operator is 'achin', and your own name is 'Charbot9000'.";

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
pub enum ChatCompletionRole {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "function")]
    Function,
}

/// A message send to or from the openai system
///
/// This generally corresponds to an entry in the "messages"
/// list in the openai chat completions API.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatCompletionRole,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    /// This is string-encoded json
    pub arguments: String,
}

#[derive(Serialize, Debug)]
pub struct ChatCompletions {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub functions: Vec<FunctionDef>,
    pub temperature: Option<f32>,
}

#[derive(Serialize, Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: schemars::schema::RootSchema,
}

#[derive(Deserialize, Debug)]
pub struct ChatResponse {
    id: String,
    object: String,
    created: i64,
    choices: Vec<ChatResponseChoice>,
    usage: ChatUsage,
}

#[derive(Deserialize, Debug)]
pub struct ChatResponseChoice {
    pub index: i32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Deserialize, Debug)]
pub struct ChatUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

#[derive(Serialize, Debug)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Image {
    pub url: String,
}

#[derive(Deserialize, Debug)]
pub struct ImageResponse {
    pub created: u64,
    pub data: Vec<Image>,
}

#[derive(JsonSchema)]
// Start function definitions
struct Evaluate {
    /// A mathmatical expression, like "4 * 3 - 2"
    pub input: String,
}

// End function definitions

async fn get_chat_helper(
    client: &reqwest::Client,
    chat: &ChatCompletions,
) -> anyhow::Result<ChatMessage> {
    let req = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&chat)
        .send()
        .await?;

    if !req.status().is_success() {
        anyhow::bail!(format!("API returned {}", req.status()))
    }

    let resp_text = req.text().await?;

    let mut resp: ChatResponse = match serde_json::from_str(&resp_text) {
        Ok(val) => val,
        Err(e) => {
            println!("error requesting chat:");
            println!("{e}");
            println!("{resp_text}");
            anyhow::bail!("error requesting chat");
        }
    };
    dbg!(&resp.usage);

    let resp = resp
        .choices
        .pop()
        .map(|x| x.message)
        .context("No completions returned")?;

    Ok(resp)
}

/// Get the chat completions for the given chat messages
///
/// This can return multiple chat messages if a function was called
pub async fn get_chat(messages: Vec<ChatMessage>, temp: f32) -> anyhow::Result<Vec<ChatMessage>> {
    let _start = std::time::Instant::now();
    println!("Sending chat completion request {:?}", messages.last());
    let now = Utc::now();
    let mut chat_messages = vec![ChatMessage {
        role: ChatCompletionRole::System,
        content: Some(format!(
            "{}. Current date: {}",
            SYSTEM_PROMPT,
            now.date_naive()
        )),
        name: None,
        function_call: None,
    }];
    chat_messages.extend(messages);

    let functions = vec![FunctionDef {
        name: "get_current_weather".into(),
        description: Some("Gets the current weather, given a city and state".into()),
        parameters: schema_for!(anna::wttr::WeatherInput),
    }];

    let chat = ChatCompletions {
        model: "gpt-4".into(),
        messages: chat_messages,
        temperature: Some(temp),
        functions: functions.clone(),
    };

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()?;
    let resp = get_chat_helper(&client, &chat).await?;

    let mut to_return = Vec::new();
    to_return.push(resp.clone());

    // If our respone has function call info, we need to handle that, send another request
    // and then return the result

    if let Some(call) = &resp.function_call {
        if call.name == "get_current_weather" {
            let weather_input = serde_json::from_str(&call.arguments)?;
            let weather_output = wttr::get_weather(&weather_input).await?;

            let mut new_messages = chat.messages;
            let function_message = ChatMessage {
                role: ChatCompletionRole::Function,
                content: Some(serde_json::to_string(&weather_output)?),
                name: Some(call.name.to_string()),
                function_call: None,
            };
            new_messages.push(function_message.clone());
            to_return.push(function_message);

            // call API again with the updated messages
            let chat = ChatCompletions {
                model: "gpt-4".into(),
                messages: new_messages,
                temperature: Some(temp),
                functions,
            };
            let resp = get_chat_helper(&client, &chat).await?;
            to_return.push(resp);

            Ok(to_return)
        } else {
            bail!("Unknown function call: {}", call.name);
        }
    } else {
        Ok(to_return)
    }
}

pub async fn get_image(prompt: &str) -> anyhow::Result<String> {
    let img = ImageGenerationRequest {
        prompt: prompt.into(),
        n: None,
        size: None,
    };

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()?;
    let req = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&img)
        .send()
        .await?;

    if !req.status().is_success() {
        anyhow::bail!(format!("API returned {}", req.status()))
    }

    let resp_text = req.text().await?;

    let resp: ImageResponse = match serde_json::from_str(&resp_text) {
        Ok(val) => val,
        Err(e) => {
            println!("error requesting image generation:");
            println!("{e}");
            println!("{resp_text}");
            anyhow::bail!("error requesting image generation");
        }
    };
    // dbg!(&resp);

    // download the images, and rehost on cloudflare
    for data in &resp.data {
        let req = client.get(&data.url).send().await?;

        let bytes = req.bytes().await?;
        // let mut output_file = File::create("tmp.png")?;
        let mut data = std::io::Cursor::new(Vec::with_capacity(5 * 1024 * 1024));
        std::io::copy(&mut bytes.reader(), &mut data)?;

        let digest = md5::compute(&data.get_ref());

        let upload_resp = client
            .put(format!("https://up.em32.site/?hash={:x}", digest))
            .header("Content-Type", "image/png")
            .body(data.into_inner())
            .send()
            .await?;

        let url = upload_resp.text().await?;
        if url.starts_with("https://") {
            return Ok(url);
        }
    }

    anyhow::bail!("unknown error")
}

#[tokio::test]
async fn test_openai() -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let req = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .send()
        .await?;

    println!("{}", req.text().await?);

    Ok(())
}

#[tokio::test]
async fn test_openai_chat() -> anyhow::Result<()> {
    let messages = vec![ChatMessage {
        role: ChatCompletionRole::User,
        content: Some("what is the current weather in each of (1) Reynoldsburg, Ohio, (2) Tampere, Finland, and (3) heaven itself?".into()),
        name: None,
        function_call: None,
    }];

    let resp = get_chat(messages, 1.0).await?;
    dbg!(resp);

    Ok(())
}

#[tokio::test]
async fn test_openai_chat2() -> anyhow::Result<()> {
    let chat = ChatCompletions {
        model: "gpt-4".into(),
        messages: vec![
            ChatMessage {
                role: ChatCompletionRole::System,
                content: Some(SYSTEM_PROMPT.into()),
                name: None,
                function_call: None,
            },
            ChatMessage {
                role: ChatCompletionRole::User,
                content: Some("how's the weather up there?".into()),
                name: None,
                function_call: None,
            },
        ],
        temperature: Some(0.7),
        functions: vec![FunctionDef {
            name: "get_current_weather".into(),
            description: Some("Gets the current weather, given a city and state".into()),
            parameters: schema_for!(anna::wttr::WeatherInput),
        }],
    };

    let client = reqwest::Client::new();
    let req = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&chat)
        .send()
        .await?;

    let resp = req.text().await?;
    println!("{}", resp);
    // let resp: ChatResponse = req.json().await?;
    // Ok(resp.choices.get(0).context("No chat completions returned")?.message)
    Ok(())
}

#[tokio::test]
async fn test_image_generation() -> anyhow::Result<()> {
    let img = ImageGenerationRequest {
        prompt:
            "A crayon drawing colorful duck in a large pond.  White birch trees suround the pond"
                .into(),
        n: None,
        size: None,
    };

    let client = reqwest::Client::new();
    let req = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&img)
        .send()
        .await?;

    let resp: ImageResponse = req.json().await?;
    dbg!(&resp);

    // download the images, and rehost on cloudflare
    for data in &resp.data {
        let req = client.get(&data.url).send().await?;

        let bytes = req.bytes().await?;
        // let mut output_file = File::create("tmp.png")?;
        let mut data = std::io::Cursor::new(Vec::with_capacity(5 * 1024 * 1024));
        std::io::copy(&mut bytes.reader(), &mut data)?;

        let digest = md5::compute(&data.get_ref());

        let upload_resp = client
            .put(format!("https://up.em32.site/?hash={:x}", digest))
            .header("Content-Type", "image/png")
            .body(data.into_inner())
            .send()
            .await?;

        println!("{}", upload_resp.text().await?);
    }

    Ok(())
}

#[test]
fn test_json_schemas() {
    use schemars::{schema_for, JsonSchema};

    #[derive(JsonSchema)]
    struct WeatherInput {
        pub city: String,
        pub state: String,
        pub country: Option<String>,
    }

    let my_func = FunctionDef {
        name: "my_func".into(),
        description: None,
        parameters: schema_for!(WeatherInput),
    };

    let j = serde_json::to_string_pretty(&my_func).unwrap();
    println!("{j}");

    // let schema = ;
    // println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}

#[tokio::test]
async fn test_openai_chat_functions() -> anyhow::Result<()> {
    #[derive(JsonSchema)]
    struct Evaluate {
        /// A mathmatical expression, like "4 * 3 - 2"
        pub input: String,
    }

    let chat = ChatCompletions {
        model: "gpt-4".into(),
        messages: vec![
            ChatMessage {
                role: ChatCompletionRole::System,
                content: Some(SYSTEM_PROMPT.into()),
                name: None,
                function_call: None,
            },
            ChatMessage {
                role: ChatCompletionRole::User,
                content: Some("What is weather in Paris?".into()),
                name: None,
                function_call: None,
            },
        ],
        temperature: Some(0.7),
        functions: vec![
            FunctionDef {
                name: "get_current_weather".into(),
                description: Some("Gets the current weather, given a city and state".into()),
                parameters: schema_for!(anna::wttr::WeatherInput),
            },
            FunctionDef {
                name: "evaluate_expression".into(),
                description: Some("Evaluates a mathematical expression".into()),
                parameters: schema_for!(Evaluate),
            },
        ],
    };

    println!("{}", serde_json::to_string_pretty(&chat).unwrap());

    let client = reqwest::Client::new();
    let req = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&chat)
        .send()
        .await?;

    // let resp = req.text().await?;
    let resp: ChatResponse = req.json().await?;
    println!("{resp:#?}");
    // Ok(resp.choices.get(0).context("No chat completions returned")?.message)
    Ok(())
}
