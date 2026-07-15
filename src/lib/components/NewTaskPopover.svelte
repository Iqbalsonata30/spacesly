<script lang="ts">
  type Props = {
    title: string;
    description: string;
    onTitleChange: (value: string) => void;
    onDescriptionChange: (value: string) => void;
    onClose: () => void;
    onCreate: () => void;
  };

  let { title, description, onTitleChange, onDescriptionChange, onClose, onCreate }: Props =
    $props();
</script>

<aside class="new-task-popover" aria-label="Create new task">
  <button class="close-detail" type="button" aria-label="Close" onclick={onClose}>×</button>
  <div>
    <p class="section-kicker">New Task</p>
    <h3>Create Agent work</h3>
  </div>
  <label>
    <span>Task title</span>
    <input
      placeholder="What should the Agent do?"
      value={title}
      oninput={(event) => onTitleChange(event.currentTarget.value)}
    />
  </label>
  <label>
    <span>Description</span>
    <textarea
      placeholder="Add context, expected outcome, links, constraints, or anything the Agent should know."
      value={description}
      oninput={(event) => onDescriptionChange(event.currentTarget.value)}></textarea>
  </label>
  <footer>
    <span>This creates a local Spacesly card. It can run without Jira.</span>
    <button type="button" onclick={onCreate}>Create task</button>
  </footer>
</aside>
