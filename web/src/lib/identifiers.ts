const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function shortId(value: string): string {
  return UUID_PATTERN.test(value) ? value.slice(0, 8) : value;
}

export async function copyText(value: string): Promise<void> {
  await navigator.clipboard.writeText(value);
}
