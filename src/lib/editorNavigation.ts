export type EditorLocation = {
  path: string;
  line: number;
  character: number;
};

export type EditorNavigationState = {
  entries: EditorLocation[];
  index: number;
};

const MAX_NAVIGATION_ENTRIES = 100;

export function createEditorNavigation(): EditorNavigationState {
  return { entries: [], index: -1 };
}

export function pushEditorLocation(
  state: EditorNavigationState,
  location: EditorLocation,
): EditorNavigationState {
  const current = state.entries[state.index];
  if (current && locationsEqual(current, location)) return state;
  const entries = [...state.entries.slice(0, state.index + 1), location].slice(
    -MAX_NAVIGATION_ENTRIES,
  );
  return { entries, index: entries.length - 1 };
}

export function editorNavigationTarget(
  state: EditorNavigationState,
  direction: -1 | 1,
): { state: EditorNavigationState; location: EditorLocation } | null {
  const index = state.index + direction;
  const location = state.entries[index];
  return location ? { state: { ...state, index }, location } : null;
}

export function canNavigateEditor(state: EditorNavigationState, direction: -1 | 1): boolean {
  const index = state.index + direction;
  return index >= 0 && index < state.entries.length;
}

function locationsEqual(left: EditorLocation, right: EditorLocation): boolean {
  return left.path === right.path && left.line === right.line && left.character === right.character;
}
