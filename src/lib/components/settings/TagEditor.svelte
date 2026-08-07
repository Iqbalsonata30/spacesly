<script lang="ts">
  import { tick } from "svelte";
  import { Plus, X } from "lucide-svelte";

  let {
    id,
    value = [],
    onChange,
    placeholder = "Type and press Enter",
    lowercase = false,
    allowCommaSplit = true,
    maxTags = 50,
    variant = "chips",
  }: {
    id?: string;
    value: string[];
    onChange: (tags: string[]) => void;
    placeholder?: string;
    lowercase?: boolean;
    allowCommaSplit?: boolean;
    maxTags?: number;
    /** "chips" wraps tags inline; "list" renders each tag on its own monospace row (e.g. CLI arguments). */
    variant?: "chips" | "list";
  } = $props();

  let draft = $state("");
  let focused = $state(false);
  let adding = $state(false);
  let addInput = $state<HTMLInputElement>();

  function normalize(raw: string): string {
    let tag = raw.trim();
    if (lowercase) tag = tag.toLowerCase();
    return tag;
  }

  function commit() {
    const entries = draft
      .split(allowCommaSplit ? /[\n,]/ : "\n")
      .map(normalize)
      .filter(Boolean);
    if (entries.length === 0) {
      draft = "";
      return;
    }
    const next = [...value];
    for (const entry of entries) {
      if (entry && !next.includes(entry)) next.push(entry);
    }
    const limited = next.slice(0, maxTags);
    if (limited.length !== value.length) onChange(limited);
    draft = "";
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault();
      commit();
    } else if (event.key === "Backspace" && !draft && value.length) {
      onChange(value.slice(0, -1));
    }
  }

  function removeAt(index: number, restoreFocus = false) {
    onChange(value.filter((_, i) => i !== index));
    if (variant === "list" && restoreFocus) {
      void tick().then(() => focusArgument(Math.min(index, value.length - 2)));
    }
  }

  function updateAt(index: number, nextValue: string) {
    const next = [...value];
    next[index] = nextValue;
    onChange(next);
  }

  function focusArgument(index: number) {
    if (index < 0) {
      startAdding();
      return;
    }
    document.getElementById(`${id ?? "argument"}-row-${index}`)?.focus();
  }

  function startAdding() {
    adding = true;
    void tick().then(() => addInput?.focus());
  }

  function commitArgumentDraft(keepOpen = false) {
    if (draft.length > 0) onChange([...value, draft]);
    draft = "";
    adding = keepOpen;
    if (keepOpen) void tick().then(() => addInput?.focus());
  }

  function onArgumentKeydown(event: KeyboardEvent, index: number) {
    const input = event.currentTarget as HTMLInputElement;
    if (event.key === "Enter") {
      event.preventDefault();
      if (index < value.length - 1) focusArgument(index + 1);
      else startAdding();
    } else if (event.key === "Backspace" && input.value.length === 0) {
      event.preventDefault();
      removeAt(index);
      void tick().then(() => focusArgument(index - 1));
    }
  }
</script>

{#if variant === "list"}
  <div class="argument-editor" class:focused>
    <div class="argument-rows">
      {#each value as argument, index (index)}
        <div class="argument-row">
          <input
            id={`${id ?? "argument"}-row-${index}`}
            class="argument-input"
            type="text"
            value={argument}
            aria-label={`Argument ${index + 1}`}
            spellcheck="false"
            oninput={(event) => updateAt(index, event.currentTarget.value)}
            onkeydown={(event) => onArgumentKeydown(event, index)}
            onfocus={() => (focused = true)}
            onblur={() => (focused = false)}
          />
          <button
            type="button"
            class="argument-remove"
            aria-label={`Remove argument ${index + 1}: ${argument}`}
            title="Remove argument"
            onclick={() => removeAt(index, true)}
          >
            <X size={13} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      {/each}
      {#if adding}
        <div class="argument-row argument-row-new">
          <input
            {id}
            bind:this={addInput}
            class="argument-input"
            type="text"
            value={draft}
            {placeholder}
            aria-label={placeholder}
            spellcheck="false"
            oninput={(event) => (draft = event.currentTarget.value)}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                commitArgumentDraft(true);
              } else if (event.key === "Escape") {
                event.preventDefault();
                draft = "";
                adding = false;
              }
            }}
            onfocus={() => (focused = true)}
            onblur={() => {
              focused = false;
              commitArgumentDraft();
            }}
          />
        </div>
      {:else}
        <button {id} type="button" class="add-argument" onclick={startAdding}>
          <Plus size={13} strokeWidth={2} aria-hidden="true" />
          Add argument
        </button>
      {/if}
    </div>
  </div>
{:else}
  <div class="tag-editor" class:focused class:has-tags={value.length > 0} data-variant={variant}>
    <div class="tag-editor-chips">
      {#each value as tag, i (tag)}
        <span class="chip">
          <span class="chip-label">{tag}</span>
          <button
            type="button"
            class="chip-remove"
            aria-label={`Remove ${tag}`}
            title={`Remove ${tag}`}
            onclick={() => removeAt(i)}
          >
            <X size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        </span>
      {/each}
      <span class="tag-editor-add">
        <input
          {id}
          type="text"
          value={draft}
          placeholder={value.length ? "Add more…" : placeholder}
          aria-label={placeholder}
          oninput={(e) => {
            draft = e.currentTarget.value;
            if ((allowCommaSplit && draft.includes(",")) || draft.includes("\n")) commit();
          }}
          onkeydown={onKeydown}
          onfocus={() => (focused = true)}
          onblur={() => {
            focused = false;
            commit();
          }}
        />
      </span>
    </div>
  </div>
{/if}

<style>
  .tag-editor {
    display: grid;
    gap: 6px;
    min-width: 0;
    cursor: text;
  }

  .tag-editor-chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-height: 42px;
    padding: 6px 8px;
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    background: var(--surface-inset);
    box-shadow: inset 0 1px 0 var(--border-subtle);
  }

  .tag-editor.focused .tag-editor-chips {
    border-color: var(--focus-border);
    box-shadow:
      0 0 0 3px var(--focus-ring),
      inset 0 1px 0 var(--border-subtle);
    outline: none;
  }

  .tag-editor-add {
    display: flex;
    align-items: center;
    flex: 1 1 96px;
    min-width: 96px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    max-width: 100%;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    padding: 3px 5px 3px 9px;
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: 12px;
    line-height: 1.3;
    white-space: nowrap;
  }

  .chip-label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .chip-remove {
    display: inline-flex;
    flex: 0 0 auto;
    align-items: center;
    justify-content: center;
    width: 17px;
    height: 17px;
    border: 0;
    border-radius: 50%;
    padding: 0;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
    transition:
      background 0.12s,
      color 0.12s;
  }

  .chip-remove:hover {
    background: var(--danger-bg);
    color: var(--danger);
  }

  .tag-editor-chips input {
    flex: 1 1 auto;
    min-width: 0;
    width: auto;
    border: 0;
    padding: 5px 2px;
    background: transparent;
    box-shadow: none;
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
  }

  .tag-editor-chips input:focus {
    border: 0;
    box-shadow: none;
    outline: none;
  }

  .tag-editor-chips input::placeholder {
    color: var(--text-muted);
  }

  .argument-editor {
    min-width: 0;
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    background: var(--surface-inset);
    box-shadow: inset 0 1px 0 var(--border-subtle);
  }

  .argument-editor:focus-within {
    border-color: var(--focus-border);
    box-shadow:
      0 0 0 3px var(--focus-ring),
      inset 0 1px 0 var(--border-subtle);
  }

  .argument-rows {
    max-height: 190px;
    overflow-x: hidden;
    overflow-y: auto;
  }

  .argument-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 30px;
    align-items: center;
    min-width: 0;
    min-height: 34px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .argument-row:focus-within {
    background: var(--surface-hover);
  }

  .argument-input {
    width: 100%;
    min-width: 0;
    height: 33px !important;
    border: 0 !important;
    border-radius: 0 !important;
    padding: 5px 4px 5px 11px !important;
    background: transparent !important;
    box-shadow: none !important;
    color: var(--text-primary);
    font-family: var(--font-mono) !important;
    font-size: 12.5px !important;
    text-overflow: ellipsis;
  }

  .argument-input:focus {
    outline: none !important;
    text-overflow: clip;
  }

  .argument-remove {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    padding: 0;
    background: transparent;
    color: var(--text-muted);
  }

  .argument-remove:hover,
  .argument-remove:focus-visible {
    background: var(--danger-bg);
    color: var(--danger);
  }

  .argument-row-new {
    grid-template-columns: minmax(0, 1fr);
  }

  .add-argument {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 34px;
    justify-content: flex-start;
    border: 0;
    border-radius: 0;
    padding: 0 11px;
    background: transparent;
    color: var(--text-link);
    font-size: 11.5px;
    font-weight: 700;
  }

  .add-argument:hover,
  .add-argument:focus-visible {
    background: var(--surface-hover);
  }
</style>
