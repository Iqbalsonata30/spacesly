# Spacesly

Spacesly is an AI-native desktop workspace for turning external work items into observable AI workflows.

## Jira MCP Connector

Spacesly can fetch Jira boards and tickets directly through Jira REST using your Jira credentials. MCP remains available for tool/agent integration, but board sync does not require MCP to be healthy.
Jira REST calls use platform TLS and system proxy detection so corporate/internal Jira certificates and proxy settings are handled closer to browser/OS behavior. Calls have a 15 second timeout, and the last synced board is cached locally so the app can open instantly without waiting for Jira.

Open Settings from the toolbar, then configure reusable MCP connections first. Jira, OCP, Bamboo, and future integrations should select from the same MCP Connections list instead of owning separate server setup forms.

MCP Connection fields:

- `Name`: display name for the connector.
- `Connection Type`: generic, Jira, OpenShift/OCP, or Bamboo. This is metadata for organizing integrations.
- `Command`: executable or full command line used to start the MCP server, for example `uvx mcp-atlassian`, `mcp-atlassian`, or `/full/path/to/uvx mcp-atlassian`.
- `Arguments`: additional JSON array arguments such as `["--transport", "stdio"]`, or whitespace-separated arguments.
- `Connection Configuration`: per-MCP server environment/config values. Use one `KEY=value` per line or paste a JSON object. This is where Jira, OCP/Kubernetes, Bamboo, or any future MCP-specific credentials/settings belong.

Jira Board Sync fields:

- `MCP Connection For Jira`: which reusable MCP connection belongs to Jira.
- `Jira URL`: Jira site URL, for example `https://company.atlassian.net`.
- `Authentication Method`: choose `Email + API token`, `Personal access token`, or `Username + password`.
- `Email / Username`: Jira account email or username when required by the selected method.
- Credential field: API token, PAT, or password based on the selected method.
- `Workspace / Board Name`: label used for the synced Spacesly board.
- `Project Key Filter`: optional Jira project key used when loading boards.
- `Board Name Filter`: optional board name search used when loading boards.
- `Manual Jira Board ID`: use this if board loading is slow/fails but you already know the Jira board ID.
- `MCP Tool Name`: Jira search tool exposed by the server, for example `jira_search`.
- `Board List Tool`: MCP tool used to list Jira boards, usually `jira_get_agile_boards`.
- `Board Issues Tool`: MCP tool used to fetch issues from a Jira board, usually `jira_get_board_issues`.
- `JQL`: query used by `Sync Jira`.

Sync flow:

1. Fill Jira URL and the selected credential method.
2. Optionally fill MCP command for tool/agent integration.
3. Click `Connect Jira` to load available Jira kanban/scrum boards.
3. Select a Jira board.
4. Click `Sync Jira board` in the toolbar.

Sync is incremental by default: it fetches 25 cards from 1 Jira page. Increase `Cards Per Sync Page` or `Max Pages Per Sync` in Settings if you want more cards, but smaller values keep the app responsive on slow Jira boards.

If no Jira board is selected, Spacesly falls back to the configured JQL search. Settings are stored locally in the desktop app for now.

`MCP connected with 49 tools, but Jira returned no boards or tickets yet` means the MCP server is reachable, but Jira data calls returned no data for the current settings. Spacesly now falls back to Jira Agile REST for board and board issue fetching using the Jira URL and credentials from Settings. Add `Project Key Filter`, `Board Name Filter`, or a manual board ID, then try `Connect Jira` or `Sync Jira board` again. Expand `Available MCP tools` after testing to confirm the tool names match your MCP server. Spacesly auto-detects prefixed tool names, so `mcp-atlassian_jira_get_agile_boards` and `jira_get_agile_boards` both work.

If `uvx` or `mcp-atlassian` returns `No such file or directory`, the desktop process cannot find that executable. Spacesly loads your login shell environment before spawning MCP servers, but if your launcher still misses it, use a full path from `which uvx`.

MCP connection tests time out after 45 seconds and return captured MCP stderr when available, so startup/configuration failures should surface in Settings instead of hanging indefinitely.

## Agent Runtime

Spacesly can run a card through an Agent when you click `Start` or drag the card to `Ready` / `In progress`. Users choose from supported providers and models; Spacesly fills the correct endpoint automatically.

Agent fields:

- `Provider`: supported provider such as OpenAI, Gemini, DeepSeek, or Claude.
- `Model`: supported model for the selected provider.
- Provider API key: credential required by the selected provider.
- `Temperature`: model temperature for task execution.

Click `Test Agent` in Settings to verify the current model. `New task` creates a local Spacesly card that can be executed by the Agent without Jira. When an Agent completes a Jira-backed card, Spacesly stores the execution summary on the card, posts the Agent summary as a Jira comment, moves the card to Done, and transitions the Jira issue to Done.

## Development

```bash
bun run check
rtk cargo test
```
