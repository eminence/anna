use std::time::Duration;

use anna::upload_content;
use anyhow::{bail, Context};
use async_openai::{
    config::OpenAIConfig,
    types::{
        AudioInput, AudioResponseFormat, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessage, ChatCompletionResponseMessage,
        CreateChatCompletionRequest, CreateImageRequest, CreateTranscriptionRequest,
        CreateTranslationRequest, Image, ImageQuality,
    },
};
use chrono::Utc;
use schemars::JsonSchema;

pub const SYSTEM_PROMPT: &str = "You are chatbot in an online chat room.  There are multiple people in this chatroom, their names will appear in angle brackets.  You can answer questions, or extend the conversation with interesting comments.  Answer with short messages and do not repeat yourself. Be creative. Your operator is 'achin', and your own name is 'Charbot9000'.";

#[derive(JsonSchema)]
// Start function definitions
struct Evaluate {
    /// A mathmatical expression, like "4 * 3 - 2"
    pub input: String,
}

// async fn get_chat_helper(
//     client: &reqwest::Client,
//     chat: &ChatCompletions,
// ) -> anyhow::Result<ChatMessage> {
//     let req = client
//         .post("https://api.openai.com/v1/chat/completions")
//         .bearer_auth(crate::secrets::OPENAPI_KEY)
//         .json(&chat)
//         .send()
//         .await?;

//     if !req.status().is_success() {
//         anyhow::bail!(format!("API returned {}", req.status()))
//     }

//     let resp_text = req.text().await?;

//     let mut resp: ChatResponse = match serde_json::from_str(&resp_text) {
//         Ok(val) => val,
//         Err(e) => {
//             println!("error requesting chat:");
//             println!("{e}");
//             println!("{resp_text}");
//             anyhow::bail!("error requesting chat");
//         }
//     };
//     dbg!(&resp.usage);

//     let resp = resp
//         .choices
//         .pop()
//         .map(|x| x.message)
//         .context("No completions returned")?;

//     Ok(resp)
// }

/// Get the chat completions for the given chat messages
///
/// This can return multiple chat messages if a function was called
pub async fn get_chat(
    messages: Vec<ChatCompletionRequestMessage>,
    _temp: f32,
) -> anyhow::Result<Vec<ChatCompletionResponseMessage>> {
    let _start = std::time::Instant::now();
    println!(
        "Sending chat completion request ({} total messages) {:?}",
        messages.len(),
        messages.last()
    );
    let now = Utc::now();

    let mut m = vec![ChatCompletionRequestMessage::System(
        ChatCompletionRequestSystemMessage {
            role: async_openai::types::Role::System,
            content: format!("{}. Current date: {}", SYSTEM_PROMPT, now.date_naive()),
            name: None,
        },
    )];

    m.extend(messages);

    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let mut resp = client
        .chat()
        .create(CreateChatCompletionRequest {
            messages: m,
            model: "gpt-4-vision-preview".to_string(),
            max_tokens: Some(4096),
            // temperature: Some(temp),
            ..Default::default()
        })
        .await?;

    let resp_msg = resp.choices.pop().context("Missing a response")?.message;

    Ok(vec![resp_msg])
}

pub async fn get_image(prompt: &str) -> anyhow::Result<String> {
    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let resp = client
        .images()
        .create(CreateImageRequest {
            prompt: prompt.to_string(),
            model: Some(async_openai::types::ImageModel::DallE3),
            n: Some(1),
            quality: Some(ImageQuality::HD),
            ..Default::default()
        })
        .await?;

    for data in resp.data {
        if let Image::Url {
            url,
            revised_prompt: _,
        } = &*data
        {
            // download and rehost
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(60))
                .build()?;
            let resp = client.get(url).send().await?;

            let rehosted_url = upload_content(resp.bytes().await?.to_vec(), "image/png").await?;
            return Ok(rehosted_url);
        } else {
            bail!("Image data returned as b64json, not url")
        }
    }

    anyhow::bail!("unknown error")
}

/// Returns a URL to the uploaded speech
pub async fn get_tts(text: &str) -> anyhow::Result<String> {
    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let resp = client
        .audio()
        .speech(async_openai::types::CreateSpeechRequest {
            input: text.into(),
            model: async_openai::types::SpeechModel::Tts1Hd,
            voice: async_openai::types::Voice::Echo,
            response_format: Some(async_openai::types::SpeechResponseFormat::Opus),
            speed: None,
        })
        .await?;

    let rehosted_url = upload_content(resp.bytes.to_vec(), "audio/ogg").await?;

    Ok(format!("{rehosted_url}.ogg"))
}

pub async fn get_translation(audio_url: &str, prompt: Option<String>) -> anyhow::Result<String> {
    // filename is the name of the file to be translated
    let filename = audio_url.split('/').last().unwrap_or("unknown.ogg");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .user_agent("anna/1.0.0")
        .build()
        .unwrap();

    // download the audio adnd store as a Bytes object
    let resp = client.get(audio_url).send().await?;

    // make sure content type is audio:
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|ct| ct.to_str().ok().map(|s| s.to_owned()));
    if !matches!(ct, Some(s) if s.starts_with("audio/") || s.starts_with("video/")) {
        bail!("Content type is not audio")
    }

    let audio = resp.bytes().await?;

    let translation_request = CreateTranslationRequest {
        file: AudioInput::from_bytes(filename.into(), audio),
        model: "whisper-1".into(),
        prompt,
        response_format: Some(AudioResponseFormat::Json),
        temperature: None,
    };
    // dbg!(&translation_request);

    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let resp = client.audio().translate(translation_request).await?;

    Ok(resp.text)
}

pub async fn get_transcription(audio_url: &str, prompt: Option<String>) -> anyhow::Result<String> {
    // filename is the name of the file to be translated
    let filename = audio_url.split('/').last().unwrap_or("unknown.ogg");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // download the audio adnd store as a Bytes object
    let resp = client.get(audio_url).send().await?;

    // make sure content type is audio:
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|ct| ct.to_str().ok().map(|s| s.to_owned()));
    if !matches!(ct, Some(s) if s.starts_with("audio/") || s.starts_with("video/")) {
        bail!("Content type is not audio")
    }

    let audio = resp.bytes().await?;

    let translation_request = CreateTranscriptionRequest {
        file: AudioInput::from_bytes(filename.into(), audio),
        model: "whisper-1".into(),
        prompt,
        response_format: Some(AudioResponseFormat::Json),
        temperature: None,
        language: None,
        timestamp_granularities: None
    };
    // dbg!(&translation_request);

    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let resp = client.audio().transcribe(translation_request).await?;

    Ok(resp.text)
}

#[tokio::test]
async fn test_tts() {
    let url = get_tts("Hello, how are you doing on this fine evening?")
        .await
        .unwrap();

    println!("{url}")
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

// #[test]
// fn test_json_schemas() {
//     use schemars::{schema_for, JsonSchema};

//     #[derive(JsonSchema)]
//     struct WeatherInput {
//         pub city: String,
//         pub state: String,
//         pub country: Option<String>,
//     }

//     let my_func = FunctionDef {
//         name: "my_func".into(),
//         description: None,
//         parameters: schema_for!(WeatherInput),
//     };

//     let j = serde_json::to_string_pretty(&my_func).unwrap();
//     println!("{j}");

//     // let schema = ;
//     // println!("{}", serde_json::to_string_pretty(&schema).unwrap());
// }

// #[tokio::test]
// async fn test_openai_chat_functions() -> anyhow::Result<()> {
//     #[derive(JsonSchema)]
//     struct Evaluate {
//         /// A mathmatical expression, like "4 * 3 - 2"
//         pub input: String,
//     }

//     let chat = ChatCompletions {
//         model: "gpt-4-vision-preview".into(),
//         messages: vec![
//             ChatMessage {
//                 role: ChatCompletionRole::System,
//                 content: Some(SYSTEM_PROMPT.into()),
//                 name: None,
//                 function_call: None,
//             },
//             ChatMessage {
//                 role: ChatCompletionRole::User,
//                 content: Some("What is weather in Paris?".into()),
//                 name: None,
//                 function_call: None,
//             },
//         ],
//         temperature: Some(0.7),
//         functions: vec![
//             FunctionDef {
//                 name: "get_current_weather".into(),
//                 description: Some("Gets the current weather, given a city and state".into()),
//                 parameters: schema_for!(anna::wttr::WeatherInput),
//             },
//             FunctionDef {
//                 name: "evaluate_expression".into(),
//                 description: Some("Evaluates a mathematical expression".into()),
//                 parameters: schema_for!(Evaluate),
//             },
//         ],
//     };

//     println!("{}", serde_json::to_string_pretty(&chat).unwrap());

//     let client = reqwest::Client::new();
//     let req = client
//         .post("https://api.openai.com/v1/chat/completions")
//         .bearer_auth(crate::secrets::OPENAPI_KEY)
//         .json(&chat)
//         .send()
//         .await?;

//     // let resp = req.text().await?;
//     let resp: ChatResponse = req.json().await?;
//     println!("{resp:#?}");
//     // Ok(resp.choices.get(0).context("No chat completions returned")?.message)
//     Ok(())
// }
// #[tokio::test]
// async fn test_openai_crate() {
//     let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
//     let client = async_openai::Client::with_config(cfg);

//     let user_msg1 = ChatCompletionRequestUserMessageArgs::default()
//         .content(vec![ChatCompletionRequestMessageContentPart::Text(
//             "hello, please describe this image".into(),
//         )])
//         .build()
//         .unwrap();

//     let user_msg2 = ChatCompletionRequestUserMessageArgs::default()
//         .content(vec![ChatCompletionRequestMessageContentPartImage {
//             r#type: "image".into(),
//             image_url: "https://i.imgur.com/sPygpea.jpeg".into(),
//         }
//         .into()])
//         .build()
//         .unwrap();

//     let messages = vec![
//         ChatCompletionRequestMessage::User(user_msg1),
//         ChatCompletionRequestMessage::User(user_msg2),
//     ];
//     let chat_request = CreateChatCompletionRequest {
//         model: "gpt-4-vision-preview".to_string(),
//         max_tokens: Some(4096),
//         messages,
//         ..Default::default()
//     };

//     let resp = client.chat().create(chat_request).await;
//     let _ = dbg!(resp);
// }

#[tokio::test]
async fn test_embedding() {
    let cfg = OpenAIConfig::new().with_api_key(crate::secrets::OPENAPI_KEY);
    let client = async_openai::Client::with_config(cfg);

    let res = client
        .embeddings()
        .create(async_openai::types::CreateEmbeddingRequest {
            model: "text-embedding-ada-002".to_string(),
            input: async_openai::types::EmbeddingInput::String("hello, how are you?".to_string()),
            encoding_format: None,
            user: None,
            dimensions: None,
        })
        .await
        .unwrap();

    dbg!(res);
}
