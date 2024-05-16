use std::{collections::HashMap, fs::File};

use anyhow::Context;
use async_openai::types::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestMessageContentPart, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent, Role,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// pub mod plugins;

pub mod openai;
mod secrets;
pub mod wttr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageThing {
    /// When this message was generated
    pub date: DateTime<Utc>,
    pub msg: ChatCompletionRequestMessage,
}

impl ChatMessageThing {
    pub fn new_now(msg: ChatCompletionRequestMessage) -> Self {
        Self {
            date: Utc::now(),
            msg,
        }
    }
    pub fn reconstitute(self) -> Self {
        let msg = match self.msg {
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage { content, role, name }) => {
                if role == Role::User {
                    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                        content: ChatCompletionRequestUserMessageContent::Text(content),
                        role,
                        name,
                    })
                } else {
                    ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage { content, role, name })
                }
            },
            other => other
        };
        ChatMessageThing { date: self.date, msg}
    }
    pub fn get_for_api(&self, now: DateTime<Utc>) -> ChatCompletionRequestMessage {
        if now - self.date < chrono::Duration::hours(1) {
            return self.msg.clone();
        }
        match &self.msg {
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Array(arr),
                role,
                name,
            }) => {
                let new_arr = arr
                    .iter()
                    .filter(|elem| {
                        matches!(elem, ChatCompletionRequestMessageContentPart::Text(..))
                    })
                    .cloned()
                    .collect();
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: ChatCompletionRequestUserMessageContent::Array(new_arr),
                    role: *role,
                    name: name.clone(),
                })
            }
            _ => self.msg.clone(),
        }
    }
    pub fn get_as_irc_format(&self) -> Option<&str> {
        match &self.msg {
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content,
                ..
            }) => Some(content.as_str()),
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content,
                ..
            }) => match content {
                ChatCompletionRequestUserMessageContent::Text(s) => Some(s),
                ChatCompletionRequestUserMessageContent::Array(arr) => arr
                    .iter()
                    .filter_map(|part| {
                        if let ChatCompletionRequestMessageContentPart::Text(s) = part {
                            Some(s.text.as_str())
                        } else {
                            None
                        }
                    })
                    .next(),
            },
            ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                content,
                ..
            }) => content.as_deref(),
            ChatCompletionRequestMessage::Tool(_) => None,
            ChatCompletionRequestMessage::Function(_) => None,
        }
    }
}

/// Upload some content to up.em32.site and return a URL
///
///
pub async fn upload_content(data: Vec<u8>, content_type: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder().build()?;

    let upload_resp = client
        .put("https://up.em32.site")
        .header("Content-Type", content_type)
        .body(data)
        .send()
        .await
        .context("Failed to upload text")?;

    let url = upload_resp.text().await?;
    if url.starts_with("https://") {
        return Ok(url);
    }
    anyhow::bail!("Unexpected error uploading")
}

#[tokio::test]
async fn test_upload() {
    let data = "hello world";
    let url = upload_content(data.as_bytes().to_vec(), "text/plain; charset=utf-8")
        .await
        .unwrap();
    println!("{url}");
}

pub fn get_prompt(key: &str) -> anyhow::Result<String> {
    let file = File::open("prompts.json")?;
    let mut prompts: HashMap<String, String> = serde_json::from_reader(file)?;

    Ok(prompts.remove(key).context("Prompt not found")?)
}

pub async fn generate_interjection(channel_messages: &[ChatMessageThing]) -> anyhow::Result<Option<String>> {

    let mut all_msg = String::new();
    for msg in channel_messages.iter().filter_map(|msg| msg.get_as_irc_format()) {
        all_msg.push_str(msg);
        all_msg.push('\n');
    }
    dbg!(&all_msg);

    let instruction = get_prompt("interject")?;

    let completion_messages = vec![
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(
                instruction.replace("{AB}", "below"),
            ),
            role: async_openai::types::Role::User,
            name: None,
        }),
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(all_msg),
            role: async_openai::types::Role::User,
            name: None,
        }),
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(
                instruction.replace("{AB}", "above"),
            ),
            role: async_openai::types::Role::User,
            name: None,
        }),
    ];

    let resp = openai::get_chat(completion_messages, Some("gpt-4o"), Some(0.8)).await?;
    dbg!(&resp);

    if let Some(m) = resp.get(0) {
        if let Some(m) = &m.content {
            if m.contains("no comment") {
                return Ok(None);
            }
            return Ok(Some(m.to_string()));
        }
    }
    Ok(None)
}

pub async fn generate_image_prompt(channel_messages: &[ChatMessageThing]) -> anyhow::Result<Option<String>> {

    let mut all_msg = String::new();
    for msg in channel_messages.iter().filter_map(|msg| msg.get_as_irc_format()) {
        all_msg.push_str(msg);
        all_msg.push('\n');
    }

    let instruction = get_prompt("image")?;

    let completion_messages = vec![
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(
                instruction.replace("{AB}", "below"),
            ),
            role: async_openai::types::Role::User,
            name: None,
        }),
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(all_msg),
            role: async_openai::types::Role::User,
            name: None,
        }),
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(
                instruction.replace("{AB}", "above"),
            ),
            role: async_openai::types::Role::User,
            name: None,
        }),
    ];

    let resp = openai::get_chat(completion_messages, Some("gpt-4o"), Some(0.8)).await?;
    dbg!(&resp);

    if let Some(m) = resp.get(0) {
        if let Some(m) = &m.content {
            if m.contains("no image") {
                return Ok(None);
            }
            return Ok(Some(openai::get_image(m.trim_matches('"')).await?))
        }
    }
    Ok(None)
}
