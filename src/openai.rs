use std::{fs::File, io::Seek, io::Write, time::Duration};

use anyhow::Context;
use bytes::Buf;
use chrono::Utc;
use futures::io::Cursor;
use serde::{Deserialize, Serialize};

use crate::{IRCMessage, TEMPERATURE};

pub const SYSTEM_PROMPT: &str = "You are chatbot in an online chat room.  There are multiple people in this chatroom, their names will appear in angle brackets.  You can answer questions, or extend the conversation with interesting comments.  Answer with short messages and do not repeat yourself. Be creative. Your operator is 'achin', and your own name is 'Charbot9000'.";

#[derive(Serialize, Deserialize, Debug)]
pub enum ChatCompletionRole {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    pub role: ChatCompletionRole,
    pub content: String,
}

#[derive(Serialize, Debug)]
pub struct ChatCompletions {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
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

pub async fn get_chat(messages: Vec<ChatMessage>, temp: f32) -> anyhow::Result<ChatMessage> {
    let _start = std::time::Instant::now();
    println!("Sending chat completion request {:?}", messages.last());
    let now = Utc::now();
    let mut chat_messages = vec![ChatMessage {
        role: ChatCompletionRole::System,
        content: format!("{}. Current date: {}", SYSTEM_PROMPT, now.date_naive()),
    }];
    chat_messages.extend(messages);

    let chat = ChatCompletions {
        model: "gpt-3.5-turbo".into(),
        messages: chat_messages,
        temperature: Some(temp),
    };

    let client = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(30))
    .timeout(Duration::from_secs(60))
    .build()?;
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

    resp.choices
        .pop()
        .map(|x| x.message)
        .context("No completions returned")
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
    let chat = ChatCompletions {
        model: "gpt-3.5-turbo".into(),
        messages: vec![
            ChatMessage {
                role: ChatCompletionRole::System,
                content: SYSTEM_PROMPT.into(),
            },
            ChatMessage {
                role: ChatCompletionRole::User,
                content: "What is x264?".into(),
            },
        ],
        temperature: Some(0.7),
    };

    let client = reqwest::Client::new();
    let req = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(crate::secrets::OPENAPI_KEY)
        .json(&chat)
        .send()
        .await?;

    let resp: ChatResponse = req.json().await?;
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
