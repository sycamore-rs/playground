use std::error::Error;

use gloo_net::http::Request;
use js_sys::encode_uri_component;

use crate::BACKEND_URL;

/// Creates a new paste on <https://pastebin.com>. Returns the id of the created paste.
pub async fn new_paste(code: &str) -> Result<String, Box<dyn Error>> {
    let encoded = encode_uri_component(code);
    Ok(Request::post(&format!("{BACKEND_URL}/paste"))
        .body(format!("code={encoded}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await?
        .text()
        .await?)
}

pub async fn get_paste(url: &str) -> Result<String, Box<dyn Error>> {
    Ok(Request::get(url).send().await?.text().await?)
}
