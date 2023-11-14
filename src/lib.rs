use anyhow::Context;

// pub mod plugins;

pub mod wttr;

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
