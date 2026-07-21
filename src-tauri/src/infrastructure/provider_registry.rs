#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiStyle {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
}

impl ApiStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai_chat",
            Self::OpenAiResponses => "openai_responses",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "openai_chat" => Some(Self::OpenAiChat),
            "openai_responses" => Some(Self::OpenAiResponses),
            "anthropic_messages" => Some(Self::AnthropicMessages),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_style: ApiStyle,
    pub models: &'static [&'static str],
}

const OPENAI_MODELS: &[&str] = &["gpt-5.5", "gpt-5.1", "gpt-4.1-mini"];
const GEMINI_MODELS: &[&str] = &["gemini-2.5-pro", "gemini-2.5-flash"];
const DEEPSEEK_MODELS: &[&str] = &["deepseek-v4-flash", "deepseek-chat"];
const CLAUDE_MODELS: &[&str] = &["claude-sonnet-4-5", "claude-haiku-4-5"];

pub fn profile(id: &str) -> Option<ProviderProfile> {
    match id.trim() {
        "openai" => Some(ProviderProfile {
            id: "openai",
            name: "OpenAI",
            base_url: "https://api.openai.com/v1",
            api_style: ApiStyle::OpenAiResponses,
            models: OPENAI_MODELS,
        }),
        "gemini" => Some(ProviderProfile {
            id: "gemini",
            name: "Gemini",
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
            api_style: ApiStyle::OpenAiChat,
            models: GEMINI_MODELS,
        }),
        "deepseek" => Some(ProviderProfile {
            id: "deepseek",
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            api_style: ApiStyle::OpenAiChat,
            models: DEEPSEEK_MODELS,
        }),
        "claude" => Some(ProviderProfile {
            id: "claude",
            name: "Claude",
            base_url: "https://api.anthropic.com/v1",
            api_style: ApiStyle::AnthropicMessages,
            models: CLAUDE_MODELS,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_supported_provider_profiles() {
        let openai = profile("openai").unwrap();
        assert_eq!(openai.name, "OpenAI");
        assert_eq!(openai.api_style, ApiStyle::OpenAiResponses);
        assert!(openai.models.contains(&"gpt-5.5"));
        assert_eq!(profile("missing"), None);
    }

    #[test]
    fn parses_only_registered_api_styles() {
        assert_eq!(ApiStyle::parse("openai_chat"), Some(ApiStyle::OpenAiChat));
        assert_eq!(ApiStyle::parse("unknown"), None);
    }
}
