# Spacesly Icon Audit

## Standard

Spacesly uses `lucide-svelte` for authored interface icons. Retained icons follow these sizes:

- 16 px for standard actions and close controls.
- 14 px for compact row actions, tree items, status steps, and disclosures.
- Lucide's default stroke width; no mixed filled-icon set.
- A 6-9 px gap when an icon accompanies text.

Icons inside already named controls are hidden from assistive technology. Icon-only controls retain an explicit `aria-label`; `title` is supplementary.

## Removed

| Area                                   | Removed                                                                             | Reason                                                                                                 |
| -------------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Workspace header                       | Static workspace caret                                                              | Suggested a workspace menu that does not exist.                                                        |
| Board                                  | Static lane carets                                                                  | Suggested collapsible lanes without a collapse action.                                                 |
| Board                                  | `+` from New task                                                                   | Repeated the adjacent action label.                                                                    |
| Settings, MCP, Skills, Environment     | `+` from labeled create actions                                                     | Added visual noise without improving recognition.                                                      |
| Task cards and task detail             | Always-green status dots                                                            | Duplicated status text and incorrectly implied success for every state.                                |
| Agent launcher and connection settings | Repetitive status dots                                                              | Replaced by explicit status text so state is not color-only.                                           |
| Agent Console                          | Hero orb and activity pulse                                                         | Repeated the status heading, progress percentage, progress bar, and current-activity text.             |
| Agent Console                          | Open-task, completion, result-summary, result-row, and technical-console decoration | Labels already communicated the action or content; generic repeated icons did not differentiate items. |
| Git actions                            | Icons beside Stage All, Unstage All, Pull, Push, Refresh, Merge                     | Full labels were faster to read and the dense icon row added little value.                             |
| Git file lists                         | Generic file icon on every row                                                      | Every row was already identified as a file by its path and section context.                            |
| Git empty states                       | Generic file and success illustrations                                              | Empty-state headings and descriptions fully communicate the state.                                     |
| Workspace search                       | Search icon inside the labeled search field                                         | Panel title, label, placeholder, and input role already establish search.                              |
| MCP inherited settings                 | Decorative radial highlight                                                         | Surface hierarchy and text already identify the information block.                                     |

## Replaced

| Area                                          | Previous                           | Current                                       | Reason                                                                   |
| --------------------------------------------- | ---------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------ |
| Settings, dialogs, notifications, task detail | Font `×`                           | Lucide `X`, 16 px                             | Consistent stroke, alignment, and control sizing.                        |
| Editor navigation                             | Font arrows                        | Lucide `ArrowLeft` / `ArrowRight`, 16 px      | Recognizable navigation with consistent rendering across platforms.      |
| Editor outline                                | Font triangles and refresh glyph   | Lucide chevrons and `RefreshCw`               | The icons now map directly to disclosure and refresh actions.            |
| Editor tabs                                   | `!`, `•`, and font close glyph     | Lucide warning, CSS dirty dot, and Lucide `X` | Conflict and unsaved states remain scannable without mixed font symbols. |
| Collapsed file sidebar                        | `>`                                | Lucide `PanelLeftOpen`                        | Communicates opening a side panel rather than generic direction.         |
| File browser                                  | `FileSearch` for Open file         | Lucide `File`                                 | Avoids implying content search.                                          |
| File browser                                  | Same collapse icon as Hide sidebar | Lucide `ListCollapse`                         | Directly communicates collapsing the current hierarchy.                  |
| Git row actions                               | 13 px unlabeled icons              | 14 px Lucide icons in 32 px labeled controls  | Improves consistency and accessible naming.                              |

## Kept

- File and folder tree icons because they differentiate item types and expansion state.
- Git branch and disclosure icons because they improve source-control scanning.
- Stage and unstage icons on compact per-file actions because the surrounding row establishes their meaning.
- Agent execution-step icons because completed, running, blocked, and pending steps are easier to scan and are also named in text.
- Warning icons for actionable warning groups.
- Native `<details>/<summary>` markers and native select indicators because they communicate actual disclosure behavior.
- Diff `+` and `-`, path `/`, truncation ellipses, terminal `$`, dirty punctuation in text, and mathematical symbols because they are content notation, not decorative interface icons.
- The Jira `Partial Success` warning symbol because it is exported status communication and remains backed by explicit text.
- CSS resize handles because they are direct-manipulation affordances rather than decorative icons.

## Screen Coverage

- Dashboard and top navigation.
- Task board, task cards, task details, empty lanes, and new-task popover.
- Agent Console, execution plan, results, activity log, and technical console.
- Files, editor tabs, document outline, references, workspace search, and terminal.
- Git branch picker, Git actions, file lists, context menu, and empty states.
- Settings navigation, Agent, MCP, Jira, Skills, Global Environment, and Theme.
- Skill editor, confirmation dialogs, notifications, validation, empty, loading, and error states.

Text-first screens such as Workspace Chat, Terminal, shared Settings sections, and AI Edit Review remain intentionally icon-light.
