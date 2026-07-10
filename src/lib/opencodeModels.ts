export type OpencodeModelOption = {
  id: string;
  label: string;
  provider: string;
  badge: "GPT" | "Free" | "Fast" | "Reasoning";
  description: string;
};

export const opencodeModelOptions: OpencodeModelOption[] = [
  {
    id: "openai/gpt-5.5",
    label: "GPT-5.5",
    provider: "OpenAI",
    badge: "GPT",
    description: "Best default when your OpenCode auth has OpenAI access.",
  },
  {
    id: "openai/gpt-5",
    label: "GPT-5",
    provider: "OpenAI",
    badge: "GPT",
    description: "Strong coding and reasoning model for production changes.",
  },
  {
    id: "openai/gpt-4.1",
    label: "GPT-4.1",
    provider: "OpenAI",
    badge: "GPT",
    description: "Stable GPT option for everyday coding tasks.",
  },
  {
    id: "google/gemini-2.5-flash",
    label: "Gemini 2.5 Flash",
    provider: "Google",
    badge: "Free",
    description: "Fast, low-cost choice that is often available on free Gemini quota.",
  },
  {
    id: "deepseek/deepseek-chat",
    label: "DeepSeek Chat",
    provider: "DeepSeek",
    badge: "Free",
    description: "Good budget coding model when DeepSeek auth is configured.",
  },
  {
    id: "openrouter/qwen/qwen3-coder:free",
    label: "Qwen3 Coder Free",
    provider: "OpenRouter",
    badge: "Free",
    description: "Free OpenRouter coding option when OpenRouter is configured in OpenCode.",
  },
  {
    id: "openrouter/deepseek/deepseek-chat-v3-0324:free",
    label: "DeepSeek V3 Free",
    provider: "OpenRouter",
    badge: "Free",
    description: "Free OpenRouter general coding model for lighter tasks.",
  },
  {
    id: "anthropic/claude-sonnet-4-20250514",
    label: "Claude Sonnet 4",
    provider: "Anthropic",
    badge: "Reasoning",
    description: "Strong implementation model if Anthropic is configured.",
  },
];

export function normalizeOpencodeModel(value: unknown, fallback = "openai/gpt-5.5"): string {
  const id = typeof value === "string" ? value : fallback;
  return opencodeModelOptions.some((option) => option.id === id) ? id : fallback;
}
