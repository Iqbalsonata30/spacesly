<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { X } from "lucide-svelte";
  import {
    categoryLabel,
    skillCategories,
    skillTriggers,
    triggerLabel,
    validateAgentSkill,
    type AgentSkill,
    type SkillCategory,
    type SkillTrigger,
  } from "$lib/agentSkills";

  type Props = {
    skill: AgentSkill;
    skills: AgentSkill[];
    isNew: boolean;
    onSave: (skill: AgentSkill) => string | null;
    onCancel: () => void;
  };

  let { skill, skills, isNew, onSave, onCancel }: Props = $props();
  let form = $state(untrack(() => structuredClone($state.snapshot(skill))));
  let error = $state<string | null>(null);
  let nameInput: HTMLInputElement | null = $state(null);
  let dialogElement: HTMLFormElement | null = $state(null);
  let previousFocus: HTMLElement | null = null;

  onMount(() => {
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    nameInput?.focus();
  });
  onDestroy(() => previousFocus?.focus());

  function update(values: Partial<AgentSkill>) {
    form = { ...form, ...values };
    error = null;
  }

  function save() {
    const next: AgentSkill = {
      ...form,
      name: form.name.trim(),
      description: form.description.trim(),
      custom_category: form.custom_category.trim(),
      priority: form.priority,
      instructions: form.instructions.trim(),
      notes: form.notes.trim(),
      updated_at: new Date().toISOString(),
    };
    error = validateAgentSkill(next, skills);
    if (!error) error = onSave(next);
  }

  function handleKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      onCancel();
      return;
    }
    if (event.key !== "Tab" || !dialogElement) return;
    const focusable = [
      ...dialogElement.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    ];
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<div
  class="skill-editor-backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="skill-editor-title"
  tabindex="-1"
  onclick={(event) => event.target === event.currentTarget && onCancel()}
  onkeydown={handleKeydown}
>
  <form
    bind:this={dialogElement}
    class="skill-editor-dialog"
    onsubmit={(event) => {
      event.preventDefault();
      save();
    }}
  >
    <header>
      <div>
        <p>Agent Skill</p>
        <h3 id="skill-editor-title">{isNew ? "Create skill" : `Edit ${skill.name}`}</h3>
      </div>
      <button type="button" aria-label="Close skill editor" onclick={onCancel}
        ><X size={16} aria-hidden="true" /></button
      >
    </header>

    <div class="skill-editor-fields">
      <label>
        <span>Name</span>
        <input
          bind:this={nameInput}
          value={form.name}
          required
          maxlength="160"
          autocomplete="off"
          oninput={(event) => update({ name: event.currentTarget.value })}
        />
      </label>

      <label>
        <span>Description</span>
        <textarea
          class="skill-description-input"
          value={form.description}
          required
          maxlength="500"
          placeholder="Explain when and why this skill is useful."
          oninput={(event) => update({ description: event.currentTarget.value })}></textarea>
      </label>

      <div class="skill-editor-row">
        <label>
          <span>Category</span>
          <select
            value={form.category}
            onchange={(event) => update({ category: event.currentTarget.value as SkillCategory })}
          >
            {#each skillCategories as category (category)}
              <option value={category}>{categoryLabel(category)}</option>
            {/each}
          </select>
        </label>

        <label>
          <span>Trigger</span>
          <select
            value={form.trigger}
            onchange={(event) => {
              const trigger = event.currentTarget.value as SkillTrigger;
              update({ trigger, enabled: trigger === "disabled" ? false : form.enabled });
            }}
          >
            {#each skillTriggers as trigger (trigger)}
              <option value={trigger}>{triggerLabel(trigger)}</option>
            {/each}
          </select>
        </label>

        <label>
          <span>Priority</span>
          <input
            type="number"
            min="0"
            max="100"
            step="1"
            value={form.priority}
            oninput={(event) => update({ priority: Number(event.currentTarget.value) })}
          />
        </label>
      </div>

      {#if form.category === "custom"}
        <label>
          <span>Custom category</span>
          <input
            value={form.custom_category}
            maxlength="120"
            placeholder="For example, Observability"
            oninput={(event) => update({ custom_category: event.currentTarget.value })}
          />
        </label>
      {/if}

      <label class="skill-enabled-toggle">
        <input
          type="checkbox"
          checked={form.enabled}
          onchange={(event) =>
            update({
              enabled: event.currentTarget.checked,
              trigger:
                event.currentTarget.checked && form.trigger === "disabled"
                  ? "contextual"
                  : form.trigger,
            })}
        />
        <span>
          <strong>Enabled</strong>
          <small>Disabled skills are never loaded, including manual-only skills.</small>
        </span>
      </label>

      <label>
        <span>Instructions</span>
        <textarea
          class="skill-instructions-input"
          value={form.instructions}
          required
          placeholder="Write the ordered procedure in Markdown or plain text."
          oninput={(event) => update({ instructions: event.currentTarget.value })}></textarea>
        <small>Only selected skills are injected into an Agent execution prompt.</small>
      </label>

      <label>
        <span>Optional notes</span>
        <textarea
          class="skill-notes-input"
          value={form.notes}
          maxlength="4096"
          placeholder="Private maintenance notes. Notes are not injected into prompts."
          oninput={(event) => update({ notes: event.currentTarget.value })}></textarea>
      </label>

      {#if error}<p class="skill-editor-error" role="alert">{error}</p>{/if}
    </div>

    <footer>
      <span>Created {new Date(form.created_at).toLocaleDateString()}</span>
      <div>
        <button type="button" onclick={onCancel}>Cancel</button>
        <button class="skill-save" type="submit">{isNew ? "Create skill" : "Save changes"}</button>
      </div>
    </footer>
  </form>
</div>

<style>
  .skill-editor-backdrop {
    position: fixed;
    z-index: 60;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 24px;
    background: var(--overlay-bg);
    backdrop-filter: blur(8px);
  }

  .skill-editor-dialog {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    width: min(760px, 100%);
    max-height: min(860px, calc(100vh - 48px));
    overflow: hidden;
    border: 1px solid var(--border-strong);
    border-radius: 16px;
    background: var(--dialog-bg);
    box-shadow: var(--shadow-dialog);
  }

  header,
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 16px 18px;
  }

  header {
    border-bottom: 1px solid var(--border-subtle);
  }

  header p,
  header h3 {
    margin: 0;
  }

  header p {
    color: var(--accent);
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  header h3 {
    margin-top: 4px;
    color: var(--text-bright);
    font-size: 18px;
  }

  header button {
    width: 36px;
    height: 36px;
    border-radius: 10px;
  }

  .skill-editor-fields {
    display: grid;
    gap: 15px;
    min-height: 0;
    overflow-y: auto;
    padding: 18px;
  }

  label {
    display: grid;
    gap: 7px;
    color: var(--text-primary);
  }

  label > span:first-child {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  input,
  select,
  textarea {
    box-sizing: border-box;
    width: 100%;
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    padding: 10px 11px;
    background: var(--surface-inset);
    color: var(--text-primary);
    font: inherit;
  }

  input:focus-visible,
  select:focus-visible,
  textarea:focus-visible {
    border-color: var(--focus-border);
    outline: none;
    box-shadow: 0 0 0 3px var(--focus-ring);
  }

  textarea {
    resize: vertical;
    line-height: 1.5;
  }

  .skill-description-input,
  .skill-notes-input {
    min-height: 74px;
  }

  .skill-instructions-input {
    min-height: 220px;
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .skill-editor-row {
    display: grid;
    grid-template-columns: 1.2fr 1.2fr 0.6fr;
    gap: 12px;
  }

  .skill-enabled-toggle {
    grid-template-columns: auto minmax(0, 1fr);
    align-items: start;
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    padding: 12px;
    background: var(--surface-raised);
  }

  .skill-enabled-toggle input {
    width: 17px;
    margin-top: 2px;
    accent-color: var(--accent);
  }

  .skill-enabled-toggle span {
    display: grid;
    gap: 3px;
    letter-spacing: normal;
    text-transform: none;
  }

  .skill-enabled-toggle strong {
    color: var(--text-bright);
  }

  small,
  footer > span {
    color: var(--text-dim);
    font-size: 11px;
    line-height: 1.4;
  }

  .skill-editor-error {
    margin: 0;
    border: 1px solid var(--danger-border);
    border-radius: 9px;
    padding: 10px 12px;
    background: var(--danger-bg);
    color: var(--danger);
  }

  footer {
    border-top: 1px solid var(--border-subtle);
    background: var(--surface-raised);
  }

  footer div {
    display: flex;
    gap: 8px;
  }

  footer button {
    min-height: 38px;
    padding: 0 14px;
  }

  footer .skill-save {
    border-color: var(--selection-border);
    background: var(--surface-selected);
    color: var(--accent);
    font-weight: 900;
  }

  @media (max-width: 640px) {
    .skill-editor-backdrop {
      align-items: end;
      padding: 0;
    }

    .skill-editor-dialog {
      max-height: 94vh;
      border-radius: 16px 16px 0 0;
    }

    .skill-editor-row {
      grid-template-columns: 1fr;
    }

    footer {
      align-items: flex-start;
      flex-direction: column;
    }

    footer div {
      width: 100%;
    }

    footer button {
      flex: 1;
    }
  }
</style>
