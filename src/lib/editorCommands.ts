export type EditorCommandId =
  | "editor.save"
  | "editor.format"
  | "editor.close"
  | "editor.nextTab"
  | "editor.previousTab"
  | "editor.goToDefinition"
  | "editor.quickFix"
  | "editor.navigateBack"
  | "editor.navigateForward";

export type EditorCommandHandler = () => void | Promise<void>;

export function createEditorCommandRegistry() {
  const handlers = new Map<EditorCommandId, EditorCommandHandler>();

  return {
    register(command: EditorCommandId, handler: EditorCommandHandler) {
      handlers.set(command, handler);
      return () => {
        if (handlers.get(command) === handler) handlers.delete(command);
      };
    },
    execute(command: EditorCommandId): boolean {
      const handler = handlers.get(command);
      if (!handler) return false;
      void handler();
      return true;
    },
    has(command: EditorCommandId): boolean {
      return handlers.has(command);
    },
  };
}

export type EditorCommandRegistry = ReturnType<typeof createEditorCommandRegistry>;
