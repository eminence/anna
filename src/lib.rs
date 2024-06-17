use std::{
    collections::HashMap,
    fs::File,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use async_openai::types::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestMessageContentPart, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent, Role,
};
use chrono::{DateTime, Utc};
// use numbat::markup::Formatter;
use serde::{Deserialize, Serialize};
use wasmtime::{
    component::ResourceAny,
    Store,
};

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
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content,
                role,
                name,
            }) => {
                if role == Role::User {
                    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                        content: ChatCompletionRequestUserMessageContent::Text(content),
                        role,
                        name,
                    })
                } else {
                    ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                        content,
                        role,
                        name,
                    })
                }
            }
            other => other,
        };
        ChatMessageThing {
            date: self.date,
            msg,
        }
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

pub async fn generate_interjection(
    channel_messages: &[ChatMessageThing],
) -> anyhow::Result<Option<String>> {
    let mut all_msg = String::new();
    for msg in channel_messages
        .iter()
        .filter_map(|msg| msg.get_as_irc_format())
    {
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

pub async fn generate_image_prompt(
    channel_messages: &[ChatMessageThing],
) -> anyhow::Result<Option<String>> {
    let mut all_msg = String::new();
    for msg in channel_messages
        .iter()
        .filter_map(|msg| msg.get_as_irc_format())
    {
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
            return Ok(Some(openai::get_image(m.trim_matches('"')).await?));
        }
    }
    Ok(None)
}

// struct IRCFormatter;

// impl numbat::markup::Formatter for IRCFormatter {
//     fn format_part(
//         &self,
//         numbat::markup::FormattedString(_output_type, format_type, text):  &numbat::markup::FormattedString,
//     ) -> String {
//         match format_type {
//             numbat::markup::FormatType::Whitespace => format!("{text}"),
//             numbat::markup::FormatType::Emphasized => format!("\x02{text}\x0f"),
//             numbat::markup::FormatType::Dimmed => format!("{text}"),
//             numbat::markup::FormatType::Text => format!("{text}"),
//             numbat::markup::FormatType::String => format!("\x0303{text}\x0f"),
//             numbat::markup::FormatType::Keyword => format!("\x0313{text}\x0f"),
//             numbat::markup::FormatType::Value => format!("\x0308{text}\x0f"),
//             numbat::markup::FormatType::Unit => format!("\x0311{text}\x0f"),
//             numbat::markup::FormatType::Identifier => format!("{text}"),
//             numbat::markup::FormatType::TypeIdentifier => format!("\x0312\x1d{text}\x0f"),
//             numbat::markup::FormatType::Operator => format!("\x02{text}\x0f"),
//             numbat::markup::FormatType::Decorator => format!("\x0303{text}\x0f"),
//         }
//     }
// }

// pub fn get_numbat_result(input: &str, ctx: &mut numbat::Context) -> anyhow::Result<String> {
//     let to_be_printed: Arc<Mutex<Vec<_>>> = Arc::new(Mutex::new(vec![]));
//     let to_be_printed_c = to_be_printed.clone();
//     let registry = ctx.dimension_registry().clone();
//     let mut settings = numbat::InterpreterSettings {
//         print_fn: Box::new(move |s: &numbat::markup::Markup| {
//             to_be_printed_c.lock().unwrap().push(s.clone());
//         }),
//     };
//     let (statements, result) =
//         ctx.interpret_with_settings(&mut settings, input, numbat::resolver::CodeSource::Text)?;

//     let mut s = String::new();
//     for statement in &statements {
//         let markup = numbat::pretty_print::PrettyPrint::pretty_print(statement);
//         s.push_str(&IRCFormatter.format(&markup, false));

//         // s.push_str(&format!(
//         //     "{}",

//         // ))
//     }

//     let r = result.to_markup(statements.last(), &registry, true, true);
//     s.push_str(&IRCFormatter.format(&r, false));
//     // s.push_str(r.to_string().as_str());

//     Ok(s)
// }

wasmtime::component::bindgen!({
    path: "world.wit",
    world: "example",
    async: false
});

struct MyState {
    ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
}

impl wasmtime_wasi::WasiView for MyState {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        &mut self.ctx
    }
}

impl MyState {
    fn new() -> Self {
        let table = wasmtime_wasi::ResourceTable::new();
        let ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .build();
        Self { ctx, table }
    }
}

pub struct NumbatComponent {
    store: Store<MyState>,
    inst: Example,
    inner_ctx: ResourceAny,
}

impl NumbatComponent {
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut config = wasmtime::Config::default();
        config.wasm_component_model(true);
        // config.async_support(true);

        let engine = wasmtime::Engine::new(&config)?;
        let mut linker = wasmtime::component::Linker::new(&engine);

        let wasi_view = MyState::new();

        let mut store = wasmtime::Store::new(&engine, wasi_view);

        let component = wasmtime::component::Component::from_file(&engine, path)?;

        wasmtime_wasi::add_to_linker_sync(&mut linker)?;

        let (inst, _) = Example::instantiate(&mut store, &component, &linker)?;

        let x = inst.component_numbat_component_numbat();
        let y = x.ctx().call_constructor(&mut store)?;

        Ok(Self {
            store,
            inst,
            inner_ctx: y,
        })
    }

    pub fn eval(&mut self, input: &str) -> anyhow::Result<String> {
        let guest = self.inst.component_numbat_component_numbat();

        let output = guest
            .ctx()
            .call_eval(&mut self.store, self.inner_ctx, input)?
            .map_err(|s| anyhow::anyhow!(s))?;

        Ok(output)
    }
}

#[test]
fn test_wasmtime() -> anyhow::Result<()> {
    let mut comp = NumbatComponent::new("numbat_component.wasm")?;
    let x = comp.eval("let x = 1")?;
    dbg!(x);
    let y = comp.eval("x * 2")?;
    dbg!(y);

    let z = comp.eval("panic");
    dbg!(z);

    // let mut comp = NumbatComponent::new("numbat_component.wasm")?;
    // let y = comp.eval("x * 2")?;
    // dbg!(y);

    Ok(())
}
