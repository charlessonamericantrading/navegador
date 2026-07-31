use url::Url;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Patch => "PATCH",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkRequest {
    pub url: Url,
    pub method: Method,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl NetworkRequest {
    pub fn new(url_str: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(url_str)?;
        let mut headers = HashMap::new();
        headers.insert(
            "User-Agent".to_string(),
            "NextGenWebEngine/0.1.0 (Custom 100% Native Engine)".to_string(),
        );
        headers.insert("Accept".to_string(), "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string());
        headers.insert("Accept-Language".to_string(), "en-US,en;q=0.9,es;q=0.8".to_string());

        Ok(Self {
            url,
            method: Method::Get,
            headers,
            body: None,
        })
    }
}
