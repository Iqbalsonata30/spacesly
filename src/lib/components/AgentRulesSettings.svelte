<script lang="ts">
  import { tick } from "svelte";
  import { exampleAgentRules, summarizeAgentRules } from "$lib/agentRules";

  type Props = {
    draft: string;
    saved: string;
    defaultRules: string;
    saving: boolean;
    saveError: string | null;
    saveMessage: string | null;
    onDraftChange: (value: string) => void;
    onSave: () => void;
    onRevert: () => void;
  };

  let {
    draft,
    saved,
    defaultRules,
    saving,
    saveError,
    saveMessage,
    onDraftChange,
    onSave,
    onRevert,
  }: Props = $props();

  let mode = $state<"edit" | "preview">("edit");
  let moreOpen = $state(false);
  let expanded = $state(false);
  let currentLine = $state(1);
  let textarea: HTMLTextAreaElement | null = $state(null);
  let lineGutter: HTMLDivElement | null = $state(null);
  let actionMessage = $state<string | null>(null);
  let summary = $derived(summarizeAgentRules(draft));
  let dirty = $derived(draft !== saved);
  let lineNumbers = $derived(
    Array.from({ length: Math.max(1, summary.lineCount) }, (_, i) => i + 1),
  );

  function updateCurrentLine() {
    if (!textarea) return;
    currentLine = textarea.value.slice(0, textarea.selectionStart).split("\n").length;
  }

  function updateDraft(value: string) {
    actionMessage = null;
    onDraftChange(value);
  }

  async function handleEditorKeydown(event: KeyboardEvent) {
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (dirty && !saving) onSave();
      return;
    }
    if (event.key !== "Tab" || !textarea) return;
    event.preventDefault();
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    textarea.setRangeText("  ", start, end, "end");
    updateDraft(textarea.value);
    await tick();
    textarea?.setSelectionRange(start + 2, start + 2);
    updateCurrentLine();
  }

  function handleMoreActionsKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && moreOpen) {
      event.preventDefault();
      event.stopPropagation();
      moreOpen = false;
    }
  }

  function replaceDraft(next: string, message: string, confirmMessage?: string) {
    if (confirmMessage && !window.confirm(confirmMessage)) return;
    updateDraft(next);
    mode = "edit";
    moreOpen = false;
    actionMessage = message;
    void tick().then(() => textarea?.focus());
  }

  function revertDraft() {
    onRevert();
    moreOpen = false;
    actionMessage = "Unsaved changes reverted";
  }

  async function copyRules() {
    try {
      await navigator.clipboard.writeText(draft);
      actionMessage = "Rules copied";
    } catch (reason) {
      actionMessage = `Could not copy rules: ${reason instanceof Error ? reason.message : String(reason)}`;
    }
    moreOpen = false;
  }
</script>

<section class:expanded class="agent-rules-workspace" aria-label="Agent Rules editor">
  <section class="rule-behavior" aria-labelledby="rule-behavior-title">
    <div class="rules-section-heading">
      <div>
        <h4 id="rule-behavior-title">Rule behavior</h4>
        <p>Rules define global constraints for every Agent action.</p>
      </div>
      <details>
        <summary>Learn how rules are applied</summary>
        <p>
          Rules are included before task-specific instructions and evaluated from top to bottom.
        </p>
      </details>
    </div>
    <dl>
      <div>
        <dt>Priority</dt>
        <dd>Before task instructions</dd>
      </div>
      <div>
        <dt>Scope</dt>
        <dd>All Agent actions</dd>
      </div>
      <div>
        <dt>Format</dt>
        <dd>One rule per line</dd>
      </div>
    </dl>
  </section>

  <section class="rules-editor-section" aria-labelledby="rules-editor-title">
    <div class="rules-editor-heading">
      <div>
        <h4 id="rules-editor-title">Rules applied to every run</h4>
        <p>
          Write one direct and enforceable rule per line. Rules are evaluated from top to bottom.
        </p>
      </div>
      <div class="rules-view-switch" role="tablist" aria-label="Rules editor view">
        <button
          type="button"
          role="tab"
          aria-selected={mode === "edit"}
          class:active={mode === "edit"}
          onclick={() => (mode = "edit")}>Edit</button
        >
        <button
          type="button"
          role="tab"
          aria-selected={mode === "preview"}
          class:active={mode === "preview"}
          onclick={() => (mode = "preview")}>Preview</button
        >
      </div>
    </div>

    <div class="rules-editor-surface">
      <div class="rules-editor-toolbar">
        <div>
          <span>{summary.rules.length} rule{summary.rules.length === 1 ? "" : "s"}</span>
          <span>{summary.lineCount} line{summary.lineCount === 1 ? "" : "s"}</span>
          <span>{summary.characterCount} characters</span>
        </div>
        <div>
          {#if !draft.trim()}
            <button
              type="button"
              onclick={() => replaceDraft(exampleAgentRules, "Example rules inserted")}
              >Insert example rules</button
            >
          {/if}
          <button type="button" onclick={() => (expanded = !expanded)}
            >{expanded ? "Restore size" : "Expand editor"}</button
          >
          <div class="rules-more-actions" role="group" aria-label="Rules actions">
            <button
              type="button"
              aria-haspopup="menu"
              aria-expanded={moreOpen}
              onkeydown={handleMoreActionsKeydown}
              onclick={() => (moreOpen = !moreOpen)}>More actions</button
            >
            {#if moreOpen}
              <div
                role="menu"
                tabindex="-1"
                onkeydown={handleMoreActionsKeydown}
                onfocusout={(event) => {
                  if (!event.currentTarget.contains(event.relatedTarget as Node | null))
                    moreOpen = false;
                }}
              >
                <button type="button" role="menuitem" disabled={!dirty} onclick={revertDraft}
                  >Revert unsaved changes</button
                >
                <button type="button" role="menuitem" onclick={() => void copyRules()}
                  >Copy all rules</button
                >
                <button
                  type="button"
                  role="menuitem"
                  onclick={() =>
                    replaceDraft(
                      defaultRules,
                      "Default rules restored",
                      "Reset Agent Rules? This will replace the current Rules with the application defaults.",
                    )}>Reset to default</button
                >
                <button
                  class="danger"
                  type="button"
                  role="menuitem"
                  disabled={!draft}
                  onclick={() =>
                    replaceDraft(
                      "",
                      "Rules cleared",
                      "Clear all Agent Rules? This cannot be undone.",
                    )}>Clear rules</button
                >
              </div>
            {/if}
          </div>
        </div>
      </div>

      {#if mode === "edit"}
        <div class="rules-textarea-shell">
          <div class="rules-line-numbers" bind:this={lineGutter} aria-hidden="true">
            {#each lineNumbers as line (line)}<span class:current={line === currentLine}
                >{line}</span
              >{/each}
          </div>
          <textarea
            id="agent-rules-input"
            bind:this={textarea}
            value={draft}
            spellcheck="false"
            aria-labelledby="rules-editor-title"
            aria-describedby="agent-rules-help agent-rules-validation"
            placeholder={`# Example rules\n\n${exampleAgentRules}`}
            oninput={(event) => {
              updateDraft(event.currentTarget.value);
              updateCurrentLine();
            }}
            onclick={updateCurrentLine}
            onkeyup={updateCurrentLine}
            onscroll={(event) => {
              if (lineGutter) lineGutter.scrollTop = event.currentTarget.scrollTop;
            }}
            onkeydown={handleEditorKeydown}></textarea>
        </div>
      {:else}
        <div class="rules-preview" role="tabpanel" aria-label="Rules preview">
          {#if summary.rules.length > 0}
            <ol>
              {#each summary.rules as rule, i (i)}<li>{rule}</li>{/each}
            </ol>
          {:else}
            <div class="rules-empty">
              <strong>No rules have been added yet.</strong>
              <p>Add direct constraints manually or insert the example rules.</p>
              <button
                type="button"
                onclick={() => replaceDraft(exampleAgentRules, "Example rules inserted")}
                >Insert example rules</button
              >
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="rules-guidance" id="agent-rules-help">
      <p>
        Write one direct and enforceable rule per line. Start with verbs such as verify, require,
        avoid, block, or ask.
      </p>
      <details>
        <summary>Examples</summary>
        <div>
          <strong>Good</strong>
          <ul>
            <li>Verify the active branch before committing changes.</li>
            <li>Ask before deleting files.</li>
            <li>Avoid modifying generated files.</li>
          </ul>
          <strong>Avoid</strong>
          <ul>
            <li>Be careful.</li>
            <li>Make the project better.</li>
            <li>Try to minimize problems.</li>
          </ul>
        </div>
      </details>
    </div>

    <div class="rules-validation" id="agent-rules-validation" aria-live="polite">
      {#each summary.notices as notice (notice.message)}
        <p class={notice.level}>
          <strong>{notice.level === "warning" ? "Warning" : "Suggestion"}</strong>{notice.message}
        </p>
      {/each}
    </div>
  </section>

  <footer class="rules-action-footer">
    <div aria-live="polite">
      {#if saveError}<strong class="error">Couldn’t save Agent Rules</strong><span>{saveError}</span
        >
      {:else if dirty}<strong>Unsaved changes</strong><span
          >Save to apply these Rules after restart.</span
        >
      {:else if saveMessage}<strong class="success">{saveMessage}</strong>
      {:else if actionMessage}<strong>{actionMessage}</strong>
      {:else}<span>All changes saved</span>{/if}
    </div>
    <div>
      <button type="button" disabled={!dirty || saving} onclick={revertDraft}>Revert</button>
      <button class="save-rules" type="button" disabled={!dirty || saving} onclick={onSave}
        >{saving ? "Saving…" : saveError ? "Try again" : "Save changes"}</button
      >
    </div>
  </footer>
</section>

<style>
  .agent-rules-workspace {
    display: grid;
    gap: 24px;
    min-width: 0;
  }

  .rule-behavior {
    display: grid;
    gap: 14px;
    border-block: 1px solid var(--border-subtle);
    padding: 16px 0;
  }

  .rules-section-heading,
  .rules-editor-heading,
  .rules-editor-toolbar,
  .rules-action-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  h4,
  p,
  dl,
  dt,
  dd,
  ol,
  ul {
    margin: 0;
  }

  h4 {
    color: var(--text-bright);
    font-size: 14px;
  }

  .rules-section-heading p,
  .rules-editor-heading p,
  .rules-guidance > p {
    margin-top: 4px;
    color: var(--text-secondary);
    font-size: 12px;
    line-height: 1.5;
  }

  .rules-section-heading details {
    flex: 0 0 auto;
    max-width: 330px;
    color: var(--text-secondary);
    font-size: 11px;
  }

  .rules-section-heading summary,
  .rules-guidance summary {
    color: var(--text-link);
    cursor: pointer;
    font-weight: 800;
  }

  .rules-section-heading details p {
    margin-top: 8px;
    line-height: 1.45;
  }

  dl {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
  }

  dl div {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 10px;
    align-items: baseline;
    min-width: 0;
  }

  dt {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 850;
  }

  dd {
    overflow: hidden;
    color: var(--text-primary);
    font-size: 12px;
    font-weight: 750;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rules-editor-section {
    display: grid;
    gap: 12px;
    min-width: 0;
  }

  .rules-editor-heading {
    align-items: flex-end;
  }

  .rules-view-switch {
    display: inline-flex;
    flex: 0 0 auto;
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    padding: 3px;
    background: var(--surface-inset);
  }

  .rules-view-switch button {
    min-height: 30px;
    border: 0;
    border-radius: 7px;
    padding: 0 12px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 850;
  }

  .rules-view-switch button.active {
    background: var(--surface-selected);
    color: var(--text-bright);
    box-shadow: 0 1px 3px color-mix(in srgb, var(--text-bright) 10%, transparent);
  }

  .rules-editor-surface {
    min-width: 0;
    overflow: visible;
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    background: var(--surface-inset);
    box-shadow: inset 0 1px 0 color-mix(in srgb, var(--text-bright) 5%, transparent);
  }

  .rules-editor-surface:focus-within {
    border-color: var(--focus-border);
    box-shadow: 0 0 0 3px var(--focus-ring);
  }

  .rules-editor-toolbar {
    min-height: 42px;
    border-bottom: 1px solid var(--border-subtle);
    padding: 0 10px 0 14px;
    background: var(--surface-raised);
  }

  .rules-editor-toolbar > div {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .rules-editor-toolbar span {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 750;
  }

  .rules-editor-toolbar button,
  .rules-empty button,
  .rules-action-footer button {
    min-height: 32px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    padding: 0 10px;
    background: var(--surface-raised);
    color: var(--text-primary);
    font: inherit;
    font-size: 11px;
    font-weight: 800;
  }

  .rules-more-actions {
    position: relative;
  }

  .rules-more-actions > div {
    position: absolute;
    z-index: 4;
    top: calc(100% + 7px);
    right: 0;
    display: grid;
    width: 210px;
    overflow: hidden;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--menu-bg);
    box-shadow: var(--shadow-popover);
  }

  .rules-more-actions > div button {
    min-height: 38px;
    border: 0;
    border-bottom: 1px solid var(--border-subtle);
    border-radius: 0;
    text-align: left;
  }

  .rules-more-actions > div button:last-child {
    border-bottom: 0;
  }

  .rules-more-actions > div button.danger {
    color: var(--danger);
  }

  .rules-textarea-shell {
    display: grid;
    grid-template-columns: 48px minmax(0, 1fr);
    min-height: clamp(340px, 44vh, 520px);
    overflow: hidden;
    border-radius: 0 0 13px 13px;
    background: var(--code-block-bg);
  }

  .expanded .rules-textarea-shell,
  .expanded .rules-preview {
    min-height: min(64vh, 680px);
  }

  .rules-line-numbers {
    display: grid;
    align-content: start;
    overflow: hidden;
    border-right: 1px solid var(--border-subtle);
    padding: 16px 0 24px;
    color: var(--text-muted);
    background: color-mix(in srgb, var(--surface-raised) 72%, transparent);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 22px;
    text-align: right;
  }

  .rules-line-numbers span {
    padding-right: 12px;
  }

  .rules-line-numbers span.current {
    color: var(--text-primary);
    background: var(--surface-hover);
  }

  textarea {
    box-sizing: border-box;
    width: 100%;
    min-height: inherit;
    resize: vertical;
    border: 0;
    border-radius: 0;
    padding: 16px 18px 24px;
    background: transparent;
    color: var(--code-text);
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 22px;
    outline: none;
    tab-size: 2;
    white-space: pre;
  }

  textarea::placeholder {
    color: var(--text-muted);
  }

  .rules-preview {
    min-height: clamp(340px, 44vh, 520px);
    overflow: auto;
    padding: 20px 24px;
    background: var(--code-block-bg);
  }

  .rules-preview ol {
    display: grid;
    gap: 8px;
    padding-left: 28px;
  }

  .rules-preview li {
    border-bottom: 1px solid var(--border-subtle);
    padding: 0 4px 9px;
    color: var(--text-primary);
    line-height: 1.5;
  }

  .rules-preview li::marker {
    color: var(--text-dim);
    font-family: var(--font-mono);
  }

  .rules-empty {
    display: grid;
    max-width: 420px;
    min-height: 260px;
    place-content: center;
    justify-items: start;
    gap: 8px;
    margin: auto;
    color: var(--text-secondary);
  }

  .rules-empty strong {
    color: var(--text-bright);
  }

  .rules-guidance {
    color: var(--text-secondary);
  }

  .rules-guidance details {
    margin-top: 8px;
    font-size: 11px;
  }

  .rules-guidance details > div {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 7px 12px;
    margin-top: 10px;
    border-left: 2px solid var(--border-subtle);
    padding-left: 12px;
  }

  .rules-guidance ul {
    padding-left: 17px;
  }

  .rules-validation {
    display: grid;
    gap: 6px;
  }

  .rules-validation:empty {
    display: none;
  }

  .rules-validation p {
    display: flex;
    gap: 8px;
    border-left: 3px solid var(--info-border);
    padding: 7px 10px;
    background: var(--info-bg);
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1.4;
  }

  .rules-validation p.warning {
    border-color: var(--warning-border);
    background: var(--warning-bg);
  }

  .rules-validation p strong {
    color: var(--text-primary);
    text-transform: capitalize;
  }

  .rules-action-footer {
    position: sticky;
    z-index: 3;
    bottom: -20px;
    margin: 4px -20px -20px;
    border-top: 1px solid var(--border-subtle);
    padding: 12px 20px;
    background: var(--dialog-bg);
    box-shadow: 0 -10px 26px color-mix(in srgb, var(--surface) 82%, transparent);
    backdrop-filter: blur(18px) saturate(120%);
  }

  .rules-action-footer > div {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .rules-action-footer > div:first-child {
    min-width: 0;
    flex-wrap: wrap;
    color: var(--text-secondary);
    font-size: 11px;
  }

  .rules-action-footer strong {
    color: var(--text-bright);
  }

  .rules-action-footer strong.error {
    color: var(--danger);
  }

  .rules-action-footer strong.success {
    color: var(--success);
  }

  .rules-action-footer .save-rules {
    border-color: var(--selection-border);
    background: var(--accent);
    color: var(--surface-overlay);
  }

  button:focus-visible,
  summary:focus-visible,
  textarea:focus-visible {
    outline: 2px solid var(--focus-border);
    outline-offset: 2px;
  }

  button:disabled {
    cursor: not-allowed;
    opacity: var(--disabled-opacity);
  }

  @media (max-width: 760px) {
    .rules-section-heading,
    .rules-editor-heading,
    .rules-editor-toolbar,
    .rules-action-footer {
      align-items: stretch;
      flex-direction: column;
    }

    .rules-section-heading details {
      max-width: none;
    }

    dl {
      grid-template-columns: 1fr;
    }

    dl div {
      grid-template-columns: 72px minmax(0, 1fr);
    }

    .rules-view-switch {
      align-self: flex-start;
    }

    .rules-editor-toolbar > div {
      flex-wrap: wrap;
    }

    .rules-action-footer > div:last-child,
    .rules-action-footer button {
      width: 100%;
    }

    .rules-textarea-shell {
      grid-template-columns: 38px minmax(0, 1fr);
      min-height: 320px;
    }
  }
</style>
