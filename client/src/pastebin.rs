use std::error::Error;

use gloo_net::http::Request;

use crate::BACKEND_URL;

/// Creates a new paste on <https://pastebin.com>. Returns the id of the created paste.
pub async fn new_paste(code: &str) -> Result<String, Box<dyn Error>> {
    Ok(Request::post(&format!("{BACKEND_URL}/paste"))
        .body(format!("code={code}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await?
        .text()
        .await?)
}

pub async fn get_paste(url: &str) -> Result<String, Box<dyn Error>> {
    Ok(Request::get(url).send().await?.text().await?)
}
