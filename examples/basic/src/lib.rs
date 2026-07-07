use zero_ai::types::Model;

pub fn devpilot_model() -> Model {
    let api_key = std::env::var("DEVPILOT_API_KEY").unwrap_or_else(|_| "dummy".to_string());
    Model {
        id: "glm-5-1".to_string(),
        provider: zero_ai::types::Provider::Anthropic,
        api_key,
        base_url: Some(
            "http://devpilot.zhonganonline.com/devpilot/v1/external/direct/cline/v1/messages"
                .to_string(),
        ),
        max_tokens: 2048,
    }
}
