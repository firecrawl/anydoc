//! `ocr: 'hosted'`: the node package's `parseHosted`, minus the environment
//! fallbacks browsers and edge runtimes do not have.

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::ConvertOptions;

const API_URL: &str = "https://api.firecrawl.dev";
const TIMEOUT_MS: u32 = 300_000;
const PARSE_OPTIONS: &str = concat!(
    r#"{"parsers":[{"type":"pdf","mode":"auto"}],"origin":"anydoc@"#,
    env!("CARGO_PKG_VERSION"),
    r#""}"#
);

#[wasm_bindgen]
extern "C" {
    // The global one, not `window.fetch`: workers, Node and edge runtimes
    // have no `window`.
    #[wasm_bindgen(js_name = fetch, catch)]
    fn fetch(url: &str, init: &web_sys::RequestInit) -> Result<js_sys::Promise, JsValue>;
}

pub(crate) fn wants_hosted(options: &ConvertOptions) -> bool {
    get(options, "ocr").as_string().as_deref() == Some("hosted")
}

// The whole document goes, not only the pages that need OCR: Parse has no
// page selection.
pub(crate) async fn parse_hosted(
    bytes: &[u8],
    options: &ConvertOptions,
) -> Result<String, JsValue> {
    let api_key = get(options, "apiKey").as_string().filter(|key| !key.is_empty());
    let api_url = get(options, "apiUrl").as_string().unwrap_or_else(|| API_URL.to_owned());
    let url = format!("{}/v2/parse", api_url.strip_suffix('/').unwrap_or(&api_url));

    let (response, reply) = send(&url, bytes, api_key.as_deref()).await.map_err(|error| {
        hosted_error(&format!("Firecrawl Parse: {}", message(&error)), Some(error))
    })?;
    if !response.ok() || get(&reply, "success") != JsValue::TRUE {
        let detail = get(&reply, "error").as_string().unwrap_or_else(|| response.status_text());
        return Err(hosted_error(&describe(response.status(), &detail, api_key.is_some()), None));
    }
    match get(&get(&reply, "data"), "markdown").as_string() {
        Some(markdown) if markdown.ends_with('\n') => Ok(markdown),
        Some(markdown) if !markdown.is_empty() => Ok(markdown + "\n"),
        _ => Err(hosted_error("Firecrawl Parse returned no Markdown", None)),
    }
}

/// The response, and its JSON body or `null` when it has none.
async fn send(
    url: &str,
    bytes: &[u8],
    api_key: Option<&str>,
) -> Result<(web_sys::Response, JsValue), JsValue> {
    let blob_type = web_sys::BlobPropertyBag::new();
    blob_type.set_type("application/pdf");
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let file = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &blob_type)?;
    let body = web_sys::FormData::new()?;
    body.append_with_str("options", PARSE_OPTIONS)?;
    body.append_with_blob_and_filename("file", &file, "document.pdf")?;
    let headers = web_sys::Headers::new()?;
    if let Some(key) = api_key {
        headers.set("authorization", &format!("Bearer {key}"))?;
    }
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&body);
    init.set_headers(&headers);
    init.set_signal(Some(&web_sys::AbortSignal::timeout_with_u32(TIMEOUT_MS)));

    let response: web_sys::Response = JsFuture::from(fetch(url, &init)?).await?.unchecked_into();
    let reply = match response.json() {
        Ok(json) => JsFuture::from(json).await.unwrap_or(JsValue::NULL),
        Err(_) => JsValue::NULL,
    };
    Ok((response, reply))
}

fn describe(status: u16, detail: &str, keyed: bool) -> String {
    match status {
        401 => format!("Firecrawl Parse rejected the API key: {detail}"),
        402 => format!("Firecrawl Parse is out of credits: {detail}"),
        429 if keyed => format!("Firecrawl Parse rate limit reached: {detail}"),
        429 => format!("Firecrawl Parse keyless limit reached, pass apiKey: {detail}"),
        _ => format!("Firecrawl Parse: {detail}"),
    }
}

fn hosted_error(message: &str, cause: Option<JsValue>) -> JsValue {
    let error = js_sys::Error::new(message);
    let _ = js_sys::Reflect::set(&error, &"code".into(), &"hosted".into());
    if let Some(cause) = cause {
        let _ = js_sys::Reflect::set(&error, &"cause".into(), &cause);
    }
    error.into()
}

/// `error.message`, which a timeout's `DOMException` carries as well.
fn message(error: &JsValue) -> String {
    get(error, "message").as_string().unwrap_or_else(|| format!("{error:?}"))
}

/// `target[key]`, or `undefined` when `target` is not an object.
fn get(target: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(target, &key.into()).unwrap_or(JsValue::UNDEFINED)
}
