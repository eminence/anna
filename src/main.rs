use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anna::upload_content;
use anyhow::{bail, Context};
use async_openai::types::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestFunctionMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionResponseMessage,
};
use async_openai::types::{
    ChatCompletionRequestMessageContentPart, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartText,
};
use chrono::{DateTime, Utc};
use futures::prelude::*;
use irc::client::prelude::*;
use openai::get_tts;
use serde::{Deserialize, Serialize};

const OPT_IN_ALL_CAPTURE: &[&str] = &[
    "achin",
    "aheadley",
    "Tunabrain",
    "agrif",
    "CounterPillow",
    "GizmoBot",
];
const BOTNAME: &str = "Charbot9000";
const BOTNAME_PREFIX1: &str = "Charbot9000:";
const BOTNAME_PREFIX2: &str = "Charbot9000,";
const BOTS_TO_IGNORE: &[&str] = &["EmceeOverviewer", "box-bot", "GizmoBot"];

mod openai;
mod secrets;

/// An atomic F32
///
/// This is a wrapper around an AtomicU32 that stores the f32 bits as a u32.
pub struct AtomicF32 {
    storage: AtomicU32,
}
impl AtomicF32 {
    const fn init() -> Self {
        Self {
            storage: AtomicU32::new(0),
        }
    }
    pub fn new(value: f32) -> Self {
        let as_u32 = value.to_bits();
        Self {
            storage: AtomicU32::new(as_u32),
        }
    }
    pub fn store(&self, value: f32) {
        let as_u32 = value.to_bits();
        self.storage.store(as_u32, Ordering::SeqCst)
    }
    pub fn load(&self) -> f32 {
        let as_u32 = self.storage.load(Ordering::SeqCst);
        f32::from_bits(as_u32)
    }
}

static TEMPERATURE: AtomicF32 = AtomicF32::init();

// #[derive(Debug)]
// pub enum IRCSender {
//     /// A message generated by another IRC user
//     Other(String),
//     /// A message generated by openAI (aka this bot)
//     Myself,
// }

// #[derive(Debug)]
// pub struct IRCMessage {
//     sender: IRCSender,
//     message: String,
// }

// #[derive(Debug)]
// pub enum IRCMessage {
//     AssistantMessage { content: String },
//     AssistantFunction { name: String}
//     User { nick: String, content: String },
//     Function { name: String, content: String },
// }
// impl IRCMessage {
//     fn as_chat_msg(&self) -> ChatMessage {
//         match self {
//             IRCMessage::Assistant { content } => ChatMessage {
//                 role: openai::ChatCompletionRole::Assistant,
//                 content: Some(content.to_string()),
//                 name: None,
//                 function_call: None,
//             },
//             IRCMessage::User { nick, content } => ChatMessage {
//                 role: openai::ChatCompletionRole::User,
//                 content: Some(format!("<{}> {}", nick, content)),
//                 name: None,
//                 function_call: None,
//             },
//             IRCMessage::Function { name, content } => ChatMessage {
//                 role: openai::ChatCompletionRole::Function,
//                 content: Some(content.to_string()),
//                 name: Some(name.to_string()),
//                 function_call: None,
//             },
//         }
//     }
// }

pub fn trim_botname(msg: &str) -> &str {
    let msg = msg.trim_start();
    if let Some(x) = msg.strip_prefix(&format!("{BOTNAME}:")) {
        x.trim()
    } else if let Some(x) = msg.strip_prefix(&format!("<{BOTNAME}>")) {
        x.trim()
    } else {
        msg.trim()
    }
}

fn reponse_msg_to_request_msg(msg: ChatCompletionResponseMessage) -> ChatCompletionRequestMessage {
    #![allow(deprecated)]
    match msg.role {
        async_openai::types::Role::System => {
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content: msg.content.expect("Missing content"),
                role: msg.role,
                name: None,
            })
        }
        async_openai::types::Role::User => {
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: msg.content.expect("Missing content").into(),
                role: msg.role,
                name: None,
            })
        }
        async_openai::types::Role::Assistant => {
            ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                content: msg.content,
                role: msg.role,
                tool_calls: msg.tool_calls,
                function_call: msg.function_call,
                name: None,
            })
        }
        async_openai::types::Role::Tool => {
            ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                role: msg.role,
                content: msg.content.expect("Missing content"),
                tool_call_id: msg.tool_calls.unwrap().pop().unwrap().id,
            })
        }
        async_openai::types::Role::Function => {
            ChatCompletionRequestMessage::Function(ChatCompletionRequestFunctionMessage {
                role: msg.role,
                content: msg.content,
                name: msg.function_call.unwrap().name,
            })
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessageThing {
    /// When this message was generated
    date: DateTime<Utc>,
    msg: ChatCompletionRequestMessage,
}

impl ChatMessageThing {
    pub fn new_now(msg: ChatCompletionRequestMessage) -> Self {
        Self {
            date: Utc::now(),
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

/// Contains a list of all relevant messages for a given IRC channel
#[derive(Debug, Clone)]
pub struct MessageMap {
    inner: Arc<Mutex<HashMap<String, VecDeque<ChatMessageThing>>>>,
    client: reqwest::Client,
}

impl Default for MessageMap {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .user_agent("anna/1.0.0")
            .build()
            .unwrap();
        Self {
            inner: Default::default(),
            client,
        }
    }
}

impl MessageMap {
    pub async fn get_content_type(&self, url: &str) -> anyhow::Result<String> {
        // First, try a head request
        if let Ok(resp) = self.client.head(url).send().await {
            // extract the Content-Type header if the response was successful
            if dbg!(resp.status()).is_success() {
                if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
                    return Ok(ct.to_str()?.to_owned());
                }
            }
            println!("Retrying with GET request");

            // if the resp is a 404, then don't try a GET request
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("404");
            }
        }

        // if the head request failed, try a GET request
        let resp = self.client.get(url).send().await?;

        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|ct| ct.to_str().ok().map(|s| s.to_owned()))
            .context("Failed to get content type")?;

        // let body = resp.text().await?;
        // println!("Got body: {body}");

        Ok(ct)
    }
    pub async fn extract_image_urls(&self, sender: &str, message: &str) -> Vec<ChatMessageThing> {
        let mut m = Vec::new();

        let urls: Vec<_> = message
            .split_ascii_whitespace()
            .filter(|s| s.starts_with("https://"))
            .collect();

        if urls.is_empty() {
            let msg = ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(format!(
                    "<{sender}> {message}"
                )),
                role: async_openai::types::Role::User,
                name: Some(sender.to_string()),
            });
            m.push(ChatMessageThing::new_now(msg));
        } else {
            let mut content: Vec<ChatCompletionRequestMessageContentPart> =
                vec![ChatCompletionRequestMessageContentPartText::from(format!(
                    "<{sender}> {message}"
                ))
                .into()];
            for url in urls {
                dbg!(&url);
                if let Some(ct) = self.get_content_type(url).await.ok() {
                    dbg!(&ct);
                    if ct.starts_with("image/") {
                        content.push(
                            ChatCompletionRequestMessageContentPartImage {
                                r#type: "image_url".into(),
                                image_url: url.into(),
                            }
                            .into(),
                        );
                    }
                }
            }
            let msg = ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Array(content),
                role: async_openai::types::Role::User,
                name: Some(sender.to_string()),
            });
            m.push(ChatMessageThing::new_now(msg));
        }

        m
    }
    fn trim_message_for_age_and_contextsize(v: &mut VecDeque<ChatMessageThing>) {
        // remove any message older than 24 hours
        let now = Utc::now();
        while let Some(ChatMessageThing { date, .. }) = v.front() {
            if now.signed_duration_since(*date).num_hours() > 48 {
                v.pop_front();
            } else {
                break;
            }
        }

        // todo make sure we're below a certain context size (as measured in tokens)
    }
    pub async fn insert_usermsg(&mut self, channel: &str, sender: &str, message: &str) {
        let mut inner = self.inner.lock().expect("inner lock is poisoned");
        let m = if !inner.contains_key(channel) {
            inner.insert(channel.to_string(), Default::default());
            inner
                .get_mut(channel)
                .expect("Failed to get just inserted item")
        } else {
            inner.get_mut(channel).expect("Failed to get known item")
        };

        // look for things that look like URLs in the message

        m.extend(self.extract_image_urls(sender, message).await);

        MessageMap::trim_message_for_age_and_contextsize(m);

        // write out list of message to a file
        if let Ok(output) = File::create(format!("{channel}.json")) {
            let _ = serde_json::to_writer_pretty(output, m);
        }
    }
    pub fn insert_selfmsg(&mut self, channel: &str, messages: &[ChatCompletionResponseMessage]) {
        let mut inner = self.inner.lock().expect("inner lock is poisoned");
        let m = if !inner.contains_key(channel) {
            inner.insert(channel.to_string(), Default::default());
            inner
                .get_mut(channel)
                .expect("Failed to get just inserted item")
        } else {
            inner.get_mut(channel).expect("Failed to get known item")
        };

        for msg in messages {
            m.push_back(ChatMessageThing::new_now(reponse_msg_to_request_msg(
                msg.to_owned(),
            )));
        }

        MessageMap::trim_message_for_age_and_contextsize(m);

        // write out list of message to a file
        if let Ok(output) = File::create(format!("{channel}.json")) {
            let _ = serde_json::to_writer_pretty(output, m);
        }
    }
    pub fn clear_chat_message(&self, channel: &str) {
        let mut inner = self.inner.lock().expect("inner lock is poisoned");
        if let Some(list) = inner.get_mut(channel) {
            list.clear();
        }
    }
    pub fn get_chat_messages(
        &self,
        channel: &str,
        all_context: bool,
    ) -> Vec<ChatCompletionRequestMessage> {
        let inner = self.inner.lock().expect("inner lock is poisoned");
        let mut v = Vec::new();

        // When converting into a list to sent to the API, don't send images older than
        // an hour, in order to keep context size down and speed up processing
        let now = Utc::now();
        if let Some(list) = inner.get(channel) {
            if all_context {
                v.extend(list.iter().map(|cmt| cmt.get_for_api(now)));
                // for msg in list {
                //     v.push(msg.clone());
                // }
            } else if let Some(cmt) = list.back() {
                v.push(cmt.get_for_api(now));
            }
        }

        v
    }
}

fn boolify(s: Option<&str>) -> Option<bool> {
    if let Some(s) = s {
        match s {
            "y" | "yes" | "true" | "on" => Some(true),
            "n" | "no" | "false" | "off" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

fn get_chat_instruction(line: &str) -> Option<ChatInstruction> {
    // defaults
    let mut inst = ChatInstruction {
        msg: line.trim(),
        temp: TEMPERATURE.load(),
        context: true,
        save: true,
        pastebin: false,
        tts: false,
    };

    if let Some(data) = line.trim().strip_prefix("!chat") {
        if data.is_empty() {
            return Some(ChatInstruction::default(""));
        }
        // multiple parsing options, because why not
        if data.starts_with(['/', ':']) {
            let mut split = data[1..].splitn(2, ' ');
            let cmds = split.next().unwrap();

            for cmd in cmds.split([':', ',', '/']) {
                inst.update(cmd);
            }

            if let Some(rest) = split.next() {
                inst.msg = rest.trim();
            } else {
                inst.msg = ""
            }
        } else {
            // maybe we have !chat --foo=bar --baz syntax
            let mut skipped_words = 0;
            for (idx, cmd) in data.split_ascii_whitespace().enumerate() {
                if let Some(cmd) = cmd.strip_prefix("--") {
                    inst.update(cmd);
                } else {
                    skipped_words = idx;
                    break;
                }
            }
            inst.msg = data
                .trim()
                .splitn(skipped_words + 1, ' ')
                .last()
                .unwrap()
                .trim();
        }
    } else if let Some(data) = line
        .strip_prefix(BOTNAME_PREFIX1)
        .or_else(|| line.strip_prefix(BOTNAME_PREFIX2))
    {
        inst.msg = data.trim();
    } else {
        return None;
    }
    Some(inst)
}

#[derive(Debug, Copy, Clone)]
struct ChatInstruction<'a> {
    msg: &'a str,
    temp: f32,
    /// Whether or not to send previous messages as context
    context: bool,
    /// Whether or not to save this message and its reply as context
    save: bool,
    /// Whether or not to send only a pastebin link
    pastebin: bool,
    /// Whether to send the reply as audio
    tts: bool,
}

impl<'a> ChatInstruction<'a> {
    pub fn default(s: &str) -> ChatInstruction {
        ChatInstruction {
            msg: s,
            temp: TEMPERATURE.load(),
            context: true,
            save: true,
            pastebin: false,
            tts: false,
        }
    }
    /// Updates this object
    ///
    /// cmd is somse sting of the form "key" or "key=value"
    pub fn update(&mut self, cmd: &str) {
        let mut s = cmd.splitn(2, '=');
        let param = s.next().unwrap();
        match param {
            "context" => {
                if let Some(val) = boolify(s.next()) {
                    self.context = val
                }
            }
            "save" => {
                if let Some(val) = boolify(s.next()) {
                    self.save = val
                }
            }
            "paste" | "pastebin" => {
                self.pastebin = boolify(s.next()).unwrap_or(true);
            }
            "temp" => {
                if let Some(val) = s.next().and_then(|s| s.parse::<f32>().ok()) {
                    self.temp = val.clamp(0.0, 2.0)
                }
            }
            "tts" => {
                self.tts = boolify(s.next()).unwrap_or(true);
            }
            _ => (),
        }
    }
}

// Takes all owned parameters because we'll spawn an async closure in here
fn spawn_chat_completion_inner<'a>(
    for_chat: Vec<ChatCompletionRequestMessage>,
    inst: ChatInstruction<'a>,
    resp_target: String,
    target: String,
    sender: Sender,
    source_nick: String,
    mut message_map: MessageMap,
) {
    tokio::spawn(async move {
        match openai::get_chat(for_chat, None, inst.temp).await {
            Ok(resp) => {
                dbg!(&resp);
                if inst.save {
                    message_map.insert_selfmsg(&target, &resp);
                }
                // we need to save all messages, but only the last one will be sent back to IRC
                match resp.last() {
                    Some(ChatCompletionResponseMessage {
                        content: Some(resp_content),
                        ..
                    }) => {
                        if inst.pastebin {
                            match upload_content(
                                resp_content.as_bytes().to_vec(),
                                "text/plain; charset=utf-8",
                            )
                            .await
                            {
                                Ok(url) => {
                                    let _ = sender.send_privmsg(
                                        &resp_target,
                                        format!("{source_nick}: {url}",),
                                    );
                                }
                                Err(e) => {
                                    dbg!(e);
                                }
                            }
                        } else if inst.tts {
                            match get_tts(&resp_content).await {
                                Ok(url) => {
                                    let _ = sender.send_privmsg(
                                        &resp_target,
                                        format!("{source_nick}: {url}"),
                                    );
                                }
                                Err(e) => {
                                    dbg!(e);
                                }
                            }
                        } else {
                            send_possibly_long_message(
                                sender,
                                &resp_target,
                                trim_botname(resp_content),
                            )
                            .await;
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => {
                println!("Error getting chat from openai:");
                println!("{e}");
                let _ = sender.send_privmsg(
                    &resp_target,
                    format!("{source_nick}: Error getting chat from openai: {e}"),
                );
            }
        }
    });
}

fn spawn_chat_completion<'a>(
    for_chat: Vec<ChatCompletionRequestMessage>,
    inst: ChatInstruction<'a>,
    resp_target: impl ToString,
    target: impl ToString,
    sender: Sender,
    source_nick: impl ToString,
    message_map: MessageMap,
) {
    spawn_chat_completion_inner(
        for_chat,
        inst,
        resp_target.to_string(),
        target.to_string(),
        sender,
        source_nick.to_string(),
        message_map,
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config {
        owners: vec!["achin".into()],
        nickname: Some(BOTNAME.into()),
        channels: vec!["##em32".into(), "#overviewer".into()],
        server: Some("irc.libera.chat".into()),
        use_tls: Some(true),
        ..Default::default()
    };

    TEMPERATURE.store(1.0);

    let mut client = Client::from_config(config).await?;

    // keeps a list of the past 50 messages in a chat room
    let mut message_map = MessageMap::default();

    let mut stream = client.stream()?;
    let sender = client.sender();
    client.identify()?;

    loop {
        let message: Message = stream.select_next_some().await?;
        match message.command {
            Command::PING(..) | Command::PONG(..) => continue,
            _ => (),
        }
        if let Command::ERROR(..) = message.command {
            break;
        }
        if let Command::PRIVMSG(target, msg) = &message.command {
            let Some(source_nick) = message.source_nickname() else {
                continue;
            };
            if BOTS_TO_IGNORE.contains(&source_nick) {
                // to prevent annoying bot loops, never listen to other robots
                continue;
            }

            if let Some(resp_target) = message.response_target() {
                if "achin" == source_nick {
                    if msg.contains("go quit") || msg.starts_with("!quit") {
                        break;
                    }
                    if let Some(to_join) = msg.strip_prefix("!join ") {
                        sender.send_join(to_join.trim())?;
                        continue;
                    }
                    if let Some(to_part) = msg.strip_prefix("!part ") {
                        sender.send_part(to_part.trim())?;
                        continue;
                    }
                }

                if let Some(to_echo) = msg.strip_prefix("!echo ") {
                    sender.send_privmsg(resp_target, to_echo.trim())?;
                    continue;
                } else if let Some(temp_str) = msg.strip_prefix("!set_temp ") {
                    if let Ok(temp) = temp_str.parse::<f32>() {
                        if temp.is_finite() {
                            let temp = temp.clamp(0.0, 2.0);
                            TEMPERATURE.store(temp);
                            sender
                                .send_privmsg(resp_target, format!("Temperature is now {temp}"))?;
                        } else {
                            sender.send_privmsg(resp_target, "What are you trying to do?")?;
                        }
                    } else {
                        sender.send_privmsg(
                            resp_target,
                            format!("Failed to parse '{temp_str}' as a float"),
                        )?;
                    }
                    continue;
                } else if msg.starts_with("!get_temp") {
                    sender.send_privmsg(
                        resp_target,
                        format!("Current global temp is {}", TEMPERATURE.load()),
                    )?;
                    continue;
                } else if let Some(msg) = msg.strip_prefix("!tts ") {
                    let sender = sender.clone();
                    let msg = msg.to_string();
                    let resp_target = resp_target.to_string();
                    tokio::spawn(async move {
                        match get_tts(&msg).await {
                            Ok(url) => sender.send_privmsg(resp_target, url),
                            Err(e) => sender.send_privmsg(resp_target, format!("Error: {e}")),
                        }
                    });
                } else if let Some(msg) = msg.strip_prefix("!translate ") {
                    let sender = sender.clone();
                    let resp_target = resp_target.to_string();
                    let mut split = msg.splitn(2, ' ');
                    let url = split.next().unwrap_or("");
                    let prompt = split.next();
                    if url.starts_with("https://") {
                        let url = url.to_string();
                        let prompt = prompt.map(|s| s.to_string());
                        tokio::spawn(async move {
                            match openai::get_translation(&url, prompt).await {
                                Ok(translated) => {
                                    send_possibly_long_message(sender, &resp_target, &translated)
                                        .await;
                                }
                                Err(e) => {
                                    let _ = sender.send_privmsg(resp_target, format!("Error: {e}"));
                                }
                            }
                        });
                    }
                } else if let Some(msg) = msg.strip_prefix("!transcribe ") {
                    let sender = sender.clone();
                    let resp_target = resp_target.to_string();
                    let mut split = msg.splitn(2, ' ');
                    let url = split.next().unwrap_or("");
                    let prompt = split.next();
                    if url.starts_with("https://") {
                        let url = url.to_string();
                        let prompt = prompt.map(|s| s.to_string());
                        tokio::spawn(async move {
                            match openai::get_transcription(&url, prompt).await {
                                Ok(translated) => {
                                    send_possibly_long_message(sender, &resp_target, &translated)
                                        .await;
                                }
                                Err(e) => {
                                    let _ = sender.send_privmsg(resp_target, format!("Error: {e}"));
                                }
                            }
                        });
                    }
                } else if let Some(inst) = get_chat_instruction(msg) {
                    dbg!(&inst);
                    if inst.save && !inst.msg.trim().is_empty() {
                        message_map
                            .insert_usermsg(target, source_nick, inst.msg.trim())
                            .await;
                    }

                    // get a list of all known messages for the given channel (or only the last message if inst.context = false)
                    let mut for_chat = message_map.get_chat_messages(target, inst.context);
                    if !inst.save {
                        // our message wasn't inserted into the message map, so we have to explictly append it to what we send to openai
                        for_chat.extend(
                            message_map
                                .extract_image_urls(source_nick, inst.msg)
                                .await
                                .into_iter()
                                .map(|cmt| cmt.msg),
                        );
                    }
                    dbg!(&for_chat);
                    spawn_chat_completion(
                        for_chat,
                        inst,
                        resp_target,
                        target,
                        sender.clone(),
                        source_nick,
                        message_map.clone(),
                    );

                    continue;
                } else if let Some(prompt) = msg.strip_prefix("!img ") {
                    let cloned_sender = sender.clone();
                    let resp_target = resp_target.to_string();
                    let prompt = prompt.to_string();
                    let source_nick = source_nick.to_string();
                    tokio::spawn(async move {
                        match openai::get_image(&prompt).await {
                            Ok(url) => {
                                let _ = cloned_sender.send_privmsg(
                                    resp_target,
                                    format!("{}...: {url}", &prompt[..25.min(prompt.len())]),
                                );
                            }
                            Err(e) => {
                                println!("Error getting image from openai:");
                                println!("{e}");
                                let _ = cloned_sender.send_privmsg(
                                    &resp_target,
                                    format!("{source_nick}: Error getting image from openai: {e}"),
                                );
                            }
                        }
                    });

                    continue;
                } else if msg.starts_with("!clearctx") {
                    message_map.clear_chat_message(resp_target);
                    sender.send_privmsg(
                        resp_target,
                        format!("Clearing list of saved context for {resp_target}"),
                    )?;
                }
            }
            if target.starts_with('#') {
                // only certain users are comfortable with all their messages being used
                if OPT_IN_ALL_CAPTURE.contains(&source_nick) {
                    message_map.insert_usermsg(target, source_nick, msg).await;
                }
            }
        }
    }

    client.send_quit("Bye")?;

    Ok(())
}

async fn send_possibly_long_message(sender: Sender, resp_target: &str, msg: &str) {
    let mut length = 0;
    for line in split_long_message_for_irc(msg).iter() {
        length += 1 + (line.trim().len() as f32 / 150.0).floor() as i32;
        if length < 8 {
            let _ = sender.send_privmsg(resp_target, line.trim());
        } else {
            // upload
            if let Ok(url) =
                upload_content(msg.as_bytes().to_vec(), "text/plain; charset=utf-8").await
            {
                let _ = sender.send_privmsg(
                    &resp_target,
                    format!("(there were more lines in the reply, read more at {url})"),
                );
            } else {
                let _ = sender.send_privmsg(&resp_target, "(there were more lines in the reply, but there was an error uploading the content)");
            }
            break;
        }
    }
}

fn split_long_message_for_irc(msg: &str) -> Vec<String> {
    msg.lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| textwrap::wrap(l, 400))
        .map(|c| {
            c.chars()
                .filter(|c| !c.is_ascii_control() || c.is_ascii_whitespace())
                .collect()
        })
        .collect()
}

#[test]
fn test_line_split() {
    let long_line = "Charbot9000: Interesting idea, @agrif! Here's a story about how Nut runs for president with Coco as his running mate:\n\nAfter his heroic deeds in the village battle, Nut became a beloved figure among the people. His unwavering sense of justice and courage inspired many, and soon, he found himself being encouraged to run for president. At first, Nut was hesitant. He had never considered a life in politics before, and he wasn't sure if he was cut out for it. But with the support of his friends and loved ones, Nut eventually decided to throw his hat into the ring. To help him on his campaign, Nut turned to his old friend Coco. Although Coco was still just a coconut, Nut knew that his intelligence and charm would be a valuable asset on the campaign trail. So, Nut named Coco as his running mate and the two began their journey to the White House. Together, Nut and Coco traveled across the country, meeting with voters and spreading their message of hope and unity. Nut's bold vision for a better world, combined with Coco's quick wit and infectious personality, made them a popular duo among the people. Despite facing tough opposition from other candidates, Nut and Coco never lost sight of their values. They ran a clean, honest campaign and focused on the issues that mattered most to the people. And in the end, their hard work paid off - Nut and Coco won the election in a landslide. As Nut was sworn in as the new president of the United States, he knew that he had a lot of work to do. But with Coco by his side, he was confident that they could make a real difference in the world. And as they looked out at the sea of cheering supporters before them, Nut and Coco knew that anything was possible with a little courage and a lot of heart.";
    for line in split_long_message_for_irc(long_line) {
        println!("==> {line}");
    }
}

#[test]
fn test_atomic_f32() {
    let x = AtomicF32::new(0.2);
    assert_eq!(x.load(), 0.2);

    x.store(1.5);
    assert_eq!(x.load(), 1.5);
}

#[test]
fn test_chat_instruction() {
    let inst = get_chat_instruction("hello world");
    assert!(inst.is_none());

    let inst = get_chat_instruction("!chat hello world").unwrap();
    assert_eq!(inst.msg, "hello world");

    let inst = get_chat_instruction("Charbot9000: hello world").unwrap();
    assert_eq!(inst.msg, "hello world");
    let inst = get_chat_instruction("Charbot9000, hello world").unwrap();
    assert_eq!(inst.msg, "hello world");

    let inst = get_chat_instruction("!chat:temp=1").unwrap();
    assert_eq!(inst.temp, 1.0);
    assert!(inst.context);
    assert!(inst.save);
    assert!(!inst.pastebin);
    assert!(inst.msg.is_empty());

    let inst = get_chat_instruction("!chat:temp=0.5,context=no hello world").unwrap();
    assert_eq!(inst.temp, 0.5);
    assert!(!inst.context);
    assert!(inst.save);
    assert!(!inst.pastebin);
    assert_eq!(inst.msg, "hello world");

    let inst = get_chat_instruction("!chat/temp=55/save hello world").unwrap();
    assert_eq!(inst.temp, 2.0);
    assert!(inst.context);
    assert!(inst.save);
    assert!(!inst.pastebin);
    assert_eq!(inst.msg, "hello world");

    let inst = get_chat_instruction("!chat --pastebin --save=no --temp=3 hello    world").unwrap();
    assert_eq!(inst.temp, 2.0);
    assert!(inst.context);
    assert!(!inst.save);
    assert!(inst.pastebin);
    assert!(!inst.tts);
    assert_eq!(inst.msg, "hello    world");

    let inst = get_chat_instruction("!chat --tts hello").unwrap();
    assert!(inst.tts);

    let inst = get_chat_instruction("!chat --tts=yes hello").unwrap();
    assert!(inst.tts);

    let inst = get_chat_instruction("!chat --tts=false hello").unwrap();
    assert!(!inst.tts);
}

#[tokio::test]
async fn test_image_detection() {
    let mut messages = MessageMap::default();

    messages
        .insert_usermsg(
            "#em32",
            "achin",
            "Please describe this URL: https://i.imgur.com/Sb4xdqa.jpeg",
        )
        .await;

    dbg!(messages.inner);
}

#[tokio::test]
async fn test_load_from_disk() -> anyhow::Result<()> {
    let f = File::open("#overviewer.json")?;

    let mut all_msg = String::new();
    let messages: Vec<ChatMessageThing> = serde_json::from_reader(f)?;
    for msg in messages.iter().filter_map(|msg| msg.get_as_irc_format()) {
        all_msg.push_str(msg);
        all_msg.push('\n');
    }

    let instruction = "Analyze the _AB_ IRC conversation for tone, content, and general sentiment.  Is there anything you can add to the conversation? If the conversation is lighthearted and jocular, you can add a whimsical comment, but only if it relates to the current conversation.  If the conversation is technical, you add a technically accurate and relevant comment.  It is acceptable to not and anything.  Reply with only the message to be added and nothing else.  If adding noting, then reply only with 'no comment'";

    let completion_messages = vec![
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(
                instruction.replace("_AB_", "below"),
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
                instruction.replace("_AB_", "above"),
            ),
            role: async_openai::types::Role::User,
            name: None,
        }),
    ];

    let resp = openai::get_chat(completion_messages, Some("gpt-4-0125-preview"), 1.0).await?;
    dbg!(resp);

    Ok(())
}
