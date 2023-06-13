use anyhow::Context;

pub mod plugins;

pub async fn upload_text(data: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder().build()?;

    let digest = md5::compute(data);

    let upload_resp = client
        .put(format!("https://up.em32.site/?hash={:x}", digest))
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(data.to_string())
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
    upload_text(data).await.unwrap();
}