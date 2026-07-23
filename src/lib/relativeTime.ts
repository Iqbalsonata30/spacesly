export function relativeTimeLabel(value: string, now = Date.now()): string {
  let timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) {
    const match = value.trim().match(/^(\d{1,2}):(\d{2})(?::(\d{2}))?\s*(AM|PM)?$/i);
    if (!match) return value;
    const date = new Date(now);
    let hours = Number(match[1]);
    const meridiem = match[4]?.toUpperCase();
    if (meridiem === "PM" && hours < 12) hours += 12;
    if (meridiem === "AM" && hours === 12) hours = 0;
    date.setHours(hours, Number(match[2]), Number(match[3] ?? 0), 0);
    timestamp = date.getTime();
    if (timestamp > now) timestamp -= 24 * 60 * 60 * 1000;
  }

  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000));
  if (seconds < 2) return "Just now";
  if (seconds < 60) return `${seconds} sec ago`;

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;

  const hours = Math.floor(minutes / 60);
  return `${hours} hr ago`;
}
