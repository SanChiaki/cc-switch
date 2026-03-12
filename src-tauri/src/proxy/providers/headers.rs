use crate::provider::Provider;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::HashMap;

fn to_header_value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    source: &str,
    raw_name: &str,
    raw_value: &str,
) -> Result<(), String> {
    let name = raw_name.trim();
    if name.is_empty() {
        return Err(format!("{source} contains an empty header name"));
    }

    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| format!("{source} contains invalid header name '{name}': {e}"))?;
    let header_value = HeaderValue::from_str(raw_value)
        .map_err(|e| format!("{source} contains invalid value for header '{name}': {e}"))?;

    headers.insert(header_name, header_value);
    Ok(())
}

fn merge_json_headers(headers: &mut HeaderMap, source: &str, value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err(format!("{source} must be a JSON object"));
    };

    for (key, raw_value) in object {
        let Some(value_str) = to_header_value_string(raw_value) else {
            return Err(format!(
                "{source} header '{key}' must be a string, number, or boolean"
            ));
        };
        insert_header(headers, source, key, &value_str)?;
    }

    Ok(())
}

fn merge_string_headers(
    headers: &mut HeaderMap,
    source: &str,
    values: &HashMap<String, String>,
) -> Result<(), String> {
    for (key, value) in values {
        insert_header(headers, source, key, value)?;
    }

    Ok(())
}

pub fn extract_custom_request_headers(provider: &Provider) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();

    if let Some(value) = provider.settings_config.get("headers") {
        merge_json_headers(&mut headers, "settingsConfig.headers", value)?;
    }

    if let Some(value) = provider.settings_config.get("requestHeaders") {
        merge_json_headers(&mut headers, "settingsConfig.requestHeaders", value)?;
    }

    if let Some(meta_headers) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.request_headers.as_ref())
    {
        merge_string_headers(&mut headers, "meta.requestHeaders", meta_headers)?;
    }

    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, ProviderMeta};
    use serde_json::json;
    use std::collections::HashMap;

    fn create_provider(config: serde_json::Value, meta: Option<ProviderMeta>) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test Provider".to_string(),
            settings_config: config,
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn extracts_headers_from_provider_meta() {
        let mut request_headers = HashMap::new();
        request_headers.insert("x-test-token".to_string(), "token".to_string());

        let provider = create_provider(
            json!({}),
            Some(ProviderMeta {
                request_headers: Some(request_headers),
                ..ProviderMeta::default()
            }),
        );

        let headers = extract_custom_request_headers(&provider).unwrap();
        assert_eq!(
            headers.get("x-test-token").unwrap().to_str().unwrap(),
            "token"
        );
    }

    #[test]
    fn meta_headers_override_legacy_settings_headers() {
        let mut request_headers = HashMap::new();
        request_headers.insert("x-test-token".to_string(), "from-meta".to_string());

        let provider = create_provider(
            json!({
                "headers": {
                    "x-test-token": "from-settings"
                },
                "requestHeaders": {
                    "x-other": "value"
                }
            }),
            Some(ProviderMeta {
                request_headers: Some(request_headers),
                ..ProviderMeta::default()
            }),
        );

        let headers = extract_custom_request_headers(&provider).unwrap();
        assert_eq!(
            headers.get("x-test-token").unwrap().to_str().unwrap(),
            "from-meta"
        );
        assert_eq!(headers.get("x-other").unwrap().to_str().unwrap(), "value");
    }

    #[test]
    fn rejects_invalid_header_name() {
        let provider = create_provider(
            json!({
                "requestHeaders": {
                    "bad header": "value"
                }
            }),
            None,
        );

        let error = extract_custom_request_headers(&provider).unwrap_err();
        assert!(error.contains("invalid header name"));
    }
}
