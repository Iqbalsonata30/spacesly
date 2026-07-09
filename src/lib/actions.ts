export interface TooltipOptions {
  text: string;
  shortcut?: string;
  placement?: "top" | "bottom";
}

export function tooltip(node: HTMLElement, options: TooltipOptions) {
  let opts = options;
  let element: HTMLDivElement | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function show() {
    if (element || !opts.text) return;
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
}
