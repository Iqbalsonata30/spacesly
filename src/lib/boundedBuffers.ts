export function capList<T>(items: T[], maxItems: number): T[] {
  return items.length > maxItems ? items.slice(items.length - maxItems) : items;
}

export function capText(value: string, maxChars: number): string {
  return value.length > maxChars ? value.slice(value.length - maxChars) : value;
}
