use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use anna::upload_text;
use futures::prelude::*;
use irc::client::prelude::*;
use openai::ChatMessage;

const OPT_IN_ALL_CAPTURE: &[&str] = &["achin", "aheadley", "Tunabrain", "agrif", "CounterPillow"];
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

#[derive(Debug)]
pub enum IRCSender {
    Other(String),
    Myself,
}

#[derive(Debug)]
pub struct IRCMessage {
    sender: IRCSender,
    message: String,
}
impl IRCMessage {
    fn as_chat_msg(&self) -> ChatMessage {
        match &self.sender {
            IRCSender::Other(nick) => ChatMessage {
                role: openai::ChatCompletionRole::User,
                content: format!("<{}> {}", nick, self.message),
            },
            IRCSender::Myself => ChatMessage {
                role: openai::ChatCompletionRole::Assistant,
                content: self.message.to_string(),
            },
        }
    }
}

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

#[derive(Default, Debug, Clone)]
pub struct MessageMap {
    inner: Arc<Mutex<HashMap<String, VecDeque<IRCMessage>>>>,
}
impl MessageMap {
    pub fn insert_usermsg(&mut self, channel: &str, sender: &str, message: &str) {
        let mut inner = self.inner.lock().expect("inner lock is poisoned");
        let m = if !inner.contains_key(channel) {
            inner.insert(channel.to_string(), Default::default());
            inner
                .get_mut(channel)
                .expect("Failed to get just inserted item")
        } else {
            inner.get_mut(channel).expect("Failed to get known item")
        };
        m.push_back(IRCMessage {
            sender: IRCSender::Other(sender.to_string()),
            message: message.trim().to_string(),
        });
        if m.len() > 50 {
            m.pop_front();
        }
    }
    pub fn insert_selfmsg(&mut self, channel: &str, message: &str) {
        let mut inner = self.inner.lock().expect("inner lock is poisoned");
        let m = if !inner.contains_key(channel) {
            inner.insert(channel.to_string(), Default::default());
            inner
                .get_mut(channel)
                .expect("Failed to get just inserted item")
        } else {
            inner.get_mut(channel).expect("Failed to get known item")
        };

        let trimmed_msg = trim_botname(message);

        m.push_back(IRCMessage {
            sender: IRCSender::Myself,
            message: trimmed_msg.to_string(),
        });

        if m.len() > 50 {
            m.pop_front();
        }
    }
    pub fn get_chat_messages(&self, channel: &str, all_context: bool) -> Vec<ChatMessage> {
        let inner = self.inner.lock().expect("inner lock is poisoned");
        let mut v = Vec::new();

        if let Some(list) = inner.get(channel) {
            if all_context {
                for msg in list {
                    v.push(msg.as_chat_msg());
                }
            } else if let Some(elem) = list.back() {
                v.push(elem.as_chat_msg());
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
}

impl<'a> ChatInstruction<'a> {
    pub fn default(s: &str) -> ChatInstruction {
        ChatInstruction {
            msg: s,
            temp: TEMPERATURE.load(),
            context: true,
            save: true,
            pastebin: false,
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
            _ => (),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config {
        owners: vec!["achin".into()],
        nickname: Some(BOTNAME.into()),
        channels: vec!["##em32".into()],
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
                    if msg.contains("go quit") {
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
                }
                if let Some(temp_str) = msg.strip_prefix("!set_temp ") {
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
                }
                if msg.starts_with("!get_temp") {
                    sender.send_privmsg(
                        resp_target,
                        format!("Current global temp is {}", TEMPERATURE.load()),
                    )?;
                    continue;
                }
                if let Some(inst) = get_chat_instruction(msg) {
                    dbg!(&inst);
                    if inst.save && !inst.msg.trim().is_empty() {
                        message_map.insert_usermsg(target, source_nick, inst.msg.trim());
                    }

                    let mut for_chat = message_map.get_chat_messages(target, inst.context);
                    if !inst.save {
                        // our message wasn't inserted into the message map, so we have to explictly append it to what we send to openai
                        for_chat.push(
                            IRCMessage {
                                sender: IRCSender::Other(source_nick.to_string()),
                                message: inst.msg.trim().to_string(),
                            }
                            .as_chat_msg(),
                        );
                    }
                    dbg!(&for_chat);
                    // continue;
                    let resp_target = resp_target.to_string();
                    let target = target.to_string();
                    let sender = sender.clone();
                    let source_nick = source_nick.to_string();
                    let mut message_map = message_map.clone();
                    tokio::spawn(async move {
                        match openai::get_chat(for_chat, inst.temp).await {
                            Ok(resp) => {
                                if inst.save {
                                    message_map.insert_selfmsg(&target, &resp.content);
                                }
                                if inst.pastebin {
                                    match upload_text(&resp.content).await {
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
                                } else {
                                    let mut length = 0;
                                    for line in
                                        split_long_message_for_irc(trim_botname(&resp.content))
                                            .iter()
                                    {
                                        length +=
                                            1 + (line.trim().len() as f32 / 150.0).floor() as i32;
                                        if length < 8 {
                                            let _ = sender.send_privmsg(&resp_target, line.trim());
                                        } else {
                                            // upload
                                            if let Ok(url) = upload_text(&resp.content).await {
                                                let _ = sender.send_privmsg(&resp_target, format!("(there were more lines in the reply, read more at {url})"));
                                            } else {
                                                let _ = sender.send_privmsg(&resp_target, "(there were more lines in the reply, but I'm only sending the first few)");
                                            }
                                            break;
                                        }
                                    }
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

                    continue;
                }
                if let Some(prompt) = msg.strip_prefix("!img ") {
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
                }
            }
            if target.starts_with('#') {
                // only certain users are comfortable with all their messages being used
                if OPT_IN_ALL_CAPTURE.contains(&source_nick) {
                    message_map.insert_usermsg(target, source_nick, msg);
                }
            }
        }
    }

    client.send_quit("Bye")?;

    Ok(())
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
    assert_eq!(inst.msg, "hello    world");
}
