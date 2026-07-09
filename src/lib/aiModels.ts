export interface AiModelOption {
  id: string;
  label: string;
  description: string;
}

export interface AiProviderOption {
  id: string;
  label: string;
  baseUrl: string;
  apiStyle: "openai_chat" | "openai_responses" | "anthropic_messages";
  apiKeyLabel: string;
  apiKeyPlaceholder: string;
  models: AiModelOption[];
}

export const aiProviders: AiProviderOption[] = [
  {
    id: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    apiStyle: "openai_responses",
    apiKeyLabel: "OpenAI API Key",
    apiKeyPlaceholder: "sk-...",
    models: [
      { id: "gpt-5.5", label: "GPT-5.5", description: "Current flagship when API access is available" },
      { id: "gpt-5.1", label: "GPT-5.1", description: "Strong general agent" },
      { id: "gpt-4.1-mini", label: "GPT-4.1 Mini", description: "Fast daily worker" },
    ],
  },
  {
    id: "gemini",
    label: "Gemini",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    apiStyle: "openai_chat",
    apiKeyLabel: "Gemini API Key",
    apiKeyPlaceholder: "AIza...",
    models: [
      { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro", description: "Strong reasoning agent" },
      { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash", description: "Fast daily agent" },
    ],
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    apiStyle: "openai_chat",
    apiKeyLabel: "DeepSeek API Key",
    apiKeyPlaceholder: "sk-...",
    models: [
      { id: "deepseek-v4-flash", label: "DeepSeek V4 Flash Free", description: "Free fast agent option when available" },
      { id: "deepseek-chat", label: "DeepSeek Chat", description: "General DeepSeek agent" },
    ],
  },
  {
    id: "claude",
    label: "Claude",
    baseUrl: "https://api.anthropic.com/v1",
    apiStyle: "anthropic_messages",
    apiKeyLabel: "Anthropic API Key",
    apiKeyPlaceholder: "sk-ant-...",
    models: [
      { id: "claude-sonnet-4-5", label: "Claude Sonnet 4.5", description: "Strong coding and operations agent" },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", description: "Fast lightweight agent" },
    ],
  },
];

export function providerById(providerId: string): AiProviderOption {
  return aiProviders.find((provider) => provider.id === providerId) ?? aiProviders[0];
}

export function modelById(provider: AiProviderOption, modelId: string): AiModelOption {
  return provider.models.find((model) => model.id === modelId) ?? provider.models[0];
}

export function defaultModelForProvider(providerId: string): string {
  return providerById(providerId).models[0].id;
}
