export interface TooltipOptions {
  text: string;
  shortcut?: string;
  placement?: "top" | "bottom";
}

export function tooltip(node: HTMLElement, options: TooltipOptions) {
  const opts = options;
  let element: HTMLDivElement | null = null;

  function attach() {
    if (element) return;
    element = document.createElement("div");
    element.className = "spacesly-tooltip";

    const label = document.createElement("span");
    label.className = "spacesly-tooltip-text";
    label.textContent = opts.text;
    element.appendChild(label);

    if (opts.shortcut) {
      const kbd = document.createElement("kbd");
      kbd.className = "korlap-tooltip-kbd";
      kbd.textContent = opts.shortcut;
      element.appendChild(kbd);
    }

    document.body.appendChild(element);
  }

  node.addEventListener("mouseenter", attach);
  node.addEventListener("focus", attach);

  return {
    destroy() {
      if (element) {
        element.remove();
      }
    },
  };
}
